//! Lowering of the arguments and members of `DotNet` values (ADR-014).
use super::*;

impl Lowerer<'_> {
    /// An argument of a DotNet member: a plain variable goes by place, so a by-reference .NET
    /// parameter can write it back (ADR-014 section 4); anything else by value.
    pub(crate) fn dotnet_arg(&mut self, a: &SyntaxNode) -> L<ir::Arg> {
        let is_label = match self.res(a) {
            Some(Res::Local(n)) => self.local_labels.contains_key(n),
            Some(Res::Global(n)) => self.global_labels.contains_key(n),
            _ => false,
        };
        let variable = matches!(self.res(a), Some(Res::Local(_) | Res::Global(_) | Res::Field { .. }));
        if !is_label
            && variable
            && matches!(
                a.kind(),
                SyntaxKind::NameRef | SyntaxKind::MemberExpr | SyntaxKind::IndexExpr
            )
        {
            if let Ok(p) = self.place(a) {
                return Ok(ir::Arg::Ref(p));
            }
        }
        Ok(ir::Arg::Value(self.expr(a)?))
    }

    /// A member of a DotNet value (ADR-014): `callee` is the member expression, `op` says what to
    /// do to it. A call named like the receiver's alias is the constructor.
    pub(crate) fn dotnet_member(
        &mut self,
        _node: &SyntaxNode,
        callee: &SyntaxNode,
        name: &str,
        op: ir::DotNetOp,
        args: Vec<ir::Arg>,
    ) -> L<ir::Expr> {
        let recv = callee
            .children()
            .next()
            .filter(|_| callee.kind() == SyntaxKind::MemberExpr)
            .ok_or("a DotNet member without a receiver is not lowered")?;
        let Ty::DotNet(alias) = self.ty_of(&recv) else {
            return Err(format!("`{name}` is not a member of a DotNet value"));
        };
        let ty = if alias.is_empty() {
            None
        } else {
            Some(self.ws.resolve_dotnet(self.file, &alias).ok_or_else(|| {
                format!("DotNet '{alias}' has no dotnet {{ }} declaration in this app or its dependencies")
            })?)
        };
        if let (Some(i), ir::DotNetOp::Call(_)) = (ty, &op) {
            // The constructor is named like the alias or like the CLR type: the probe app declares
            // alias `DnStringBuilder` for `System.Text.StringBuilder` and calls `Builder.StringBuilder()`
            // (tests/al-language-onprem-dotnet/TestDotNetValueKindsContracts.al, codeunit 61400).
            let decl = &self.ws.dotnet[i].decl;
            let simple = decl.type_name.rsplit('.').next().unwrap_or("");
            let simple = simple.split('`').next().unwrap_or(simple);
            if decl.alias.eq_ignore_ascii_case(name) || simple.eq_ignore_ascii_case(name) {
                return Ok(ir::Expr::DotNet {
                    ty,
                    receiver: None,
                    op: ir::DotNetOp::Construct,
                    args,
                });
            }
        }
        Ok(ir::Expr::DotNet {
            ty,
            receiver: Some(Box::new(self.expr(&recv)?)),
            op,
            args,
        })
    }
}
