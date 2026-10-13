//! Resolves every name, member access and call in routine bodies, inferring
//! expression types. The corpus compiles on BC, so a diagnostic here means
//! the model is missing something, not that the AL is wrong.

use crate::members::{Members, ProcSig};
use crate::types::Ty;
use crate::workspace::{Diagnostic, ObjKind, Resolution, Workspace};
use al_builtins::catalog::Catalog;
use al_builtins::signature::Signature;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash (as in rustc): the per-expression maps below are keyed by three
/// small integers and written for every node, where SipHash dominated their
/// cost. Not DoS-resistant, which does not matter for compiler-internal keys;
/// it is also deterministic.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `HashMap` keyed by node position, hashed with [`FxHasher`].
pub type PosMap<V> = HashMap<(usize, usize, usize), V, BuildHasherDefault<FxHasher>>;

pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    /// Inferred type of every expression, keyed by (file index, start offset, end offset).
    pub types: PosMap<Ty>,
    /// What each name, member and call resolved to, same keys.
    pub resolutions: PosMap<Res>,
    /// The layout of every query object, by object index.
    pub queries: Vec<Option<crate::queries::QueryLayout>>,
}

/// The target of a resolved name, member or call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Res {
    /// A variable in the routine's scope (parameter, return value, local).
    Local(String),
    /// A global variable of the object (or an implicit one such as `Rec`).
    Global(String),
    /// Field `id` of table object `table`.
    Field {
        table: usize,
        id: u32,
    },
    /// A procedure: declaring object and the offset of its routine node.
    Proc {
        object: usize,
        offset: usize,
    },
    /// A catalog builtin overload id (ADR-008).
    Builtin(String),
    /// An enum value by its declared integer value (what `AsInteger()` returns).
    EnumValue {
        object: usize,
        value: i32,
    },
    OptionValue(i32),
    /// `Database::X` and friends: the object id.
    ObjectId(i32),
    /// Column `column` (position in `ir::Query::columns`) of query object `query`.
    QueryColumn {
        query: usize,
        column: usize,
    },
}

pub fn check(ws: &Workspace, members: &[Members], catalog: &Catalog) -> CheckResult {
    let mut extensions: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, o) in ws.objects.iter().enumerate() {
        if let (Some(k), Some(base)) = (o.kind.extended(), ws.object_ast(i).extends_parts()) {
            if let Resolution::Found(t) = ws.resolve_ref(o.file, k, &base) {
                extensions.entry(t).or_default().push(i);
            }
        }
    }
    let mut c = Checker {
        ws,
        members,
        catalog,
        diags: Vec::new(),
        types: Default::default(),
        resolutions: Default::default(),
        system_enums: system_enums(catalog),
        extensions,
        sections: Default::default(),
        queries: (0..ws.objects.len())
            .map(|oi| (ws.objects[oi].kind == ObjKind::Query).then(|| crate::queries::layout(ws, members, oi)))
            .collect(),
    };
    for oi in 0..ws.objects.len() {
        c.check_object(oi);
    }
    c.diags
        .sort_by_key(|d| (ws.files.iter().position(|f| f.path == d.file), d.offset));
    CheckResult {
        diagnostics: c.diags,
        types: c.types,
        resolutions: c.resolutions,
        queries: c.queries,
    }
}

/// Types that appear in the catalog's signatures but are not receivers or
/// primitives: the platform's option/enum types (`Verbosity`, `ObjectType`, ...),
/// whose values are written `Verbosity::Normal`.
fn system_enums(catalog: &Catalog) -> HashSet<String> {
    let receivers: HashSet<String> = catalog.signatures.iter().map(|s| s.receiver.to_lowercase()).collect();
    let mut out = HashSet::new();
    for s in &catalog.signatures {
        for t in s.params.iter().map(|p| p.ty.as_str()).chain([s.ret.as_str()]) {
            let t = t.to_lowercase();
            if !t.is_empty() && !t.contains(' ') && !receivers.contains(&t) {
                out.insert(t);
            }
        }
    }
    for extra in [
        "objecttype",
        "fieldtype",
        "fieldclass",
        "action",
        "pagebackgroundtaskerrorlevel",
        "testpermissions",
        "transactionmodel",
        "commitbehavior",
        "errorbehavior",
        "tableconnectiontype",
    ] {
        out.insert(extra.to_string());
    }
    out
}

/// What an expression denotes before it is used as a value.
#[derive(Debug, Clone)]
enum E {
    Val(Ty),
    /// A type used as a scope: `"ALT Status"` in `"ALT Status"::Open`, `Verbosity`.
    TypeName(Ty),
    /// `Database`, `Codeunit`, `Page`, ... before `::Name`.
    ObjKeyword(ObjKind),
    /// `Enum` before `::"Name"`.
    EnumKeyword,
    /// `Database::A.B` while the qualified name is still being read.
    PartialObj(ObjKind, Vec<String>),
    Callable(Vec<Cand>),
}

#[derive(Debug, Clone)]
struct Cand {
    min: usize,
    max: usize,
    ret: Ty,
    target: Res,
    /// Declared parameter types (catalog spelling or `Ty` display), for tie-breaks.
    param_tys: Vec<String>,
    /// Per parameter position: the argument names a field of the receiver
    /// (catalog parameters named `Field`, `Field1`, `Fields`, `FromField`, `Column`).
    fields: Vec<bool>,
    /// The variadic tail is a field list.
    rest_field: bool,
}

impl Cand {
    fn is_field_pos(&self, i: usize) -> bool {
        self.fields.get(i).copied().unwrap_or(self.rest_field)
    }
}

struct Scope {
    oi: usize,
    file: usize,
    vars: Vec<HashMap<String, Ty>>,
    /// The implicit record of table/page code (and `Rec` in codeunits with TableNo).
    rec: Option<Ty>,
    /// Whether the implicit record's fields and methods can be used unqualified.
    rec_unqualified: bool,
    /// Object whose procedures are callable unqualified.
    self_obj: usize,
}

/// A section with its first argument (its name, for controls and data items).
type NamedSection = (Option<String>, ast::Section);

struct Checker<'a> {
    ws: &'a Workspace,
    members: &'a [Members],
    catalog: &'a Catalog,
    diags: Vec<Diagnostic>,
    types: PosMap<Ty>,
    resolutions: PosMap<Res>,
    system_enums: HashSet<String>,
    /// Extended object → its extension objects.
    extensions: HashMap<usize, Vec<usize>>,
    /// `sections_of` memo by object, for `named_section`.
    sections: std::cell::RefCell<HashMap<usize, Vec<NamedSection>>>,
    /// The layout of each query object.
    queries: Vec<Option<crate::queries::QueryLayout>>,
}

fn name_of(node: &SyntaxNode) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::Ident | SyntaxKind::QuotedIdent | SyntaxKind::Integer
            )
        })
        .map(|t| ast::ident_text(&t))
        .unwrap_or_default()
}

fn is_expr(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        Literal
            | NameRef
            | BinExpr
            | PrefixExpr
            | MemberExpr
            | ScopeExpr
            | CallExpr
            | IndexExpr
            | ParenExpr
            | SetExpr
            | RangeExpr
            | TernaryExpr
    )
}

impl<'a> Checker<'a> {
    fn diag(&mut self, sc: &Scope, node: &SyntaxNode, code: &'static str, message: String) {
        self.diags.push(Diagnostic {
            file: self.ws.files[sc.file].path.clone(),
            offset: node.text_range().start().into(),
            code,
            message,
        });
    }

    fn record(&mut self, sc: &Scope, node: &SyntaxNode, r: Res) {
        let range = node.text_range();
        self.resolutions
            .insert((sc.file, range.start().into(), range.end().into()), r);
    }

    /// `Res::Field` if `name` is a field of the record type `t`.
    fn field_res(&self, sc: &Scope, t: &Ty, name: &str) -> Option<Res> {
        if let Ty::Object(ObjKind::Query, q) = t {
            let qi = self.resolve(sc, ObjKind::Query, q)?;
            let column = self.queries[qi].as_ref()?.column(name)?;
            return Some(Res::QueryColumn { query: qi, column });
        }
        let Ty::Record { table, .. } = t else { return None };
        let ti = self.resolve(sc, ObjKind::Table, table)?;
        self.members[ti].field(name).map(|f| Res::Field { table: ti, id: f.id })
    }

    fn resolve(&self, sc: &Scope, kind: ObjKind, name: &str) -> Option<usize> {
        let parts: Vec<String> = if name.parse::<u32>().is_ok() {
            vec![name.to_string()]
        } else {
            split_qualified(name)
        };
        match self.ws.resolve_ref(sc.file, kind, &parts) {
            Resolution::Found(i) => Some(i),
            _ => None,
        }
    }

