//! The checker's object context, name resolution, and the walk over an object's routines.
use super::*;

impl<'a> Checker<'a> {
    pub(crate) fn new(
        ws: &'a Workspace,
        members: &'a MemberTable<'a>,
        catalog: &'a Catalog,
        system_enums: HashSet<String>,
        demand_mode: bool,
    ) -> Checker<'a> {
        Checker {
            ws,
            members,
            catalog,
            diags: Vec::new(),
            types: Default::default(),
            resolutions: Default::default(),
            system_enums,
            sections: Default::default(),
            queries: (0..ws.objects.len()).map(|_| std::cell::OnceCell::new()).collect(),
            calls: Vec::new(),
            demand: std::cell::RefCell::new(demand_mode.then(|| demand::State::new(ws.objects.len()))),
            prefetch: None,
            resolved: Default::default(),
            panicked: HashMap::default(),
        }
    }

    /// Runs `f`; if it panics, the panic is recorded against `key` (object, routine offset) and the
    /// check goes on with the next piece of work. The checker's state after a panic only ever holds
    /// more than the routine reached (types, calls, reached objects), never less.
    pub(crate) fn guard(&mut self, key: (usize, usize), f: impl FnOnce(&mut Self)) {
        if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            self.panicked.insert(key, panic_message(&*p));
        }
    }

    pub(crate) fn diag(&mut self, sc: &Scope, node: &SyntaxNode, code: &'static str, message: String) {
        self.diags.push(Diagnostic {
            file: self.ws.files[sc.file].path.clone(),
            offset: node.text_range().start().into(),
            code,
            message,
        });
    }

    pub(crate) fn record(&mut self, sc: &Scope, node: &SyntaxNode, r: Res) {
        if let Res::Proc { object, offset } = &r {
            self.calls.push((*object, *offset));
        }
        let range = node.text_range();
        self.resolutions
            .insert((sc.file, range.start().into(), range.end().into()), r);
    }

    /// `Res::Field` if `name` is a field of the record type `t`.
    pub(crate) fn field_res(&self, sc: &Scope, t: &Ty, name: &str) -> Option<Res> {
        if let Ty::Object(ObjKind::Query, q) = t {
            let qi = self.resolve(sc, ObjKind::Query, q)?;
            let column = self.query(qi)?.column(name)?;
            return Some(Res::QueryColumn { query: qi, column });
        }
        let Ty::Record { table, .. } = t else { return None };
        let ti = self.resolve(sc, ObjKind::Table, table)?;
        self.members[ti].field(name).map(|f| Res::Field { table: ti, id: f.id })
    }

    pub(crate) fn resolve(&self, sc: &Scope, kind: ObjKind, name: &str) -> Option<usize> {
        self.resolve_at(sc.file, kind, name)
    }

    /// Resolves an object reference written in `file`. Whatever it finds is used by the code
    /// being checked, so a dependency object found this way is reached (see `demand`).
    pub(crate) fn resolve_at(&self, file: usize, kind: ObjKind, name: &str) -> Option<usize> {
        let found = self.resolve_quiet(file, kind, name)?;
        self.touch(found);
        Some(found)
    }

    /// `resolve_at` without marking the object as used.
    pub(crate) fn resolve_quiet(&self, file: usize, kind: ObjKind, name: &str) -> Option<usize> {
        if let Some(hit) = self.resolved.borrow().get(&(file, kind)).and_then(|m| m.get(name)) {
            return *hit;
        }
        let parts: Vec<String> = if name.parse::<u32>().is_ok() {
            vec![name.to_string()]
        } else {
            split_qualified(name)
        };
        let found = match self.ws.resolve_ref(file, kind, &parts) {
            Resolution::Found(i) => Some(i),
            _ => None,
        };
        self.resolved
            .borrow_mut()
            .entry((file, kind))
            .or_default()
            .insert(name.to_string(), found);
        found
    }

    /// Like `resolve_at`, for a reference already split into its parts.
    pub(crate) fn resolve_parts(&self, file: usize, kind: ObjKind, parts: &[String]) -> Option<usize> {
        match self.ws.resolve_ref(file, kind, parts) {
            Resolution::Found(i) => {
                self.touch(i);
                Some(i)
            }
            _ => None,
        }
    }

    /// The layout of query object `qi`.
    pub(crate) fn query(&self, qi: usize) -> Option<&crate::queries::QueryLayout> {
        self.queries[qi]
            .get_or_init(|| {
                (self.ws.objects[qi].kind == ObjKind::Query && self.ws.usable(qi))
                    .then(|| crate::queries::layout(self.ws, self.members, qi))
            })
            .as_ref()
    }

    /// What every routine of object `oi` sees: the object's own variables, the implicit
    /// record, and the data items of a report extension's base.
    pub(crate) fn object_ctx(&mut self, oi: usize) -> ObjCtx {
        let obj = self.ws.object_ast(oi);
        let o = &self.ws.objects[oi];
        let mut globals = super::posmap::FxMap::default();
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
                if let (Some(name), Some(table)) = (args.first(), crate::types::section_object_name(sec, 1)) {
                    globals.insert(
                        name.to_lowercase(),
                        Ty::Record {
                            table: table.into(),
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
                    table: o.name.as_str().into(),
                    temporary: false,
                });
                rec_unqualified = true;
            }
            ObjKind::TableExtension => {
                if let Some(base) = obj.extends_parts() {
                    if let Some(t) = self.resolve_parts(o.file, ObjKind::Table, &base) {
                        rec = Some(Ty::Record {
                            table: self.ws.objects[t].name.as_str().into(),
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
                    table: unquote(t).into(),
                    temporary: false,
                });
                rec_unqualified = true;
                globals.insert("currpage".into(), Ty::Object(ObjKind::Page, o.name.as_str().into()));
            }
            ObjKind::PageExtension => {
                if let Some(base) = obj.extends_parts() {
                    if let Some(p) = self.resolve_parts(o.file, ObjKind::Page, &base) {
                        let pobj = self.ws.object_ast(p);
                        rec = pobj
                            .properties()
                            .find(|x| x.name().eq_ignore_ascii_case("SourceTable"))
                            .map(|x| Ty::Record {
                                table: unquote(x.value_text()).into(),
                                temporary: false,
                            });
                        globals.insert(
                            "currpage".into(),
                            Ty::Object(ObjKind::Page, self.ws.objects[p].name.as_str().into()),
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
                    table: unquote(t).into(),
                    temporary: false,
                })
            }
            ObjKind::Report | ObjKind::ReportExtension => {
                // In a report extension `CurrReport` is the report run of the extended report, so its
                // members resolve against the base report, not against a report named like the extension
                // (Tests-ERM "Mfg. Document Entries", issue 19).
                let report = match (o.kind, obj.extends_parts()) {
                    (ObjKind::ReportExtension, Some(base)) => self
                        .resolve_parts(o.file, ObjKind::Report, &base)
                        .map_or_else(|| o.name.clone(), |r| self.ws.objects[r].name.clone()),
                    _ => o.name.clone(),
                };
                globals.insert("currreport".into(), Ty::Object(ObjKind::Report, report.into()));
                globals.insert("requestoptionspage".into(), Ty::Builtin("RequestPage".into()));
                // The request page of a report states a `SourceTable`: its triggers and controls name
                // that table's current record as `Rec` (handlers/TestReportRequestPageSourceTable
                // CompiledContracts.al). The checker only needs the name to resolve; running such a
                // report with a request page Rec is refused by the lowerer.
                if o.kind == ObjKind::Report {
                    let source = ast::sections_in(obj.syntax())
                        .into_iter()
                        .filter(|sec| sec.name().eq_ignore_ascii_case("requestpage"))
                        .find_map(|sec| {
                            sec.properties()
                                .find(|p| p.name().eq_ignore_ascii_case("SourceTable"))
                                .map(|p| unquote(p.value_text()))
                        });
                    if let Some(table) = source {
                        rec = Some(Ty::Record {
                            table: table.into(),
                            temporary: false,
                        });
                    }
                }
            }
            ObjKind::XmlPort => {
                globals.insert(
                    "currxmlport".into(),
                    Ty::Object(ObjKind::XmlPort, o.name.as_str().into()),
                );
            }
            // A query's procedures call the query's own methods unqualified and name its columns
            // and filters (query/TestQueryDataItemTableFilterDatabaseConst.al: `SetSourceFilter` of
            // Base App's query 522 does `SetRange(Source_Type, ..)`).
            ObjKind::Query => {
                rec = Some(Ty::Object(ObjKind::Query, o.name.as_str().into()));
                rec_unqualified = true;
                // `CurrQuery` is the running query in its triggers (query/TestQueryOnBeforeOpenVariablesContracts.al:75-77).
                globals.insert("currquery".into(), Ty::Object(ObjKind::Query, o.name.as_str().into()));
            }
            _ => {}
        }
        if let Some(r) = &rec {
            globals.insert("rec".into(), r.clone());
            globals.insert("xrec".into(), r.clone());
        }
        // Report extensions see the base report's data items; `modify(X)` blocks
        // run with data item X as their record.
        let mut base_items: HashMap<String, String> = HashMap::default();
        if o.kind == ObjKind::ReportExtension {
            if let Some(base) = obj.extends_parts() {
                if let Some(r) = self.resolve_parts(o.file, ObjKind::Report, &base) {
                    for sec in ast::sections_in(self.ws.object_ast(r).syntax()) {
                        if sec.name().eq_ignore_ascii_case("dataitem") {
                            let a = sec.args();
                            if let (Some(n), Some(t)) = (a.first(), crate::types::section_object_name(&sec, 1)) {
                                base_items.insert(n.to_lowercase(), t.clone());
                                globals.insert(
                                    n.to_lowercase(),
                                    Ty::Record {
                                        table: t.into(),
                                        temporary: false,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        // ADR-023: a page or report extension sees the base object's `protected var` variables
        // (those the extension does not declare itself) and its procedures.
        let mut base_obj = None;
        let mut base_vars = std::collections::HashSet::default();
        if crate::base_access::scope_enabled() {
            let base_kind = match o.kind {
                ObjKind::PageExtension => Some(ObjKind::Page),
                ObjKind::ReportExtension => Some(ObjKind::Report),
                _ => None,
            };
            if let (Some(kind), Some(parts)) = (base_kind, obj.extends_parts()) {
                if let Some(b) = self.resolve_parts(o.file, kind, &parts) {
                    base_obj = Some(b);
                    for (name, decl) in crate::base_access::visible_vars(&self.ws.object_ast(b)) {
                        if globals.contains_key(&name) {
                            continue;
                        }
                        let ty = decl.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
                        globals.insert(name.clone(), ty);
                        base_vars.insert(name);
                    }
                }
            }
        }
        let file = o.file;
        // Every type the object's variables name is used by the code that runs in it.
        let tys: Vec<Ty> = globals.values().cloned().chain(rec.clone()).collect();
        for t in &tys {
            self.touch_ty(file, t);
        }
        ObjCtx {
            oi,
            file,
            globals: std::rc::Rc::new(globals),
            rec,
            rec_unqualified,
            self_obj,
            base_items,
            base_obj,
            base_vars: std::rc::Rc::new(base_vars),
        }
    }

    pub(crate) fn check_object(&mut self, oi: usize) {
        let ctx = self.object_ctx(oi);
        let obj = self.ws.object_ast(oi);
        for r in ast::routines_in(obj.syntax()) {
            let offset = usize::from(r.syntax().text_range().start());
            if self.ws.routine_unreached(oi, offset) {
                continue;
            }
            self.guard((oi, offset), |c| c.check_routine_in(&ctx, &r));
        }
        // What the test client does when it opens a workspace page or report, and what its actions
        // run, reaches into the dependency apps.
        if matches!(
            self.ws.objects[oi].kind,
            ObjKind::Page | ObjKind::PageExtension | ObjKind::Report | ObjKind::ReportExtension
        ) {
            self.reach_caption_class_if_used(oi);
        }
        if matches!(self.ws.objects[oi].kind, ObjKind::Page | ObjKind::PageExtension) {
            self.reach_run_objects(oi);
            // The client writes to the page's table (validating a field, leaving a new row): the
            // table's triggers and events run, whichever app the table is in.
            if self.demand.borrow().is_some() {
                self.activate_source_table(oi);
            }
        }
    }

    /// The locals of routine `r` (by lower-case name) that are records of a platform table and are
    /// named in the body only as `Name.Field` with `Field` a field of that table: nothing is asked
    /// of the table, so the variable does not stand for its rows.
    pub(crate) fn field_only_records(
        &self,
        file: usize,
        r: &ast::Routine,
        locals: &super::posmap::FxMap<String, Ty>,
    ) -> HashSet<String> {
        let mut out = HashSet::default();
        let Some(body) = r.body() else { return out };
        for (name, t) in locals {
            let Ty::Record { table, .. } = t else { continue };
            let Some(ti) = self.resolve_quiet(file, ObjKind::Table, table) else {
                continue;
            };
            if !self.ws.platform_table(ti) {
                continue;
            }
            let only_fields = body
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::NameRef && name_of(n).eq_ignore_ascii_case(name))
                .all(|n| {
                    let Some(p) = n.parent().filter(|p| p.kind() == SyntaxKind::MemberExpr) else {
                        return false;
                    };
                    let mut kids = p.children();
                    let (Some(base), Some(member)) = (kids.next(), kids.next()) else {
                        return false;
                    };
                    base == n
                        && self.members[ti].field(&name_of(&member)).is_some()
                        && !p.parent().is_some_and(|g| g.kind() == SyntaxKind::CallExpr)
                });
            if only_fields {
                out.insert(name.clone());
            }
        }
        out
    }

    /// Checks one routine of the object `ctx` describes.
    pub(crate) fn check_routine_in(&mut self, ctx: &ObjCtx, r: &ast::Routine) {
        #[cfg(test)]
        if r.name() == "InjectedCheckPanic" {
            panic!("injected check panic");
        }
        let ObjCtx {
            oi,
            file,
            globals,
            rec,
            rec_unqualified,
            self_obj,
            base_items,
            base_obj,
            base_vars,
        } = ctx;
        let (oi, file, self_obj) = (*oi, *file, *self_obj);
        {
            let item_rec = r.syntax().ancestors().filter_map(ast::Section::cast).find_map(|s| {
                let a = s.args();
                match s.name().to_ascii_lowercase().as_str() {
                    // The table as the object's variables name it, also when it is namespace-qualified.
                    "dataitem" => crate::types::section_object_name(&s, 1),
                    "modify" | "add" => a.first().and_then(|n| base_items.get(&n.to_lowercase()).cloned()),
                    _ => None,
                }
            });
            let (rec, rec_unqualified) = match item_rec {
                Some(t) => (
                    Some(Ty::Record {
                        table: t.into(),
                        temporary: false,
                    }),
                    true,
                ),
                None => (rec.clone(), *rec_unqualified),
            };
            let mut sc = Scope {
                oi,
                file,
                globals: globals.clone(),
                vars: Vec::new(),
                rec,
                rec_unqualified,
                self_obj,
                base_obj: *base_obj,
                base_vars: base_vars.clone(),
            };
            let mut locals = super::posmap::FxMap::default();
            for p in r.params() {
                locals.insert(
                    p.name().to_lowercase(),
                    p.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown),
                );
            }
            if let (Some(n), Some(t)) = (r.return_name(), r.return_type()) {
                locals.insert(n.to_lowercase(), Ty::from_type_ref(&t));
            } else if let Some(t) = r.return_type() {
                // An unnamed return value is used by the code that runs as well (an interface it
                // names must be compiled, or the routine is refused for it).
                self.touch_ty(file, &Ty::from_type_ref(&t));
            }
            for v in r.local_var_decls() {
                let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
                for n in v.names() {
                    locals.insert(n.to_lowercase(), ty.clone());
                }
            }
            // The types of everything in scope are used by the code that runs here. A record variable
            // of a catalog table (`Field`, `Key`, ...) that only has its fields assigned and read is a
            // buffer in memory: it reads no row, so the shapes of all the objects the table lists are
            // not needed (Library Assert's TypeNameOf is such a routine).
            let buffers = self.field_only_records(file, r, &locals);
            for (name, t) in locals.iter() {
                if buffers.contains(name) {
                    if let Ty::Record { table, .. } = t {
                        self.resolve_at(file, ObjKind::Table, table);
                    }
                } else {
                    self.touch_ty(file, t);
                }
            }
            for t in sc.rec.iter() {
                self.touch_ty(file, t);
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

    pub(crate) fn walk(&mut self, sc: &mut Scope, node: &SyntaxNode) {
        if is_expr(node.kind()) {
            self.ty(sc, node);
            return;
        }
        if node.kind() == SyntaxKind::AssignStmt {
            let mut tys = Vec::new();
            for child in node.children() {
                if is_expr(child.kind()) {
                    tys.push((self.ty(sc, &child), child));
                } else {
                    self.walk(sc, &child);
                }
            }
            if let [(target, _), (source, rhs)] = tys.as_slice() {
                let plain = node.children_with_tokens().any(|t| t.kind() == SyntaxKind::Assign);
                if plain {
                    self.al_enum_assignment(sc, rhs, target, source);
                    self.al_integer_text_assignment(sc, rhs, target, source);
                }
            }
            return;
        }
        for child in node.children() {
            self.walk(sc, &child);
        }
    }
}
