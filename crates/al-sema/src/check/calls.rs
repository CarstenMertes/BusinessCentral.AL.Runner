//! Overload picking and the lookup of callables.
use super::*;

impl<'a> Checker<'a> {
    /// Chooses an overload: by arity, then by how many argument types fit the
    /// declared parameter types (first in catalog order on a tie). Records the
    /// choice on `node`.
    pub(crate) fn pick(&mut self, sc: &Scope, node: &SyntaxNode, cands: &[Cand], args: &[Ty], name: &str) -> Ty {
        let argc = args.len();
        // The best arity match by how many arguments fit; the first wins a tie.
        let best_of = |hidden: bool| {
            let mut best: Option<(&Cand, usize)> = None;
            for c in cands
                .iter()
                .filter(|c| c.hidden.is_some() == hidden && c.min <= argc && argc <= c.max)
            {
                // How many arguments fit decides; among equals, the overload whose parameter types are the arguments'
                // own types wins over one that only takes them as a Variant (Reservation Management has
                // `SetReservSource(Variant)` and `SetReservSource(RecordRef)`; passing a RecordRef picks the latter).
                let fit = args.iter().zip(&c.param_tys).filter(|(a, p)| fits(a, p)).count();
                // Only among the procedures AL code declares: the catalog's overloads keep their order.
                let exact = if matches!(c.target, Res::Proc { .. }) {
                    args.iter()
                        .zip(&c.param_tys)
                        .filter(|(a, p)| !matches!(a, Ty::Unknown) && a.to_string().eq_ignore_ascii_case(p))
                        .count()
                } else {
                    0
                };
                let score = fit * 1000 + exact;
                if best.is_none_or(|(_, s)| score > s) {
                    best = Some((c, score));
                }
            }
            best
        };
        let (best, newer) = (best_of(false), best_of(true));
        // An overload of a newer runtime that fits strictly better than every visible one
        // is what the author meant: BC's compiler refuses the call (ADR-015 3.3 point 4).
        if let Some((h, hs)) = newer {
            if best.is_none_or(|(_, s)| hs > s) {
                let since = h.hidden.expect("a hidden candidate has its runtime");
                let at = self.catalog.runtime().unwrap_or(since);
                self.diag(
                    sc,
                    node,
                    "AR1005",
                    format!("`{name}` is not available in runtime {at} (introduced in {since})"),
                );
                return Ty::Unknown;
            }
        }
        match best {
            Some((c, _)) => {
                let (ret, target, with) = (c.ret.clone(), c.target.clone(), c.with_off);
                self.record_with(sc, node, target, with);
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

    /// Procedures and functions named `n`, skipping variables and fields: a
    /// call `UserSecurityId()` means the function even in a table with a
    /// field of that name.
    pub(crate) fn lookup_callable(&mut self, sc: &mut Scope, n: &str) -> Option<Vec<Cand>> {
        // The record of a `with` statement first (corpus events/lt-with-statement-scopes/WithStatementAmbiguous.al:
        // P195_With_ProcedureSameNameAsTableProcedure, P195_With_BuiltInMethodSameNameAsCodeunitProcedure).
        for (t, w) in sc.withs.clone().iter().rev() {
            if let Some(E::Callable(mut cs)) = self.member(sc, t, n) {
                cs.iter_mut().for_each(|c| c.with_off = Some(*w));
                return Some(cs);
            }
        }
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
                // `Modify()` written without `Rec.` in table, page or report code writes to the
                // implicit record like any other write.
                if !c.is_empty() {
                    if let Ty::Record { table, .. } = &rec {
                        if let Some(ti) = self.resolve_quiet(sc.file, ObjKind::Table, table) {
                            self.touch_ops(ti, demand::record_ops(n));
                        }
                    }
                }
            }
        }
        // A procedure of the extension next to procedures of its base with the same name: the base's
        // are overloads of it (corpus pageext/POPageExtOverload.al on corpus branch al-runtime-rs-staging:
        // `Describe(5)` calls the base's Integer procedure, `Describe('t')` the extension's Text one).
        let base = self.base_proc_cands(sc, n);
        if c.is_empty() {
            c = base;
        } else {
            // The same parameter types is AL0440 (the lowering refuses it): the extension's own
            // procedure stays the candidate then.
            let mut all: Vec<Cand> = base
                .into_iter()
                .filter(|b| !c.iter().any(|o| o.param_tys == b.param_tys))
                .collect();
            all.extend(c);
            c = all;
        }
        if c.is_empty() {
            c = self.global_cands(n);
        }
        (!c.is_empty()).then_some(c)
    }

    /// The resolution of global `key`: the base object's variable in an extension that sees it
    /// (ADR-023), else the object's own.
    pub(crate) fn global_res(sc: &Scope, key: String) -> Res {
        if sc.base_vars.contains(&key) {
            Res::BaseGlobal(key)
        } else {
            Res::Global(key)
        }
    }

    /// The procedures of the page or report an extension extends that its code may call (ADR-023).
    pub(crate) fn base_proc_cands(&self, sc: &Scope, n: &str) -> Vec<Cand> {
        let Some(b) = sc.base_obj else { return Vec::new() };
        let ast = self.ws.object_ast(b);
        let visible: Vec<usize> = crate::base_access::visible_procs(&ast, n)
            .map(|r| usize::from(r.syntax().text_range().start()))
            .collect();
        self.members[b]
            .procedures_named(n)
            .iter()
            .filter(|p| visible.contains(&p.offset))
            .map(|p| proc_cand(p))
            .collect()
    }

    /// `this.Name` in a codeunit: a global variable, else a procedure of the codeunit. The
    /// resolution is recorded on the member expression, as a member access would.
    pub(crate) fn this_member(&mut self, sc: &mut Scope, node: &SyntaxNode, member: &SyntaxNode) -> Option<E> {
        let n = name_of(member);
        let key = n.to_lowercase();
        if let Some(t) = sc.globals.get(&key).cloned() {
            let res = if sc.base_vars.contains(&key) {
                Res::ThisBaseGlobal(key)
            } else {
                Self::global_res(sc, key)
            };
            self.record(sc, node, res);
            return Some(E::Val(t));
        }
        // `this.Id` in a query's own code: the running query's column (corpus query/PRQueryOpenColumn.al `PeekThis`).
        if let Some(rec) = sc.rec.clone() {
            if matches!(rec, Ty::Object(ObjKind::Query, _)) && sc.self_obj == sc.oi {
                if let Some(r) = self.field_res(sc, &rec, &n) {
                    if let Some(e) = self.member(sc, &rec, &n) {
                        self.record(sc, node, r);
                        return Some(e);
                    }
                }
            }
        }
        let own: Vec<Cand> = self.members[sc.self_obj]
            .procedures_named(&n)
            .iter()
            .map(|p| proc_cand(p))
            .collect();
        // `this.BaseVar` where the base report's or page's variable is not protected, or `this.LocalProc()` where the
        // procedure is local: AL0161, as for the bare name (corpus must-not-compile 0122).
        if own.is_empty()
            && !self.ws.is_dependency(sc.oi)
            && matches!(
                self.ws.objects[sc.oi].kind,
                ObjKind::ReportExtension | ObjKind::PageExtension
            )
            && let Some(b) = self.ws.extension_target(sc.oi)
            && self.base_proc_cands(sc, &n).is_empty()
        {
            let base = self.ws.object_ast(b);
            let is_var = crate::base_access::declares_var(&base, &n);
            let is_local_proc = base
                .routines()
                .any(|r| r.name().eq_ignore_ascii_case(&n) && r.is_local());
            if is_var || is_local_proc {
                let shown = if is_var { n.clone() } else { format!("{n}()") };
                self.diag(
                    sc,
                    member,
                    "AL0161",
                    format!("'{shown}' is inaccessible due to its protection level"),
                );
                return Some(E::Val(Ty::Unknown));
            }
        }
        // In an extension `this` is the base object, and both the base's procedures and the extension's own are
        // reachable through it (types/PYThisExtensions.al: this.BaseProc, this.ExtProc, this.BasePageProc, this.ExtPageProc).
        if sc.self_obj != sc.oi || sc.base_obj.is_some() {
            let own_obj: Vec<Cand> = if sc.self_obj != sc.oi {
                self.members[sc.oi]
                    .procedures_named(&n)
                    .iter()
                    .map(|p| proc_cand(p))
                    .collect()
            } else {
                Vec::new()
            };
            let base = self.base_proc_cands(sc, &n);
            let mut all: Vec<Cand> = base
                .into_iter()
                .filter(|b| !own.iter().chain(&own_obj).any(|o| o.param_tys == b.param_tys))
                .collect();
            all.extend(own);
            all.extend(own_obj);
            return (!all.is_empty()).then_some(E::Callable(all));
        }
        (!own.is_empty()).then_some(E::Callable(own))
    }

    /// Name lookup without reporting: None if the name does not resolve.
    pub(crate) fn lookup_name(&mut self, sc: &mut Scope, node: &SyntaxNode) -> Option<E> {
        // One copy of the name for the common case (a variable); the spelling as written only when it is not one.
        if !sc.withs.is_empty() {
            let n = name_of(node);
            for (t, w) in sc.withs.clone().iter().rev() {
                let Some(mut e) = self.member(sc, t, &n) else { continue };
                if let Some(r) = self.field_res(sc, t, &n) {
                    self.record_with(sc, node, r, Some(*w));
                }
                if let E::Callable(cs) = &mut e {
                    cs.iter_mut().for_each(|c| c.with_off = Some(*w));
                }
                return Some(e);
            }
        }
        let mut key = name_of(node);
        if key.is_ascii() {
            key.make_ascii_lowercase();
        } else {
            key = key.to_lowercase();
        }
        if let Some(t) = sc.vars.iter().rev().find_map(|frame| frame.get(&key)) {
            let t = t.clone();
            self.record(sc, node, Res::Local(key));
            return Some(E::Val(t));
        }
        if let Some(t) = sc.globals.get(&key) {
            let t = t.clone();
            let res = Self::global_res(sc, key);
            self.record(sc, node, res);
            return Some(E::Val(t));
        }
        let n = name_of(node);
        // `this` is the running instance of a codeunit (`this.EmitLengthWarning(..)`).
        if key == "this" && self.ws.objects[sc.oi].kind == ObjKind::Codeunit && sc.self_obj == sc.oi {
            return Some(E::Val(Ty::Object(
                ObjKind::Codeunit,
                self.ws.objects[sc.oi].name.as_str().into(),
            )));
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
        let base = self.base_proc_cands(sc, &n);
        if !own.is_empty() || !own_obj.is_empty() {
            let own: Vec<Cand> = own.into_iter().chain(own_obj).collect();
            let mut all: Vec<Cand> = base
                .into_iter()
                .filter(|b| !own.iter().any(|o| o.param_tys == b.param_tys))
                .collect();
            all.extend(own);
            return Some(E::Callable(all));
        }
        if !base.is_empty() {
            return Some(E::Callable(base));
        }
        let globals: Vec<Cand> = self.global_cands(&n);
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
            return Some(E::TypeName(Ty::Enum(self.ws.objects[i].name.as_str().into())));
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
                al_builtins::value::PROMPT_MODE_MEMBERS
                    .iter()
                    .map(|m| m.to_string())
                    .collect(),
            )));
        }
        if key == "textencoding" {
            return Some(E::TypeName(Ty::Option(crate::types::text_encoding_members())));
        }
        if self.system_enums.contains(&key) {
            return Some(E::TypeName(Ty::Builtin(n.into())));
        }
        // Library types used statically: `JsonObject`, `NavApp`, `TaskScheduler`, ...
        if self
            .catalog
            .signatures
            .iter()
            .any(|s| s.receiver.eq_ignore_ascii_case(&n))
        {
            return Some(E::TypeName(Ty::Builtin(n.into())));
        }
        None
    }

    /// Static methods reachable through a type name.
    pub(crate) fn static_members(&self, t: &Ty, name: &str) -> Option<Vec<Cand>> {
        let recv = receiver_of(t)?;
        let c = self.catalog_cands(&recv, name, false, t);
        (!c.is_empty()).then_some(c)
    }

    /// Visible overloads first, then the ones only a newer runtime has (marked `hidden`).
    pub(crate) fn catalog_cands(&self, receiver: &str, name: &str, instance_only: bool, recv_ty: &Ty) -> Vec<Cand> {
        let keep = |s: &&Signature| !instance_only || s.instance;
        let visible = self.catalog.overloads(receiver, name).into_iter().filter(keep);
        let hidden = self.catalog.hidden(receiver, name).into_iter().filter(keep);
        visible
            .map(|s| sig_cand(s, recv_ty, self.catalog.id(s)))
            .chain(hidden.map(|s| hidden_cand(s, recv_ty, self.catalog.id(s))))
            .collect()
    }

    /// Unqualified builtins (`Message`, `Commit`): visible, then hidden.
    pub(crate) fn global_cands(&self, name: &str) -> Vec<Cand> {
        self.catalog
            .global(name)
            .iter()
            .map(|s| sig_cand(s, &Ty::Unknown, self.catalog.id(s)))
            .chain(
                self.catalog
                    .hidden_global(name)
                    .iter()
                    .map(|s| hidden_cand(s, &Ty::Unknown, self.catalog.id(s))),
            )
            .collect()
    }
}