    fn check_object(&mut self, oi: usize) {
        let obj = self.ws.object_ast(oi);
        let o = &self.ws.objects[oi];
        let mut globals = HashMap::new();
        for v in obj.var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            for n in v.names() {
                globals.insert(n.to_lowercase(), ty.clone());
            }
        }
        // Report/query/xmlport data items are record variables in the object's scope.
        let sections = ast::sections_in(obj.syntax());
        for sec in &sections {
            let n = sec.name().to_ascii_lowercase();
            if n == "dataitem" || n == "tableelement" {
                let args = sec.args();
                if let (Some(name), Some(table)) = (args.first(), args.get(1)) {
                    globals.insert(
                        name.to_lowercase(),
                        Ty::Record {
                            table: table.clone(),
                            temporary: false,
                        },
                    );
                }
            }
        }
        for sec in &sections {
            let n = sec.name().to_ascii_lowercase();
            if n == "textelement" || n == "textattribute" {
                if let Some(name) = sec.args().first() {
                    globals.insert(name.to_lowercase(), Ty::Text(None));
                }
            }
        }
        if matches!(
            o.kind,
            ObjKind::Table | ObjKind::TableExtension | ObjKind::Page | ObjKind::PageExtension
        ) {
            // System variable: the field number whose validation is running.
            globals.insert("currfieldno".into(), Ty::Integer);
        }
        let prop = |name: &str| {
            obj.properties()
                .find(|p| p.name().eq_ignore_ascii_case(name))
                .map(|p| p.value_text())
        };
        let unquote = |s: String| s.trim().trim_matches('"').to_string();
        let mut rec = None;
        let mut rec_unqualified = false;
        let mut self_obj = oi;
        match o.kind {
            ObjKind::Table => {
                rec = Some(Ty::Record {
                    table: o.name.clone(),
                    temporary: false,
                });
                rec_unqualified = true;
            }
            ObjKind::TableExtension => {
                if let Some(base) = obj.extends_parts() {
                    if let Resolution::Found(t) = self.ws.resolve_ref(o.file, ObjKind::Table, &base) {
                        rec = Some(Ty::Record {
                            table: self.ws.objects[t].name.clone(),
                            temporary: false,
                        });
                        self_obj = t;
                    } else {
                        rec = Some(Ty::Unknown);
                    }
                }
                rec_unqualified = true;
            }
            ObjKind::Page => {
                rec = prop("SourceTable").map(|t| Ty::Record {
                    table: unquote(t),
                    temporary: false,
                });
                rec_unqualified = true;
                globals.insert("currpage".into(), Ty::Object(ObjKind::Page, o.name.clone()));
            }
            ObjKind::PageExtension => {
                if let Some(base) = obj.extends_parts() {
                    if let Resolution::Found(p) = self.ws.resolve_ref(o.file, ObjKind::Page, &base) {
                        let pobj = self.ws.object_ast(p);
                        rec = pobj
                            .properties()
                            .find(|x| x.name().eq_ignore_ascii_case("SourceTable"))
                            .map(|x| Ty::Record {
                                table: unquote(x.value_text()),
                                temporary: false,
                            });
                        globals.insert(
                            "currpage".into(),
                            Ty::Object(ObjKind::Page, self.ws.objects[p].name.clone()),
                        );
                    } else {
                        rec = Some(Ty::Unknown);
                        globals.insert("currpage".into(), Ty::Unknown);
                    }
                }
                rec_unqualified = true;
            }
            ObjKind::Codeunit => {
                rec = prop("TableNo").map(|t| Ty::Record {
                    table: unquote(t),
                    temporary: false,
                })
            }
            ObjKind::Report | ObjKind::ReportExtension => {
                globals.insert("currreport".into(), Ty::Object(ObjKind::Report, o.name.clone()));
                globals.insert("requestoptionspage".into(), Ty::Builtin("RequestPage".into()));
            }
            ObjKind::XmlPort => {
                globals.insert("currxmlport".into(), Ty::Object(ObjKind::XmlPort, o.name.clone()));
            }
            _ => {}
        }
        if let Some(r) = &rec {
            globals.insert("rec".into(), r.clone());
            globals.insert("xrec".into(), r.clone());
        }
        // Report extensions see the base report's data items; `modify(X)` blocks
        // run with data item X as their record.
        let mut base_items: HashMap<String, String> = HashMap::new();
        if o.kind == ObjKind::ReportExtension {
            if let Some(base) = obj.extends_parts() {
                if let Resolution::Found(r) = self.ws.resolve_ref(o.file, ObjKind::Report, &base) {
                    for sec in ast::sections_in(self.ws.object_ast(r).syntax()) {
                        if sec.name().eq_ignore_ascii_case("dataitem") {
                            let a = sec.args();
                            if let (Some(n), Some(t)) = (a.first(), a.get(1)) {
                                base_items.insert(n.to_lowercase(), t.clone());
                                globals.insert(
                                    n.to_lowercase(),
                                    Ty::Record {
                                        table: t.clone(),
                                        temporary: false,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        let file = o.file;
        for r in ast::routines_in(obj.syntax()) {
            let item_rec = r.syntax().ancestors().filter_map(ast::Section::cast).find_map(|s| {
                let a = s.args();
                match s.name().to_ascii_lowercase().as_str() {
                    "dataitem" => a.get(1).cloned(),
                    "modify" => a.first().and_then(|n| base_items.get(&n.to_lowercase()).cloned()),
                    _ => None,
                }
            });
            let (rec, rec_unqualified) = match item_rec {
                Some(t) => (
                    Some(Ty::Record {
                        table: t,
                        temporary: false,
                    }),
                    true,
                ),
                None => (rec.clone(), rec_unqualified),
            };
            let mut sc = Scope {
                oi,
                file,
                vars: vec![globals.clone()],
                rec,
                rec_unqualified,
                self_obj,
            };
            let mut locals = HashMap::new();
            for p in r.params() {
                locals.insert(
                    p.name().to_lowercase(),
                    p.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown),
                );
            }
            if let (Some(n), Some(t)) = (r.return_name(), r.return_type()) {
                locals.insert(n.to_lowercase(), Ty::from_type_ref(&t));
            }
            for v in r.local_var_decls() {
                let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
                for n in v.names() {
                    locals.insert(n.to_lowercase(), ty.clone());
                }
            }
            sc.vars.push(locals);
            let before = self.diags.len();
            if let Some(body) = r.body() {
                self.walk(&mut sc, &body);
            }
            // Property expressions of controls were never checked before the test
            // client needed them (ADR-012); an unresolved name there stays a refusal at
            // lowering, not a compile diagnostic (check::tests::builtin_test_page_methods_win_over_controls_when_called).
            if crate::pages::is_synthetic(&r.name()) {
                self.diags.truncate(before);
            }
        }
    }

    fn walk(&mut self, sc: &mut Scope, node: &SyntaxNode) {
        if is_expr(node.kind()) {
            self.ty(sc, node);
            return;
        }
        for child in node.children() {
            self.walk(sc, &child);
        }
    }

    /// Infers `node` as a value, calling a parameterless callable.
    fn ty(&mut self, sc: &mut Scope, node: &SyntaxNode) -> Ty {
        let e = self.infer(sc, node);
        let t = match e {
            E::Val(t) => t,
            E::Callable(c) => self.pick(sc, node, &c, &[], &name_of(node)),
            E::PartialObj(k, parts) => {
                self.diag(
                    sc,
                    node,
                    "AR0002",
                    format!("unresolved {} `{}`", k.label(), parts.join(".")),
                );
                Ty::Integer
            }
            _ => Ty::Unknown,
        };
        let r = node.text_range();
        self.types
            .insert((sc.file, r.start().into(), r.end().into()), t.clone());
        t
    }

    /// Chooses an overload: by arity, then by how many argument types fit the
    /// declared parameter types (first in catalog order on a tie). Records the
    /// choice on `node`.
    fn pick(&mut self, sc: &Scope, node: &SyntaxNode, cands: &[Cand], args: &[Ty], name: &str) -> Ty {
        let argc = args.len();
        let mut best: Option<(&Cand, usize)> = None;
        for c in cands.iter().filter(|c| c.min <= argc && argc <= c.max) {
            let score = args.iter().zip(&c.param_tys).filter(|(a, p)| fits(a, p)).count();
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((c, score));
            }
        }
        match best {
            Some((c, _)) => {
                let (ret, target) = (c.ret.clone(), c.target.clone());
                self.record(sc, node, target);
                ret
            }
            None => {
                self.diag(
                    sc,
                    node,
                    "AR1003",
                    format!("no overload of `{name}` takes {argc} argument(s)"),
                );
                Ty::Unknown
            }
        }
    }

    fn infer(&mut self, sc: &mut Scope, node: &SyntaxNode) -> E {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = node.children().collect();
        match node.kind() {
            Literal => E::Val(literal_ty(node)),
            ParenExpr => E::Val(kids.first().map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown)),
            NameRef => self.name(sc, node),
            PrefixExpr => {
                let t = kids.first().map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown);
                let op = first_token(node).to_ascii_lowercase();
                E::Val(if op == "not" { Ty::Boolean } else { t })
            }
            BinExpr => {
                let l = kids.first().map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown);
                let r = kids.get(1).map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown);
                E::Val(binary_ty(&op_token(node), &l, &r))
            }
            TernaryExpr => {
                if let Some(c) = kids.first() {
                    self.ty(sc, c);
                }
                let t = kids.get(1).map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown);
                if let Some(e) = kids.get(2) {
                    self.ty(sc, e);
                }
                E::Val(t)
            }
            SetExpr | RangeExpr => {
                for k in &kids {
                    self.ty(sc, k);
                }
                E::Val(Ty::Unknown)
            }
            IndexExpr => {
                let base = kids.first().map(|k| self.ty(sc, k)).unwrap_or(Ty::Unknown);
                for k in kids.iter().skip(1) {
                    self.ty(sc, k);
                }
                E::Val(match base {
                    Ty::Array(_, el) => *el,
                    Ty::Text(_) | Ty::Code(_) => Ty::Char,
                    _ => Ty::Unknown,
                })
            }
            CallExpr => {
                let callee = &kids[0];
                let args: Vec<SyntaxNode> = kids.get(1).map(|a| a.children().collect()).unwrap_or_default();
                let target = match (callee.kind() == NameRef)
                    .then(|| self.lookup_callable(sc, &name_of(callee)))
                    .flatten()
                {
                    Some(c) => E::Callable(c),
                    None => self.infer(sc, callee),
                };
                // The receiver whose fields bare-name arguments may refer to.
                let receiver = (callee.kind() == MemberExpr)
                    .then(|| callee.children().next())
                    .flatten()
                    .and_then(|b| {
                        let r = b.text_range();
                        self.types.get(&(sc.file, r.start().into(), r.end().into())).cloned()
                    });
                let cands = match &target {
                    E::Callable(c) => c.clone(),
                    _ => Vec::new(),
                };
                let unknown_callee = matches!(target, E::Val(Ty::Unknown));
                let mut arg_tys = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    // On a receiver outside the workspace, a bare name may be one of
                    // its fields: resolve it if we can, but never report it.
                    if unknown_callee && a.kind() == NameRef {
                        let t = match self.lookup_name(sc, a) {
                            Some(E::Val(t)) => t,
                            _ => Ty::Unknown,
                        };
                        let r = a.text_range();
                        self.types
                            .insert((sc.file, r.start().into(), r.end().into()), t.clone());
                        arg_tys.push(t);
                        continue;
                    }
                    let field_pos = cands.iter().any(|c| c.is_field_pos(i));
                    if field_pos && a.kind() == NameRef {
                        if let Some(t) = receiver.as_ref().and_then(|r| self.field_of(sc, r, &name_of(a))) {
                            let r = a.text_range();
                            self.types
                                .insert((sc.file, r.start().into(), r.end().into()), t.clone());
                            if let Some(r) = receiver.as_ref().and_then(|r| self.field_res(sc, r, &name_of(a))) {
                                if matches!(r, Res::QueryColumn { .. }) {
                                    self.record(sc, a, r);
                                }
                            }
                            // A `TestPage.Filter` field is a field of the page's source table
                            // (corpus testfilter/TestTestFilter.al:1-30).
                            let table_idx = match &receiver {
                                Some(Ty::Record { table, .. } | Ty::TestFilter(table)) => {
                                    self.resolve(sc, ObjKind::Table, table)
                                }
                                _ => None,
                            };
                            if let Some(ti) = table_idx {
                                if let Some(f) = self.members[ti].field(&name_of(a)) {
                                    let id = f.id;
                                    self.record(sc, a, Res::Field { table: ti, id });
                                }
                            }
                            arg_tys.push(t);
                            continue;
                        }
                    }
                    arg_tys.push(self.ty(sc, a));
                }
                match target {
                    E::Callable(c) => E::Val(self.pick(sc, node, &c, &arg_tys, &member_name(callee))),
                    E::Val(Ty::Unknown) => E::Val(Ty::Unknown),
                    E::Val(t) => {
                        self.diag(
                            sc,
                            callee,
                            "AR1004",
                            format!("`{}` of type {t} is not callable", callee.text()),
                        );
                        E::Val(Ty::Unknown)
                    }
                    _ => E::Val(Ty::Unknown),
                }
            }
            MemberExpr => {
                let base = self.infer(sc, &kids[0]);
                let mname = kids.get(1).map(name_of).unwrap_or_default();
                if let E::PartialObj(k, mut parts) = base {
                    parts.push(mname);
                    return match self.ws.resolve_ref(sc.file, k, &parts) {
                        Resolution::Found(i) => {
                            let id = self.ws.objects[i].id.unwrap_or(0) as i32;
                            self.record(sc, node, Res::ObjectId(id));
                            E::Val(Ty::Integer)
                        }
                        _ => E::PartialObj(k, parts),
                    };
                }
                let base_ty = match base {
                    E::Val(t) => t,
                    E::Callable(c) => self.pick(sc, &kids[0], &c, &[], &member_name(&kids[0])),
                    E::TypeName(t) => {
                        // Static members of a type: `"ALT Status".FromInteger(1)`. Lowering reads
                        // the enum behind the type name from the receiver's recorded type.
                        let r = kids[0].text_range();
                        self.types
                            .insert((sc.file, r.start().into(), r.end().into()), t.clone());
                        return match self.static_members(&t, &mname) {
                            Some(c) => E::Callable(c),
                            None => E::Val(Ty::Unknown),
                        };
                    }
                    E::ObjKeyword(k) => {
                        // `Codeunit.Run(...)`, `Page.RunModal(...)`, `Database.Commit()`.
                        let recv = match k {
                            ObjKind::Table => "Database",
                            other => kind_receiver(other),
                        };
                        let c = self.catalog_cands(recv, &mname, false, &Ty::Unknown);
                        return if c.is_empty() {
                            self.diag(sc, node, "AR1002", format!("no member `{mname}` on {recv}"));
                            E::Val(Ty::Unknown)
                        } else {
                            E::Callable(c)
                        };
                    }
                    E::EnumKeyword | E::PartialObj(..) => return E::Val(Ty::Unknown),
                };
                let r = kids[0].text_range();
                self.types
                    .insert((sc.file, r.start().into(), r.end().into()), base_ty.clone());
                if let Some(r) = self.field_res(sc, &base_ty, &mname) {
                    self.record(sc, node, r);
                }
                // `Host.OpenView.Invoke()` is the control called OpenView, not the built-in method:
                // a member that is itself the receiver of another member can only be a control
                // (corpus handlers/TestPageActionRunPageMode.al: actions named OpenView and OpenEdit).
                if matches!(base_ty, Ty::TestPage(_) | Ty::TestRequestPage(_) | Ty::TestPart(_))
                    && node
                        .parent()
                        .is_some_and(|p| p.kind() == MemberExpr && p.children().next().as_ref() == Some(node))
                {
                    if let Some(e) = self.page_control_member(sc, &base_ty, &mname) {
                        return e;
                    }
                }
                match self.member(sc, &base_ty, &mname) {
                    Some(e) => e,
                    None => {
                        self.diag(sc, node, "AR1002", format!("no member `{mname}` on {base_ty}"));
                        E::Val(Ty::Unknown)
                    }
                }
            }
            ScopeExpr => {
                let rhs = kids.get(1).map(name_of).unwrap_or_default();
                let base = if kids[0].kind() == NameRef {
                    match self.lookup_name(sc, &kids[0]) {
                        Some(e) => e,
                        None => {
                            // `"Some Enum"::Value` where the enum is outside the workspace.
                            self.diag(sc, node, "AR0002", format!("unresolved enum `{}`", name_of(&kids[0])));
                            return E::Val(Ty::Unknown);
                        }
                    }
                } else {
                    self.infer(sc, &kids[0])
                };
                match base {
                    E::ObjKeyword(k) => match self.resolve(sc, k, &rhs) {
                        Some(i) => {
                            let id = self.ws.objects[i].id.unwrap_or(0) as i32;
                            self.record(sc, node, Res::ObjectId(id));
                            E::Val(Ty::Integer)
                        }
                        None => E::PartialObj(k, vec![rhs]),
                    },
                    E::PartialObj(k, parts) => E::PartialObj(k, parts),
                    E::EnumKeyword => match self.resolve(sc, ObjKind::Enum, &rhs) {
                        Some(i) => E::TypeName(Ty::Enum(self.ws.objects[i].name.clone())),
                        None => {
                            self.diag(sc, node, "AR0002", format!("unresolved enum `{rhs}`"));
                            E::TypeName(Ty::Unknown)
                        }
                    },
                    E::TypeName(t) | E::Val(t) => {
                        match &t {
                            Ty::Enum(name) => {
                                if let Some(ei) = self.resolve(sc, ObjKind::Enum, name) {
                                    if let Some((v, _)) = self.members[ei]
                                        .enum_values
                                        .iter()
                                        .find(|(_, n)| n.eq_ignore_ascii_case(&rhs))
                                    {
                                        let value = *v as i32;
                                        self.record(sc, node, Res::EnumValue { object: ei, value });
                                    }
                                }
                            }
                            Ty::Option(members) => {
                                if let Some(i) = members.iter().position(|m| m.eq_ignore_ascii_case(&rhs)) {
                                    self.record(sc, node, Res::OptionValue(i as i32));
                                }
                            }
                            // `FieldClass::Normal`: platform option types.
                            Ty::Builtin(n) => {
                                if let Some(i) = al_builtins::sysenum::members(n)
                                    .and_then(|m| m.iter().position(|m| m.eq_ignore_ascii_case(&rhs)))
                                {
                                    self.record(sc, node, Res::OptionValue(i as i32));
                                }
                            }
                            _ => {}
                        }
                        E::Val(t)
                    }
                    E::Callable(c) => E::Val(self.pick(sc, &kids[0], &c, &[], &rhs)),
                }
            }
            _ => E::Val(Ty::Unknown),
        }
    }

    fn name(&mut self, sc: &mut Scope, node: &SyntaxNode) -> E {
        match self.lookup_name(sc, node) {
            Some(e) => e,
            None => {
                self.diag(sc, node, "AR1000", format!("unresolved name `{}`", name_of(node)));
                E::Val(Ty::Unknown)
            }
        }
    }

    /// Procedures and functions named `n`, skipping variables and fields: a
    /// call `UserSecurityId()` means the function even in a table with a
    /// field of that name.
    fn lookup_callable(&mut self, sc: &mut Scope, n: &str) -> Option<Vec<Cand>> {
        let mut c: Vec<Cand> = self.members[sc.self_obj]
            .procedures_named(n)
            .iter()
            .map(|p| proc_cand(p))
            .collect();
        if sc.self_obj != sc.oi {
            c.extend(self.members[sc.oi].procedures_named(n).iter().map(|p| proc_cand(p)));
        }
        if c.is_empty() && sc.rec_unqualified {
            if let Some(rec) = sc.rec.clone() {
                if let Some(recv) = receiver_of(&rec) {
                    c = self.catalog_cands(&recv, n, true, &rec);
                }
            }
        }
        if c.is_empty() {
            c = self
                .catalog
                .global(n)
                .iter()
                .map(|s| sig_cand(s, &Ty::Unknown, self.catalog.id(s)))
                .collect();
        }
        (!c.is_empty()).then_some(c)
    }

    /// Name lookup without reporting: None if the name does not resolve.
    fn lookup_name(&mut self, sc: &mut Scope, node: &SyntaxNode) -> Option<E> {
        let n = name_of(node);
        let key = n.to_lowercase();
        for (depth, frame) in sc.vars.iter().enumerate().rev() {
            if let Some(t) = frame.get(&key) {
                let t = t.clone();
                self.record(sc, node, if depth == 0 { Res::Global(key) } else { Res::Local(key) });
                return Some(E::Val(t));
            }
        }
        if sc.rec_unqualified {
            if let Some(rec) = sc.rec.clone() {
                if rec == Ty::Unknown {
                    return Some(E::Val(Ty::Unknown));
                }
                if let Some(e) = self.member(sc, &rec, &n) {
                    if let Some(r) = self.field_res(sc, &rec, &n) {
                        self.record(sc, node, r);
                    }
                    return Some(e);
                }
            }
        }
        let own: Vec<Cand> = self.members[sc.self_obj]
            .procedures_named(&n)
            .iter()
            .map(|p| proc_cand(p))
            .collect();
        let own_obj: Vec<Cand> = if sc.self_obj != sc.oi {
            self.members[sc.oi]
                .procedures_named(&n)
                .iter()
                .map(|p| proc_cand(p))
                .collect()
        } else {
            Vec::new()
        };
        if !own.is_empty() || !own_obj.is_empty() {
            return Some(E::Callable(own.into_iter().chain(own_obj).collect()));
        }
        let globals: Vec<Cand> = self
            .catalog
            .global(&n)
            .iter()
            .map(|s| sig_cand(s, &Ty::Unknown, self.catalog.id(s)))
            .collect();
        if !globals.is_empty() {
            return Some(E::Callable(globals));
        }
        let keyword = match key.as_str() {
            "database" | "table" => Some(E::ObjKeyword(ObjKind::Table)),
            "codeunit" => Some(E::ObjKeyword(ObjKind::Codeunit)),
            "page" => Some(E::ObjKeyword(ObjKind::Page)),
            "report" => Some(E::ObjKeyword(ObjKind::Report)),
            "query" => Some(E::ObjKeyword(ObjKind::Query)),
            "xmlport" => Some(E::ObjKeyword(ObjKind::XmlPort)),
            "enum" => Some(E::EnumKeyword),
            _ => None,
        };
        if keyword.is_some() {
            return keyword;
        }
        if let Some(i) = self.resolve(sc, ObjKind::Enum, &n) {
            return Some(E::TypeName(Ty::Enum(self.ws.objects[i].name.clone())));
        }
        if key == "action" {
            return Some(E::TypeName(Ty::Option(
                al_builtins::value::ACTION_MEMBERS
                    .iter()
                    .map(|m| m.to_string())
                    .collect(),
            )));
        }
        if key == "promptmode" {
            return Some(E::TypeName(Ty::Option(
                al_builtins::value::PROMPT_MODE_MEMBERS.iter().map(|m| m.to_string()).collect(),
            )));
        }
        if key == "textencoding" {
            return Some(E::TypeName(Ty::Option(crate::types::text_encoding_members())));
        }
        if self.system_enums.contains(&key) {
            return Some(E::TypeName(Ty::Builtin(n)));
        }
        // Library types used statically: `JsonObject`, `NavApp`, `TaskScheduler`, ...
        if self
            .catalog
            .signatures
            .iter()
            .any(|s| s.receiver.eq_ignore_ascii_case(&n))
        {
            return Some(E::TypeName(Ty::Builtin(n)));
        }
        None
    }

    /// Static methods reachable through a type name.
    fn static_members(&self, t: &Ty, name: &str) -> Option<Vec<Cand>> {
        let recv = receiver_of(t)?;
        let c = self.catalog_cands(&recv, name, false, t);
        (!c.is_empty()).then_some(c)
    }

    fn catalog_cands(&self, receiver: &str, name: &str, instance_only: bool, recv_ty: &Ty) -> Vec<Cand> {
        self.catalog
            .overloads(receiver, name)
            .into_iter()
            .filter(|s| !instance_only || s.instance)
            .map(|s| sig_cand(s, recv_ty, self.catalog.id(s)))
            .collect()
    }

    /// The control `name` of page (or request page) `oi`, typed as a test client handle.
    fn control_of_section(&self, oi: usize, name: &str) -> Option<E> {
        let sec = self.named_section(oi, name)?;
        Some(E::Val(match sec.name().to_ascii_lowercase().as_str() {
            "dataitem" => match sec.args().get(1) {
                Some(table) => Ty::TestFilter(table.clone()),
                None => Ty::Unknown,
            },
            "part" => match sec.args().get(1) {
                Some(page) => Ty::TestPart(page.clone()),
                None => Ty::Unknown,
            },
            "systempart" => Ty::Builtin("TestPart".into()),
            "action" | "actionref" | "customaction" | "systemaction" | "fileuploadaction" => {
                Ty::Builtin("TestAction".into())
            }
            _ => Ty::Builtin("TestField".into()),
        }))
    }

    /// `TP.Name` as a control of the page `TP` shows, if it has one.
    fn page_control_member(&mut self, sc: &Scope, t: &Ty, name: &str) -> Option<E> {
        let (Ty::TestPage(p) | Ty::TestRequestPage(p) | Ty::TestPart(p)) = t else {
            return None;
        };
        let kind = if matches!(t, Ty::TestRequestPage(_)) {
            ObjKind::Report
        } else {
            ObjKind::Page
        };
        let oi = self.resolve(sc, kind, p)?;
        self.control_of_section(oi, name)
    }

    /// Members of a value of type `t`: fields, procedures, library methods.
    fn member(&mut self, sc: &Scope, t: &Ty, name: &str) -> Option<E> {
        match t {
            Ty::Unknown | Ty::Variant if receiver_of(t).is_none() => return Some(E::Val(Ty::Unknown)),
            Ty::Record { table, .. } => {
                let Some(ti) = self.resolve(sc, ObjKind::Table, table) else {
                    return Some(E::Val(Ty::Unknown));
                };
                if let Some(f) = self.members[ti].field(name) {
                    return Some(E::Val(f.ty.clone()));
                }
                let procs: Vec<Cand> = self.members[ti]
                    .procedures_named(name)
                    .iter()
                    .map(|p| proc_cand(p))
                    .collect();
                if !procs.is_empty() {
                    return Some(E::Callable(procs));
                }
            }
            Ty::Object(kind, oname) => {
                let Some(oi) = self.resolve(sc, *kind, oname) else {
                    return Some(E::Val(Ty::Unknown));
                };
                let procs: Vec<Cand> = self.members[oi]
                    .procedures_named(name)
                    .iter()
                    .map(|p| proc_cand(p))
                    .collect();
                if !procs.is_empty() {
                    return Some(E::Callable(procs));
                }
                // `CurrPage.<Part>`: the part control, whose `.Page` is the page it shows.
                if *kind == ObjKind::Page {
                    if let Some(sec) = self.named_section(oi, name) {
                        if sec.name().eq_ignore_ascii_case("part") {
                            if let Some(page) = sec.args().get(1) {
                                return Some(E::Val(Ty::TestPart(page.clone())));
                            }
                        }
                    }
                }
                if *kind == ObjKind::Query {
                    if let Some(l) = &self.queries[oi] {
                        if let Some(c) = l.column(name) {
                            return Some(E::Val(l.column_tys[c].clone()));
                        }
                    }
                }
                if matches!(
                    kind,
                    ObjKind::Page | ObjKind::Query | ObjKind::Report | ObjKind::XmlPort
                ) && self.has_named_section(oi, name)
                {
                    return Some(E::Val(Ty::Unknown));
                }
            }
            Ty::TestPage(p) | Ty::TestRequestPage(p) | Ty::TestPart(p) => {
                // Built-in test page members (`OK`, `Close`, `Filter`, ...) win over
                // controls of the same name: corpus `Modal.OK().Invoke()` compiles.
                let kind = if matches!(t, Ty::TestRequestPage(_)) {
                    ObjKind::Report
                } else {
                    ObjKind::Page
                };
                let resolved = self.resolve(sc, kind, p);
                let recv = receiver_of(t).expect("test page types have a receiver");
                let mut c = self.catalog_cands(&recv, name, true, t);
                if !c.is_empty() {
                    // `TP.Filter` filters the page's source table.
                    let filter = resolved
                        .and_then(|o| self.source_table(o))
                        .map(Ty::TestFilter)
                        .unwrap_or(Ty::Unknown);
                    for cand in &mut c {
                        if cand.ret == Ty::Builtin("TestFilter".into()) {
                            cand.ret = filter.clone();
                        }
                    }
                    return Some(E::Callable(c));
                }
                if let (Ty::TestPart(part), true) = (t, name.eq_ignore_ascii_case("Page")) {
                    if resolved.is_some_and(|oi| self.named_section(oi, name).is_none()) {
                        return Some(E::Val(Ty::Object(ObjKind::Page, part.clone())));
                    }
                }
                if let Some(oi) = resolved {
                    if let Some(e) = self.control_of_section(oi, name) {
                        return Some(e);
                    }
                } else {
                    return Some(E::Val(Ty::Unknown));
                }
            }
            _ => {}
        }
        let recv = receiver_of(t)?;
        let c = self.catalog_cands(&recv, name, true, t);
        if !c.is_empty() {
            return Some(E::Callable(c));
        }
        // Label and TextConst values have the Text methods too.
        if matches!(t, Ty::Label) {
            let c = self.catalog_cands("Text", name, true, t);
            if !c.is_empty() {
                return Some(E::Callable(c));
            }
        }
        None
    }

    /// The type of field `name` of the table behind `t` (a record, a test
    /// page's filter, or a query's column).
    fn field_of(&self, sc: &Scope, t: &Ty, name: &str) -> Option<Ty> {
        match t {
            Ty::Record { table, .. } => match self.resolve(sc, ObjKind::Table, table) {
                Some(ti) => self.members[ti].field(name).map(|f| f.ty.clone()),
                // The table is outside the workspace (already reported); do not guess.
                None => Some(Ty::Unknown),
            },
            Ty::Unknown => Some(Ty::Unknown),
            Ty::TestFilter(table) => self.field_of(
                sc,
                &Ty::Record {
                    table: table.clone(),
                    temporary: false,
                },
                name,
            ),
            Ty::TestPage(page) | Ty::TestPart(page) => {
                let Some(pi) = self.resolve(sc, ObjKind::Page, page) else {
                    return Some(Ty::Unknown);
                };
                let table = self.source_table(pi)?;
                self.field_of(
                    sc,
                    &Ty::Record {
                        table,
                        temporary: false,
                    },
                    name,
                )
            }
            Ty::Object(ObjKind::Query, q) => {
                let qi = self.resolve(sc, ObjKind::Query, q)?;
                if let Some(c) = self.queries[qi].as_ref().and_then(|l| l.column(name).map(|c| (l, c))) {
                    return Some(c.0.column_tys[c.1].clone());
                }
                self.has_named_section(qi, name).then_some(Ty::Unknown)
            }
            _ => None,
        }
    }

    /// A named control/data item/column of object `oi` or of one of its extensions.
    fn named_section(&self, oi: usize, name: &str) -> Option<ast::Section> {
        std::iter::once(oi)
            .chain(self.extensions.get(&oi).cloned().unwrap_or_default())
            .find_map(|o| {
                self.sections
                    .borrow_mut()
                    .entry(o)
                    .or_insert_with(|| {
                        ast::sections_in(self.ws.object_ast(o).syntax())
                            .into_iter()
                            .map(|s| (s.args().into_iter().next(), s))
                            .collect()
                    })
                    .iter()
                    .find(|(first, _)| first.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(name)))
                    .map(|(_, s)| s.clone())
            })
    }

    fn source_table(&self, page: usize) -> Option<String> {
        let v = self
            .ws
            .object_ast(page)
            .properties()
            .find(|p| p.name().eq_ignore_ascii_case("SourceTable"))?
            .value_text();
        Some(v.trim().trim_matches('"').to_string())
    }

    fn has_named_section(&self, oi: usize, name: &str) -> bool {
        self.named_section(oi, name).is_some()
    }
}

fn split_qualified(name: &str) -> Vec<String> {
    crate::types::qualified_parts(name)
}

fn first_token(node: &SyntaxNode) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_string())
        .unwrap_or_default()
}

fn op_token(node: &SyntaxNode) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_ascii_lowercase())
        .unwrap_or_default()
}

fn member_name(node: &SyntaxNode) -> String {
    match node.kind() {
        SyntaxKind::MemberExpr | SyntaxKind::ScopeExpr => {
            node.children().nth(1).map(|n| name_of(&n)).unwrap_or_default()
        }
        _ => name_of(node),
    }
}

fn literal_ty(node: &SyntaxNode) -> Ty {
    let Some(t) = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
    else {
        return Ty::Unknown;
    };
    match t.kind() {
        SyntaxKind::Integer => Ty::Integer,
        SyntaxKind::Decimal => Ty::Decimal,
        SyntaxKind::StringLit => Ty::Text(None),
        SyntaxKind::DateLit => Ty::Date,
        SyntaxKind::TimeLit => Ty::Time,
        SyntaxKind::DateTimeLit => Ty::DateTime,
        SyntaxKind::BigIntLit => Ty::BigInteger,
        SyntaxKind::Ident => Ty::Boolean,
        _ => Ty::Unknown,
    }
}

fn is_numeric(t: &Ty) -> bool {
    matches!(t, Ty::Integer | Ty::BigInteger | Ty::Decimal | Ty::Char | Ty::Byte)
}

fn binary_ty(op: &str, l: &Ty, r: &Ty) -> Ty {
    match op {
        "=" | "<>" | "<" | "<=" | ">" | ">=" | "in" | "and" | "or" | "xor" => Ty::Boolean,
        "div" | "mod" => Ty::Integer,
        "/" if is_numeric(l) && is_numeric(r) => Ty::Decimal,
        "+" | "-" | "*" | "/" => match (l, r) {
            (Ty::Decimal, _) | (_, Ty::Decimal) if is_numeric(l) && is_numeric(r) => Ty::Decimal,
            (Ty::BigInteger, _) | (_, Ty::BigInteger) if is_numeric(l) && is_numeric(r) => Ty::BigInteger,
            (a, b) if is_numeric(a) && is_numeric(b) => Ty::Integer,
            (Ty::Text(_) | Ty::Code(_) | Ty::Label, _) | (_, Ty::Text(_) | Ty::Code(_) | Ty::Label) if op == "+" => {
                Ty::Text(None)
            }
            (Ty::Date, Ty::Date) if op == "-" => Ty::Integer,
            (Ty::Date, _) => Ty::Date,
            (Ty::DateTime, Ty::DateTime) if op == "-" => Ty::Duration,
            (Ty::DateTime, _) => Ty::DateTime,
            (Ty::Time, Ty::Time) if op == "-" => Ty::Duration,
            (Ty::Time, _) => Ty::Time,
            (Ty::Duration, _) => Ty::Duration,
            _ => Ty::Unknown,
        },
        _ => Ty::Unknown,
    }
}

fn kind_receiver(k: ObjKind) -> &'static str {
    match k {
        ObjKind::Codeunit => "Codeunit",
        ObjKind::Page => "Page",
        ObjKind::Report => "Report",
        ObjKind::Query => "Query",
        ObjKind::XmlPort => "XmlPort",
        ObjKind::Table => "Record",
        _ => "",
    }
}

/// The catalog receiver whose instance methods a value of `t` has.
fn receiver_of(t: &Ty) -> Option<String> {
    Some(
        match t {
            Ty::Integer => "Integer",
            Ty::BigInteger => "BigInteger",
            Ty::Decimal => "Decimal",
            Ty::Boolean => "Boolean",
            Ty::Byte => "Byte",
            Ty::Text(_) | Ty::Code(_) | Ty::Char => "Text",
            Ty::Label => "Label",
            Ty::Date => "Date",
            Ty::Time => "Time",
            Ty::DateTime => "DateTime",
            Ty::Duration => "Duration",
            Ty::Guid => "Guid",
            Ty::Enum(_) => "Enum",
            Ty::Record { .. } => "Record",
            Ty::Object(k, _) => kind_receiver(*k),
            Ty::TestPage(_) => "TestPage",
            Ty::TestRequestPage(_) => "TestRequestPage",
            Ty::TestPart(_) => "TestPart",
            Ty::TestFilter(_) => "TestFilter",
            Ty::List(_) => "List",
            Ty::Dictionary(..) => "Dictionary",
            Ty::Variant => "Variant",
            Ty::Builtin(n) => return Some(n.clone()),
            _ => return None,
        }
        .to_string(),
    )
}

fn proc_cand(p: &ProcSig) -> Cand {
    Cand {
        min: p.params.len(),
        max: p.params.len(),
        ret: p.ret.clone().unwrap_or(Ty::Unknown),
        target: Res::Proc {
            object: p.declared_in,
            offset: p.offset,
        },
        param_tys: p.params.iter().map(|x| x.ty.to_string()).collect(),
        fields: Vec::new(),
        rest_field: false,
    }
}

/// Whether an argument of type `arg` fits a parameter declared as `param`
/// (catalog spelling or `Ty` display). Only used to break arity ties.
fn fits(arg: &Ty, param: &str) -> bool {
    let p = param.to_ascii_lowercase();
    let p = p.split(['[', ' ']).next().unwrap_or("");
    match arg {
        Ty::Unknown => true,
        Ty::Integer | Ty::BigInteger | Ty::Decimal | Ty::Byte | Ty::Char => {
            matches!(
                p,
                "integer" | "biginteger" | "decimal" | "byte" | "char" | "any" | "joker" | "variant"
            )
        }
        Ty::Text(_) | Ty::Code(_) | Ty::Label => {
            matches!(p, "text" | "code" | "string" | "any" | "joker" | "variant" | "label")
        }
        Ty::Boolean => matches!(p, "boolean" | "any" | "joker" | "variant"),
        other => p == "any" || p == "joker" || p == "variant" || other.to_string().to_ascii_lowercase().starts_with(p),
    }
}

fn is_field_param(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("field") || n == "fromfield" || n.starts_with("column")
}

fn sig_cand(s: &Signature, recv: &Ty, id: String) -> Cand {
    let min = s.params.iter().filter(|p| !p.optional && !p.variadic).count()
        + s.params.iter().filter(|p| !p.optional && p.variadic).count();
    let max = if s.params.iter().any(|p| p.variadic) {
        usize::MAX
    } else {
        s.params.len()
    };
    let fields = s.params.iter().map(|p| is_field_param(&p.name)).collect();
    let rest_field = s.params.iter().any(|p| p.variadic && is_field_param(&p.name));
    Cand {
        target: Res::Builtin(id),
        param_tys: s.params.iter().map(|p| p.ty.clone()).collect(),
        min,
        max,
        ret: catalog_ty(&s.ret, recv),
        fields,
        rest_field,
    }
}

/// A catalog type name as a `Ty`, with `T`/`TKey`/`TValue` bound from the receiver.
fn catalog_ty(s: &str, recv: &Ty) -> Ty {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("Any") || s.eq_ignore_ascii_case("Joker") {
        return Ty::Unknown;
    }
    // Catalog spellings repeat on every call site of a method; parse each once.
    thread_local! {
        static PARSED: std::cell::RefCell<HashMap<String, Option<Ty>>> = Default::default();
    }
    let parsed = PARSED.with(|c| {
        c.borrow_mut()
            .entry(s.to_string())
            .or_insert_with(|| al_syntax::parse_type(s).map(|tr| Ty::from_type_ref(&tr)))
            .clone()
    });
    match parsed {
        Some(t) => subst(t, recv),
        None => Ty::Unknown,
    }
}

fn subst(t: Ty, recv: &Ty) -> Ty {
    match (&t, recv) {
        (Ty::Builtin(n), Ty::List(el)) if n == "T" => (**el).clone(),
        (Ty::Builtin(n), Ty::Dictionary(k, _)) if n == "TKey" => (**k).clone(),
        (Ty::Builtin(n), Ty::Dictionary(_, v)) if n == "TValue" => (**v).clone(),
        (Ty::List(el), _) => Ty::List(Box::new(subst((**el).clone(), recv))),
        (Ty::Dictionary(k, v), _) => Ty::Dictionary(
            Box::new(subst((**k).clone(), recv)),
            Box::new(subst((**v).clone(), recv)),
        ),
        _ => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;

    fn ws(files: &[&str]) -> Workspace {
        Workspace::load(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: files
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (format!("f{i}.al"), s.to_string()))
                    .collect(),
            }],
            &Default::default(),
        )
    }

    fn run(files: &[&str]) -> (Workspace, CheckResult) {
        let w = ws(files);
        assert!(w.diagnostics.is_empty(), "load diagnostics: {:?}", w.diagnostics);
        let m = crate::members::build(&w);
        let r = check(&w, &m, &Catalog::load());
        (w, r)
    }

    fn diags(files: &[&str]) -> Vec<String> {
        run(files)
            .1
            .diagnostics
            .iter()
            .map(|d| format!("{} {}", d.code, d.message))
            .collect()
    }

    /// Type of the expression in the first `exit(...)` of the last file.
    fn ty_of(files: &[&str]) -> Ty {
        let (w, r) = run(files);
        let fi = w.files.len() - 1;
        let root = al_syntax::SyntaxNode::new_root(w.files[fi].green.clone());
        let exit = root
            .descendants()
            .find(|n| n.kind() == al_syntax::SyntaxKind::ExitStmt)
            .expect("an exit");
        let e = exit.children().next().expect("exit has an expression");
        let range = e.text_range();
        let key = (fi, usize::from(range.start()), usize::from(range.end()));
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        r.types
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("no type for `{}`", e.text()))
    }

    fn cu(body: &str) -> String {
        format!("codeunit 50100 C {{ {body} }}")
    }

    const TABLE: &str = "table 50900 \"ALT Universal\" { fields { field(1; \"No.\"; Code[20]) { } field(2; Amount; Decimal) { } field(3; Status; Option Open,Closed) { } } procedure Double(X: Integer): Integer begin exit(X * 2); end; }";

    #[test]
    fn literals() {
        assert_eq!(ty_of(&[&cu("procedure P(): Integer begin exit(1); end;")]), Ty::Integer);
        assert_eq!(
            ty_of(&[&cu("procedure P(): Decimal begin exit(1.5); end;")]),
            Ty::Decimal
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Text begin exit('ab'); end;")]),
            Ty::Text(None)
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Boolean begin exit(true); end;")]),
            Ty::Boolean
        );
        assert_eq!(ty_of(&[&cu("procedure P(): Date begin exit(0D); end;")]), Ty::Date);
    }

    #[test]
    fn locals_params_and_globals() {
        assert_eq!(
            ty_of(&[&cu("procedure P(A: Decimal): Decimal begin exit(A); end;")]),
            Ty::Decimal
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Code[10] var L: Code[10]; begin exit(L); end;")]),
            Ty::Code(Some(10))
        );
        assert_eq!(
            ty_of(&[&cu("var G: Guid; procedure P(): Guid begin exit(G); end;")]),
            Ty::Guid
        );
    }

    #[test]
    fn names_are_case_insensitive() {
        assert_eq!(
            ty_of(&[&cu(
                "procedure P(): Decimal var MyVar: Decimal; begin exit(myvar); end;"
            )]),
            Ty::Decimal
        );
    }

    #[test]
    fn record_fields_and_table_procedures() {
        let src = cu("procedure P(): Decimal var R: Record \"ALT Universal\"; begin exit(R.Amount); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Decimal);
        let src = cu("procedure P(): Integer var R: Record \"ALT Universal\"; begin exit(R.Double(2)); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Integer);
    }

    #[test]
    fn record_builtin_methods_come_from_the_catalog() {
        let src = cu("procedure P(): Boolean var R: Record \"ALT Universal\"; begin exit(R.Insert(true)); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Boolean);
        let src = cu("procedure P(): Integer var R: Record \"ALT Universal\"; begin exit(R.Count()); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Integer);
    }

    #[test]
    fn global_builtins_and_own_procedures() {
        assert_eq!(
            ty_of(&[&cu("procedure P(): Text begin exit(Format(1)); end;")]),
            Ty::Text(None)
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Integer begin exit(StrLen('x')); end;")]),
            Ty::Integer
        );
        assert_eq!(ty_of(&[&cu("procedure P(): Date begin exit(Today()); end;")]), Ty::Date);
        assert_eq!(
            ty_of(&[&cu(
                "procedure Q(): Guid begin end; procedure P(): Guid begin exit(Q()); end;"
            )]),
            Ty::Guid
        );
    }

    #[test]
    fn parameterless_calls_without_parentheses() {
        assert_eq!(ty_of(&[&cu("procedure P(): Date begin exit(Today); end;")]), Ty::Date);
        let src = cu("procedure P(): Integer var R: Record \"ALT Universal\"; begin exit(R.Count); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Integer);
    }

    #[test]
    fn codeunit_variable_procedures() {
        let other = "codeunit 50101 Other { procedure Get(): Text begin end; }";
        assert_eq!(
            ty_of(&[
                other,
                &cu("procedure P(): Text var O: Codeunit Other; begin exit(O.Get()); end;")
            ]),
            Ty::Text(None)
        );
    }

    #[test]
    fn library_type_methods_and_generics() {
        assert_eq!(
            ty_of(&[&cu(
                "procedure P(): Integer var L: List of [Decimal]; begin exit(L.Count()); end;"
            )]),
            Ty::Integer
        );
        assert_eq!(
            ty_of(&[&cu(
                "procedure P(): Decimal var L: List of [Decimal]; begin exit(L.Get(1)); end;"
            )]),
            Ty::Decimal
        );
        assert_eq!(
            ty_of(&[&cu(
                "procedure P(): Boolean var J: JsonObject; begin exit(J.Contains('a')); end;"
            )]),
            Ty::Boolean
        );
        assert_eq!(
            ty_of(&[&cu(
                "procedure P(): Integer var T: Text; begin exit(T.IndexOf('a')); end;"
            )]),
            Ty::Integer
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            ty_of(&[&cu("procedure P(): Boolean begin exit(1 < 2); end;")]),
            Ty::Boolean
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Decimal begin exit(1 + 1.5); end;")]),
            Ty::Decimal
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Text begin exit('a' + 'b'); end;")]),
            Ty::Text(None)
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Boolean begin exit(not true); end;")]),
            Ty::Boolean
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Boolean begin exit(1 in [1, 2..3]); end;")]),
            Ty::Boolean
        );
    }

    #[test]
    fn implicit_rec_in_table_code() {
        let t = "table 50900 T { fields { field(1; Amount; Decimal) { } } procedure P(): Decimal begin exit(Amount); end; }";
        assert_eq!(ty_of(&[t]), Ty::Decimal);
        let t = "table 50900 T { fields { field(1; Amount; Decimal) { } } procedure P(): Decimal begin exit(Rec.Amount); end; }";
        assert_eq!(ty_of(&[t]), Ty::Decimal);
        let t =
            "table 50900 T { fields { field(1; A; Integer) { } } procedure P(): Integer begin exit(Count()); end; }";
        assert_eq!(ty_of(&[t]), Ty::Integer);
    }

    #[test]
    fn enum_and_option_values() {
        let e = "enum 50900 \"ALT Status\" { value(0; Open) { } }";
        assert_eq!(
            ty_of(&[
                e,
                &cu("procedure P(): Enum \"ALT Status\" begin exit(Enum::\"ALT Status\"::Open); end;")
            ]),
            Ty::Enum("ALT Status".into())
        );
        assert_eq!(
            ty_of(&[
                e,
                &cu("procedure P(): Enum \"ALT Status\" begin exit(\"ALT Status\"::Open); end;")
            ]),
            Ty::Enum("ALT Status".into())
        );
        let src =
            cu("procedure P(): Boolean var R: Record \"ALT Universal\"; begin exit(R.Status = R.Status::Closed); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Boolean);
    }

    #[test]
    fn object_id_scope() {
        assert_eq!(
            ty_of(&[
                TABLE,
                &cu("procedure P(): Integer begin exit(Database::\"ALT Universal\"); end;")
            ]),
            Ty::Integer
        );
        assert_eq!(
            ty_of(&[&cu("procedure P(): Integer begin exit(Codeunit::C); end;")]),
            Ty::Integer
        );
    }

    #[test]
    fn unresolved_name_member_and_call_are_diagnosed() {
        assert_eq!(
            diags(&[&cu("procedure P() begin Nope := 1; end;")]),
            vec!["AR1000 unresolved name `Nope`"]
        );
        assert_eq!(
            diags(&[
                TABLE,
                &cu("procedure P() var R: Record \"ALT Universal\"; begin R.Nope := 1; end;")
            ]),
            vec!["AR1002 no member `Nope` on Record \"ALT Universal\""]
        );
        assert_eq!(
            diags(&[&cu("procedure P() begin Format(); end;")]),
            vec!["AR1003 no overload of `Format` takes 0 argument(s)"]
        );
    }

    #[test]
    fn unknown_types_do_not_cascade() {
        // `Customer` is not in the workspace: AR0002 at load, nothing more here.
        let w = ws(&[&cu(
            "procedure P() var C: Record Customer; begin C.Name := 'x'; C.Foo(); end;",
        )]);
        let m = crate::members::build(&w);
        assert!(check(&w, &m, &Catalog::load()).diagnostics.is_empty());
    }

    #[test]
    fn field_arguments_resolve_against_the_receiver_table() {
        let src = cu(
            "procedure P() var R: Record \"ALT Universal\"; begin R.SetRange(\"No.\", 'A'); R.CalcFields(Amount, Status); R.SetCurrentKey(Amount); R.TestField(\"No.\"); end;",
        );
        assert_eq!(diags(&[TABLE, &src]), Vec::<String>::new());
        let src = cu("procedure P(): Integer var R: Record \"ALT Universal\"; begin exit(R.FieldNo(Amount)); end;");
        assert_eq!(ty_of(&[TABLE, &src]), Ty::Integer);
    }

    #[test]
    fn field_positions_do_not_capture_value_arguments() {
        // `Amount` here is the filter VALUE: it must resolve as a variable, so an
        // undeclared one is still an error.
        let src = cu("procedure P() var R: Record \"ALT Universal\"; begin R.SetRange(\"No.\", Nope); end;");
        assert_eq!(diags(&[TABLE, &src]), vec!["AR1000 unresolved name `Nope`"]);
    }

    const PAGES: &str = "page 50900 \"ALT List\" { SourceTable = \"ALT Universal\"; layout { area(Content) { field(\"No.\"; Rec.\"No.\") { } part(Lines; \"ALT Sub\") { } } } actions { area(Processing) { action(Go) { } } } }\npage 50901 \"ALT Sub\" { SourceTable = \"ALT Universal\"; layout { area(Content) { field(Amt; Rec.Amount) { } } } }";

    #[test]
    fn test_page_controls_parts_and_filter() {
        let src = cu(
            "procedure P() var TP: TestPage \"ALT List\"; begin TP.\"No.\".SetValue('x'); TP.Go.Invoke(); TP.Lines.Amt.SetValue(1); TP.Filter.SetFilter(Amount, '1'); end;",
        );
        assert_eq!(diags(&[TABLE, PAGES, &src]), Vec::<String>::new());
    }

    #[test]
    fn field_arguments_on_an_unresolved_table_are_not_reported() {
        // `Date` is a platform table not in the workspace (AR0002 at load).
        let w = ws(&[&cu(
            "procedure P() var D: Record Date; begin D.SetRange(\"Period Type\", 1); end;",
        )]);
        let m = crate::members::build(&w);
        assert!(check(&w, &m, &Catalog::load()).diagnostics.is_empty());
    }

    #[test]
    fn builtin_test_page_methods_win_over_controls_when_called() {
        let page = "page 50902 \"ALT Modal\" { layout { area(Content) { field(OK; X) { } } } }";
        let src = cu("procedure P() var TP: TestPage \"ALT Modal\"; begin TP.OK().Invoke(); end;");
        assert_eq!(diags(&[page, &src]), Vec::<String>::new());
    }

    #[test]
    fn curr_field_no_in_table_code() {
        let t =
            "table 50900 T { fields { field(1; A; Integer) { trigger OnValidate() begin A := CurrFieldNo; end; } } }";
        assert_eq!(diags(&[t]), Vec::<String>::new());
    }

    #[test]
    fn xmlport_text_elements_are_variables() {
        let x = "xmlport 50900 X { schema { textelement(Root) { textattribute(NoteAttr) { trigger OnBeforePassVariable() begin NoteAttr := UpperCase(NoteAttr); end; } } } }";
        assert_eq!(diags(&[x]), Vec::<String>::new());
    }

    #[test]
    fn page_extension_controls_are_test_page_members() {
        let ext = "pageextension 50903 E extends \"ALT List\" { layout { addlast(Content) { field(ExtCtl; Rec.Amount) { } } } }";
        let src = cu(
            "procedure P() var TP: TestPage \"ALT List\"; begin TP.ExtCtl.SetValue(1); TP.Lines.Filter.SetFilter(Amount, '1'); end;",
        );
        assert_eq!(diags(&[TABLE, PAGES, ext, &src]), Vec::<String>::new());
    }

    #[test]
    fn unresolved_enum_type_before_scope_is_an_external_reference() {
        assert_eq!(
            diags(&[&cu(
                "procedure P() var X: Integer; begin X := \"Reservation From Stock\"::\" \"; end;"
            )]),
            vec!["AR0002 unresolved enum `Reservation From Stock`"]
        );
    }

    #[test]
    fn namespace_qualified_object_after_scope() {
        let t = "namespace A.B;\ntable 50910 \"Ship\" { }";
        assert_eq!(
            ty_of(&[
                t,
                &cu("procedure P(): Integer begin exit(Database::A.B.\"Ship\"); end;")
            ]),
            Ty::Integer
        );
        assert_eq!(
            diags(&[
                t,
                &cu("procedure P() var R: RecordRef; begin R.Open(Database::X.Y.\"Ship\"); end;")
            ]),
            vec!["AR0002 unresolved table `X.Y.Ship`"]
        );
    }

    #[test]
    fn call_syntax_prefers_functions_over_fields_of_the_same_name() {
        let t = "table 50911 U { fields { field(1; UserSecurityId; Guid) { } } procedure P() begin UserSecurityId := UserSecurityId(); end; }";
        assert_eq!(diags(&[t]), Vec::<String>::new());
    }

    const REPORT: &str = "report 50920 \"ALT Rep\" { dataset { dataitem(Rows; \"ALT Universal\") { trigger OnAfterGetRecord() begin Amount := Amount + 1; end; } } }";

    #[test]
    fn data_item_triggers_see_the_data_item_record_unqualified() {
        assert_eq!(diags(&[TABLE, REPORT]), Vec::<String>::new());
    }

    #[test]
    fn report_extensions_see_base_data_items() {
        let ext = "reportextension 50921 E extends \"ALT Rep\" { dataset { modify(Rows) { trigger OnAfterAfterGetRecord() begin Rows.Amount := Amount; end; } } }";
        assert_eq!(diags(&[TABLE, REPORT, ext]), Vec::<String>::new());
    }

    #[test]
    fn test_request_page_data_item_is_a_filter_on_its_table() {
        let src = cu("procedure P(RP: TestRequestPage \"ALT Rep\") begin RP.Rows.SetFilter(Amount, '1'); end;");
        assert_eq!(diags(&[TABLE, REPORT, &src]), Vec::<String>::new());
    }

    #[test]
    fn table_extension_of_an_external_table_still_has_rec() {
        let w = ws(&[
            "tableextension 50930 E extends Job { fields { field(50000; F; Integer) { trigger OnValidate() begin Rec.F := CurrFieldNo; end; } } }",
        ]);
        let m = crate::members::build(&w);
        assert!(check(&w, &m, &Catalog::load()).diagnostics.is_empty());
    }

    /// Resolution of `sub`, located inside the first occurrence of `context`.
    fn res_in(files: &[&str], context: &str, sub: &str) -> Res {
        let (w, r) = run(files);
        let fi = w.files.len() - 1;
        let src = al_syntax::SyntaxNode::new_root(w.files[fi].green.clone())
            .text()
            .to_string();
        let start = src.find(context).unwrap_or_else(|| panic!("`{context}` not in source"))
            + context.find(sub).expect("sub in context");
        let key = (fi, start, start + sub.len());
        r.resolutions.get(&key).cloned().unwrap_or_else(|| {
            panic!(
                "no resolution for `{sub}`; have {:?}",
                r.resolutions.keys().filter(|k| k.0 == fi).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn resolutions_for_variables() {
        assert_eq!(
            res_in(
                &[&cu("var G: Integer; procedure P() var L: Integer; begin L := G; end;")],
                "L := G",
                "G"
            ),
            Res::Global("g".into())
        );
    }

    #[test]
    fn resolutions_for_locals_and_params() {
        let src = cu("procedure P(A: Integer) var L: Integer; begin L := A; end;");
        assert_eq!(res_in(&[&src], "L := A", "L"), Res::Local("l".into()));
        assert_eq!(res_in(&[&src], "L := A", "A"), Res::Local("a".into()));
    }

    #[test]
    fn resolutions_for_calls_pick_the_overload() {
        let r = res_in(
            &[&cu("procedure P() var T: Text; begin T := Format(1, 0, 2); end;")],
            "Format(1, 0, 2)",
            "Format(1, 0, 2)",
        );
        assert_eq!(r, Res::Builtin("System.Format#1".into()));
        let r = res_in(
            &[&cu(
                "procedure P() var T: Text; begin T := Format(1, 0, '<Integer>'); end;",
            )],
            "Format(1, 0, '<Integer>')",
            "Format(1, 0, '<Integer>')",
        );
        assert_eq!(r, Res::Builtin("System.Format#2".into()));
    }

    #[test]
    fn resolutions_for_procedures() {
        let src = cu("procedure Q(): Integer begin end; procedure P() var I: Integer; begin I := Q(); end;");
        let Res::Proc { object, offset } = res_in(&[&src], "I := Q()", "Q()") else {
            panic!()
        };
        assert_eq!(object, 0);
        assert_eq!(offset, src.find("procedure Q").unwrap());
    }

    #[test]
    fn resolutions_for_enum_and_option_values() {
        let e = "enum 50900 \"ALT Status\" { value(0; Open) { } value(5; Closed) { } }";
        let r = res_in(
            &[
                e,
                &cu("procedure P(): Enum \"ALT Status\" begin exit(\"ALT Status\"::Closed); end;"),
            ],
            "exit(\"ALT Status\"::Closed",
            "\"ALT Status\"::Closed",
        );
        assert!(matches!(r, Res::EnumValue { value: 5, .. }), "{r:?}");
        let src = cu(
            "procedure P() var R: Record \"ALT Universal\"; B: Boolean; begin B := R.Status = R.Status::Closed; end;",
        );
        assert_eq!(
            res_in(&[TABLE, &src], "= R.Status::Closed", "R.Status::Closed"),
            Res::OptionValue(1)
        );
    }

    #[test]
    fn statements_are_walked() {
        assert_eq!(
            diags(&[&cu(
                "procedure P() var i: Integer; begin if X then; for i := 1 to Y do; case Z of 1: ; end; repeat until W; end;"
            )]),
            vec![
                "AR1000 unresolved name `X`",
                "AR1000 unresolved name `Y`",
                "AR1000 unresolved name `Z`",
                "AR1000 unresolved name `W`"
            ]
        );
    }
}
