//! Lowering resolved syntax to the typed IR (ADR-011). Anything the IR or the
//! builtins registry cannot express yet becomes a reason on the routine
//! (`Routine::unsupported` in `Lowered`), never a guess.

use crate::check::{CheckResult, Res};
use crate::members::Members;
use crate::types::Ty;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_builtins::value::RtType;
use al_ir as ir;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use std::collections::HashMap;

pub struct Lowered {
    pub program: ir::Program,
    /// Per object, per routine: why it cannot run (empty = it can).
    pub unsupported: Vec<Vec<Vec<String>>>,
    /// Per object, per routine: the routines it calls directly, as (object, routine).
    pub calls: Vec<Vec<Vec<(usize, usize)>>>,
}

pub fn lower(ws: &Workspace, members: &[Members], checked: &CheckResult) -> Lowered {
    let catalog = al_builtins::catalog::Catalog::load();
    // Routine numbering: every procedure and trigger node of an object, in source order.
    let routine_nodes: Vec<Vec<ast::Routine>> = (0..ws.objects.len())
        .map(|oi| {
            ws.object_ast(oi)
                .syntax()
                .descendants()
                .filter_map(ast::Routine::cast)
                .collect()
        })
        .collect();
    let routine_at: HashMap<(usize, usize), usize> = routine_nodes
        .iter()
        .enumerate()
        .flat_map(|(oi, rs)| {
            rs.iter()
                .enumerate()
                .map(move |(ri, r)| ((oi, usize::from(r.syntax().text_range().start())), ri))
        })
        .collect();
    let mut out = Lowered {
        program: ir::Program::default(),
        unsupported: Vec::new(),
        calls: Vec::new(),
    };
    // The table whose record is the implicit `Rec` of each object, and each table's extensions.
    let mut table_of: Vec<Option<usize>> = vec![None; ws.objects.len()];
    let mut extensions: HashMap<usize, Vec<usize>> = HashMap::new();
    // Pages: the control tree, the source table, and what the client cannot drive yet.
    let mut layouts: Vec<Option<crate::pages::Layout>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut page_temporary: Vec<bool> = vec![false; ws.objects.len()];
    let mut page_extensions: HashMap<usize, Vec<usize>> = HashMap::new();
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind == ObjKind::PageExtension {
            if let Some(base) = ws.object_ast(oi).extends_parts() {
                if let Resolution::Found(p) = ws.resolve_ref(o.file, ObjKind::Page, &base) {
                    page_extensions.entry(p).or_default().push(oi);
                }
            }
        }
    }
    let mut page_reasons: Vec<Vec<String>> = vec![Vec::new(); ws.objects.len()];
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::Page {
            continue;
        }
        let obj = ws.object_ast(oi);
        let l = crate::pages::layout(&obj);
        page_reasons[oi].extend(l.reasons.iter().cloned());
        if l.controls.iter().any(|c| c.kind == ir::ControlKind::Part) {
            page_reasons[oi].push("parts are not supported by the test client yet".into());
        }
        if l.controls.iter().any(|c| {
            c.section
                .properties()
                .any(|p| p.name().eq_ignore_ascii_case("RunObject"))
        }) {
            page_reasons[oi].push("RunObject actions are not supported by the test client yet".into());
        }
        layouts[oi] = Some(l);
        let prop = |n: &str| {
            obj.properties()
                .find(|p| p.name().eq_ignore_ascii_case(n))
                .map(|p| p.value_text())
        };
        if let Some(t) = prop("SourceTable") {
            match ws.resolve_ref(o.file, ObjKind::Table, &[t.trim().trim_matches('"').to_string()]) {
                Resolution::Found(t) => table_of[oi] = Some(t),
                _ => page_reasons[oi].push(format!("source table {t} is outside the workspace")),
            }
        }
        if prop("SourceTable").is_none() {
            page_reasons[oi].push("a page without a source table is not supported by the test client yet".into());
        }
        page_temporary[oi] = prop("SourceTableTemporary").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        if page_temporary[oi] {
            page_reasons[oi].push("SourceTableTemporary is not supported by the test client yet".into());
        }
        if page_extensions.contains_key(&oi) {
            page_reasons[oi].push("page extensions are not supported by the test client yet".into());
        }
    }
    for (oi, o) in ws.objects.iter().enumerate() {
        match o.kind {
            ObjKind::Table => table_of[oi] = Some(oi),
            ObjKind::TableExtension => {
                if let Some(base) = ws.object_ast(oi).extends_parts() {
                    if let Resolution::Found(t) = ws.resolve_ref(o.file, ObjKind::Table, &base) {
                        table_of[oi] = Some(t);
                        extensions.entry(t).or_default().push(oi);
                    }
                }
            }
            _ => {}
        }
    }
    for (oi, o) in ws.objects.iter().enumerate() {
        let obj = ws.object_ast(oi);
        let mut lw = Lowerer {
            rec_slot: None,
            table_of: &table_of,
            ws,
            members,
            checked,
            catalog: &catalog,
            routine_at: &routine_at,
            oi,
            file: o.file,
            globals: HashMap::new(),
            locals: HashMap::new(),
            global_labels: HashMap::new(),
            local_labels: HashMap::new(),
            reasons: Vec::new(),
            calls: Vec::new(),
            page_reasons: &page_reasons,
            layouts: &layouts,
        };
        let mut globals = Vec::new();
        let mut global_reasons = Vec::new();
        for v in obj.var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            let label = v.type_ref().as_ref().and_then(label_text);
            for n in v.names() {
                if let Some(l) = &label {
                    lw.global_labels.insert(n.to_lowercase(), l.clone());
                }
                match lw.rt(&ty) {
                    Ok(rt) => {
                        lw.globals.insert(n.to_lowercase(), globals.len());
                        globals.push(ir::Slot { name: n, ty: rt });
                    }
                    Err(e) => global_reasons.push(format!("global `{n}`: {e}")),
                }
            }
        }
        let (mut rec_slot, mut xrec_slot) = (None, None);
        if let Some(t) = table_of[oi] {
            for (name, slot) in [("Rec", &mut rec_slot), ("xRec", &mut xrec_slot)] {
                *slot = Some(globals.len());
                lw.globals.insert(name.to_lowercase(), globals.len());
                globals.push(ir::Slot {
                    name: name.into(),
                    ty: RtType::Record(t as u32, false),
                });
            }
        }
        lw.rec_slot = rec_slot;
        if o.kind == ObjKind::Page {
            lw.globals.insert("currpage".into(), globals.len());
            globals.push(ir::Slot {
                name: "CurrPage".into(),
                ty: RtType::TestPage(oi as u32),
            });
        }
        let mut routines = Vec::new();
        let mut unsupported = Vec::new();
        let mut calls = Vec::new();
        for r in &routine_nodes[oi] {
            lw.reasons = global_reasons.clone();
            lw.calls.clear();
            let routine = lw.routine(r);
            // A TestPage drives page routines the call graph cannot see (ADR-012).
            for s in routine.locals.iter().chain(&globals) {
                if let RtType::TestPage(p) = s.ty {
                    let p = p as usize;
                    lw.calls.extend((0..routine_nodes[p].len()).map(|ri| (p, ri)));
                }
            }
            unsupported.push(std::mem::take(&mut lw.reasons));
            calls.push(std::mem::take(&mut lw.calls));
            routines.push(routine);
        }
        let page = layouts[oi]
            .as_ref()
            .map(|l| lw.page(oi, &obj, l, &routine_nodes[oi], &globals, &page_reasons[oi]));
        out.program.objects.push(ir::Object {
            kind: o.kind.label().to_string(),
            id: o.id,
            name: o.name.clone(),
            globals,
            routines,
            properties: obj.properties().map(|p| (p.name(), p.value_text())).collect(),
            enum_values: members[oi]
                .enum_values
                .iter()
                .map(|(v, n)| (*v as i32, n.clone()))
                .collect(),
            enum_captions: members[oi]
                .enum_captions
                .iter()
                .map(|(v, c)| (*v as i32, c.clone()))
                .collect(),
            table: (o.kind == ObjKind::Table)
                .then(|| lw.table_schema(oi, &obj, extensions.get(&oi).cloned().unwrap_or_default(), &routine_at)),
            rec_slot,
            xrec_slot,
            page,
        });
        out.unsupported.push(unsupported);
        out.calls.push(calls);
    }
    out
}

type L<T> = Result<T, String>;

/// An `InitValue` property as a constant of the field's type; `None` for
/// forms not handled yet (the field then starts at its type's default).
fn init_const(text: &str, ty: &Ty) -> Option<ir::Const> {
    let t = text.trim();
    Some(match ty {
        Ty::Integer => ir::Const::Integer(t.parse().ok()?),
        Ty::BigInteger => ir::Const::BigInteger(t.parse().ok()?),
        Ty::Decimal => ir::Const::Decimal(t.parse::<rust_decimal::Decimal>().ok()?.to_string()),
        Ty::Boolean if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes") => ir::Const::Boolean(true),
        Ty::Boolean if t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("no") => ir::Const::Boolean(false),
        Ty::Text(_) | Ty::Code(_) => ir::Const::Text(
            t.strip_prefix('\'')
                .and_then(|x| x.strip_suffix('\''))
                .unwrap_or(t)
                .replace("''", "'"),
        ),
        Ty::Option(m) => {
            let name = t.trim_matches('"');
            ir::Const::Option(m.iter().position(|x| x.eq_ignore_ascii_case(name))? as i32, m.clone())
        }
        Ty::Date => ir::Const::Date(date_literal(t)?),
        Ty::Time => ir::Const::Time(time_literal(t)?),
        _ => return None,
    })
}

/// `20240115D` / `0D` as a day number.
fn date_literal(text: &str) -> Option<i32> {
    let d = text.strip_suffix(['D', 'd'])?;
    if d.chars().all(|c| c == '0') {
        return Some(0);
    }
    if d.len() != 8 || !d.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(al_builtins::format::day_from_civil(
        d[..4].parse().ok()?,
        d[4..6].parse().ok()?,
        d[6..].parse().ok()?,
    ))
}

/// `120000T` / `120000.5T` / `0T` as milliseconds since midnight plus one.
fn time_literal(text: &str) -> Option<i64> {
    let d = text.strip_suffix(['T', 't'])?;
    if d.chars().all(|c| c == '0') && !d.is_empty() && d.len() < 6 {
        return Some(0);
    }
    let (hms, frac) = d.split_once('.').unwrap_or((d, "0"));
    if hms.len() != 6 || !hms.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = hms[..2].parse().ok()?;
    let m: i64 = hms[2..4].parse().ok()?;
    let s: i64 = hms[4..].parse().ok()?;
    let ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + ms + 1)
}

/// The text of a `Label '...'` declaration, quotes undoubled.
fn label_text(t: &ast::TypeRef) -> Option<String> {
    if !t.type_name().eq_ignore_ascii_case("label") {
        return None;
    }
    let tok = t
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|k| k.kind() == SyntaxKind::StringLit)?;
    let text = tok.text();
    Some(text[1..text.len() - 1].replace("''", "'"))
}

struct Lowerer<'a> {
    /// Global slot of the implicit `Rec` of the current object.
    rec_slot: Option<usize>,
    table_of: &'a [Option<usize>],
    ws: &'a Workspace,
    members: &'a [Members],
    checked: &'a CheckResult,
    catalog: &'a al_builtins::catalog::Catalog,
    routine_at: &'a HashMap<(usize, usize), usize>,
    oi: usize,
    file: usize,
    globals: HashMap<String, usize>,
    locals: HashMap<String, usize>,
    /// Declared text of Label variables, by lowercase name; reads lower to that constant.
    global_labels: HashMap<String, String>,
    local_labels: HashMap<String, String>,
    reasons: Vec<String>,
    calls: Vec<(usize, usize)>,
    /// Per object: why the test client cannot drive it yet (pages).
    page_reasons: &'a [Vec<String>],
    layouts: &'a [Option<crate::pages::Layout>],
}

fn key(file: usize, n: &SyntaxNode) -> (usize, usize, usize) {
    let r = n.text_range();
    (file, r.start().into(), r.end().into())
}

fn is_stmt(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        Block
            | AssignStmt
            | ExprStmt
            | IfStmt
            | CaseStmt
            | WhileStmt
            | RepeatStmt
            | ForStmt
            | ForeachStmt
            | ExitStmt
            | BreakStmt
            | AssertErrorStmt
            | WithStmt
            | ContinueStmt
    )
}

fn sig_tokens(n: &SyntaxNode) -> Vec<al_syntax::SyntaxToken> {
    n.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect()
}

impl Lowerer<'_> {
    fn field_pos(&self, table: usize, id: u32) -> Option<usize> {
        self.members[table].fields.iter().position(|f| f.id == id)
    }

    fn table_schema(
        &self,
        oi: usize,
        obj: &ast::Object,
        extensions: Vec<usize>,
        routine_at: &HashMap<(usize, usize), usize>,
    ) -> ir::Table {
        let m = &self.members[oi];
        let fields = m
            .fields
            .iter()
            .map(|f| ir::Field {
                id: f.id,
                name: f.name.clone(),
                ty: self.rt(&f.ty).unwrap_or(RtType::Variant),
                stored: f.class == crate::members::FieldClass::Normal,
                init: f.init_value.as_ref().and_then(|v| init_const(v, &f.ty)),
                auto_increment: f.auto_increment,
                relation: f.table_relation,
                init_text: f.init_value.clone(),
                relation_table: f.relation_table.clone(),
                on_validate: match (f.declared_in, f.on_validate) {
                    (Some(o), Some(off)) => routine_at.get(&(o, off)).map(|r| (o, *r)),
                    _ => None,
                },
                props: f.props.clone(),
            })
            .collect();
        let mut keys = Vec::new();
        for o in std::iter::once(oi).chain(extensions.iter().copied()) {
            for sec in self
                .ws
                .object_ast(o)
                .sections()
                .filter(|s| s.name().eq_ignore_ascii_case("keys"))
            {
                for k in sec.sections().filter(|k| k.name().eq_ignore_ascii_case("key")) {
                    let names: Vec<String> = k
                        .args()
                        .get(1)
                        .map(|a| a.split(',').map(|n| n.trim().trim_matches('"').to_string()).collect())
                        .unwrap_or_default();
                    let pos: Vec<usize> = names
                        .iter()
                        .filter_map(|n| m.fields.iter().position(|f| f.name.eq_ignore_ascii_case(n)))
                        .collect();
                    if pos.len() == names.len() && !pos.is_empty() {
                        keys.push(pos);
                    }
                }
            }
        }
        if keys.is_empty() {
            // Without a keys section the first field is the primary key (doc: Table keys).
            keys.push(vec![0]);
        }
        let per_company = !obj.properties().any(|p| {
            p.name().eq_ignore_ascii_case("DataPerCompany") && p.value_text().trim().eq_ignore_ascii_case("false")
        });
        ir::Table {
            fields,
            keys,
            per_company,
            extensions,
        }
    }

    /// The control tree of page `oi`, with bindings and routine indices resolved.
    fn page(
        &self,
        oi: usize,
        obj: &ast::Object,
        layout: &crate::pages::Layout,
        routine_nodes: &[ast::Routine],
        globals: &[ir::Slot],
        reasons: &[String],
    ) -> ir::Page {
        let routine_named = |name: &str| routine_nodes.iter().position(|r| r.name() == name);
        let routine_of = |r: &ast::Routine| {
            self.routine_at
                .get(&(oi, usize::from(r.syntax().text_range().start())))
                .copied()
        };
        let mut unsupported = reasons.to_vec();
        let table = self.table_of[oi];
        let global_names = crate::pages::global_names(obj);
        let base = usize::from(obj.syntax().text_range().start());
        let mut controls = Vec::new();
        for raw in &layout.controls {
            let sec = &raw.section;
            let mut props = Vec::new();
            for p in sec.properties() {
                let name = p.name().to_ascii_lowercase();
                let value = p.value_text();
                let off = usize::from(p.syntax().text_range().start()) - base;
                let dynamic = crate::pages::dynamic_type(&name).filter(|t| !crate::pages::is_literal(*t, &value));
                props.push((
                    name.clone(),
                    match dynamic {
                        Some(_) => match routine_named(&crate::pages::synthetic_name(&name, off)) {
                            Some(r) => ir::PropVal::Routine(r),
                            None => {
                                unsupported.push(format!("property {name} of control {} has no routine", raw.name));
                                ir::PropVal::Lit(value)
                            }
                        },
                        None => ir::PropVal::Lit(value),
                    },
                ));
            }
            let mut binding = ir::Binding::None;
            if raw.kind == ir::ControlKind::Field {
                let args = sec.args();
                match args.get(1).map(|a| crate::pages::classify_source(a, &global_names)) {
                    Some(crate::pages::Source::RecField(f)) => {
                        let pos = table.and_then(|t| {
                            self.members[t]
                                .fields
                                .iter()
                                .position(|x| x.name.eq_ignore_ascii_case(&f))
                        });
                        match pos {
                            Some(p) => binding = ir::Binding::Field(p),
                            None => unsupported.push(format!("control {} is bound to unknown field {f}", raw.name)),
                        }
                    }
                    Some(crate::pages::Source::Var(v)) => {
                        match globals.iter().position(|g| g.name.eq_ignore_ascii_case(&v)) {
                            Some(slot) => binding = ir::Binding::Var(slot),
                            None => unsupported.push(format!(
                                "control {} is bound to variable {v} of an unsupported type",
                                raw.name
                            )),
                        }
                    }
                    Some(crate::pages::Source::Expr) => {
                        let off = usize::from(sec.syntax().text_range().start()) - base;
                        match routine_named(&crate::pages::synthetic_name("source", off)) {
                            Some(r) => binding = ir::Binding::Expr(r),
                            None => unsupported.push(format!("control {} has no source routine", raw.name)),
                        }
                    }
                    None => {}
                }
            }
            let triggers = sec
                .triggers()
                .filter_map(|t| routine_of(&t).map(|r| (t.name().to_ascii_lowercase(), r)))
                .collect();
            let part_page = if raw.kind == ir::ControlKind::Part {
                match sec.args().get(1).map(|n| self.resolve_obj(ObjKind::Page, n)) {
                    Some(Ok(p)) => Some(p),
                    _ => {
                        unsupported.push(format!("part {} shows a page outside the workspace", raw.name));
                        None
                    }
                }
            } else {
                None
            };
            controls.push(ir::Control {
                name: raw.name.clone(),
                kind: raw.kind.clone(),
                parent: raw.parent,
                binding,
                props,
                triggers,
                part_page,
            });
        }
        let triggers = obj
            .triggers()
            .filter_map(|t| routine_of(&t).map(|r| (t.name().to_ascii_lowercase(), r)))
            .collect();
        ir::Page {
            controls,
            triggers,
            curr_page_slot: globals.iter().position(|g| g.name == "CurrPage").unwrap_or(0),
            unsupported,
        }
    }

    fn res(&self, n: &SyntaxNode) -> Option<&Res> {
        self.checked.resolutions.get(&key(self.file, n))
    }

    fn ty_of(&self, n: &SyntaxNode) -> Ty {
        self.checked
            .types
            .get(&key(self.file, n))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    fn resolve_obj(&self, kind: ObjKind, name: &str) -> L<usize> {
        match self.ws.resolve_ref(self.file, kind, &[name.to_string()]) {
            Resolution::Found(i) => Ok(i),
            _ => Err(format!("{} `{name}` is outside the workspace", kind.label())),
        }
    }

    fn rt(&self, t: &Ty) -> L<RtType> {
        Ok(match t {
            Ty::Integer => RtType::Integer,
            Ty::BigInteger => RtType::BigInteger,
            Ty::Decimal => RtType::Decimal,
            Ty::Boolean => RtType::Boolean,
            Ty::Char => RtType::Char,
            Ty::Byte => RtType::Byte,
            Ty::Text(n) => RtType::Text(*n),
            Ty::Code(n) => RtType::Code(*n),
            Ty::Label => RtType::Text(None),
            Ty::Date => RtType::Date,
            Ty::Time => RtType::Time,
            Ty::DateTime => RtType::DateTime,
            Ty::Duration => RtType::Duration,
            Ty::DateFormula => RtType::DateFormula,
            Ty::Guid => RtType::Guid,
            Ty::Variant => RtType::Variant,
            Ty::Option(m) => RtType::Option(m.clone()),
            Ty::Enum(n) => RtType::Enum(self.resolve_obj(ObjKind::Enum, n)? as u32),
            Ty::Object(ObjKind::Codeunit, n) => RtType::Codeunit(self.resolve_obj(ObjKind::Codeunit, n)? as u32),
            Ty::TestPage(n) => {
                let p = self.resolve_obj(ObjKind::Page, n)?;
                match self.page_reasons[p].first() {
                    None => RtType::TestPage(p as u32),
                    Some(why) => return Err(format!("TestPage \"{n}\": {why}")),
                }
            }
            Ty::List(t) => RtType::List(Box::new(self.rt(t)?)),
            Ty::Dictionary(k, v) => RtType::Dictionary(Box::new(self.rt(k)?), Box::new(self.rt(v)?)),
            Ty::Array(d, t) => RtType::Array(d.clone(), Box::new(self.rt(t)?)),
            Ty::Builtin(n) => RtType::Library(n.clone()),
            Ty::Record { table, temporary } => {
                RtType::Record(self.resolve_obj(ObjKind::Table, table)? as u32, *temporary)
            }
            other => return Err(format!("type {other} is not lowered yet")),
        })
    }

    fn routine(&mut self, r: &ast::Routine) -> ir::Routine {
        self.locals.clear();
        self.local_labels.clear();
        let mut locals = Vec::new();
        let mut by_var = Vec::new();
        let add = |lw: &mut Self, name: String, ty: Ty, locals: &mut Vec<ir::Slot>| match lw.rt(&ty) {
            Ok(rt) => {
                lw.locals.insert(name.to_lowercase(), locals.len());
                locals.push(ir::Slot { name, ty: rt });
            }
            Err(e) => {
                lw.reasons.push(format!("`{name}`: {e}"));
                lw.locals.insert(name.to_lowercase(), locals.len());
                locals.push(ir::Slot {
                    name,
                    ty: RtType::Variant,
                });
            }
        };
        for p in r.params() {
            by_var.push(p.is_var());
            let ty = p.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            add(self, p.name(), ty, &mut locals);
        }
        let params = locals.len();
        let ret_ty = r.return_type().map(|t| Ty::from_type_ref(&t));
        let ret = match &ret_ty {
            Some(t) => match self.rt(t) {
                Ok(rt) => Some(rt),
                Err(e) => {
                    self.reasons.push(format!("return type: {e}"));
                    None
                }
            },
            None => None,
        };
        let mut ret_local = None;
        if let (Some(n), Some(t)) = (r.return_name(), &ret_ty) {
            ret_local = Some(locals.len());
            add(self, n, t.clone(), &mut locals);
        }
        for v in r.local_var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            let label = v.type_ref().as_ref().and_then(label_text);
            for n in v.names() {
                if let Some(l) = &label {
                    self.local_labels.insert(n.to_lowercase(), l.clone());
                }
                add(self, n, ty.clone(), &mut locals);
            }
        }
        for a in r.attributes() {
            let n = a.name();
            if n.eq_ignore_ascii_case("CommitBehavior") || n.eq_ignore_ascii_case("ErrorBehavior") {
                self.reasons.push(format!("[{n}] is not implemented yet"));
            }
        }
        let body = match r.body() {
            Some(b) => self.stmts_of(&b),
            None => Vec::new(),
        };
        let field = r
            .syntax()
            .ancestors()
            .filter_map(ast::Section::cast)
            .find(|s| s.name().eq_ignore_ascii_case("field"))
            .and_then(|s| {
                let id: u32 = s.args().first()?.parse().ok()?;
                self.table_of[self.oi].and_then(|t| self.field_pos(t, id))
            });
        ir::Routine {
            name: r.name(),
            field,
            is_trigger: r.is_trigger(),
            attributes: r
                .attributes()
                .map(|a| ir::Attribute {
                    text: a.syntax().text().to_string(),
                    name: a.name(),
                    args: a
                        .args()
                        .map(|l| l.children().filter_map(|e| self.constant(&e).ok()).collect())
                        .unwrap_or_default(),
                })
                .collect(),
            params,
            by_var,
            locals,
            ret,
            ret_local,
            body,
        }
    }

    /// Statement children of a block-like node, flattened.
    fn stmts_of(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        let mut out = Vec::new();
        for c in n.children().filter(|c| is_stmt(c.kind())) {
            out.extend(self.stmt(&c));
        }
        out
    }

    fn stmt_or_empty(&mut self, n: Option<&SyntaxNode>) -> Vec<ir::Stmt> {
        n.map(|n| self.stmt(n)).unwrap_or_default()
    }

    fn stmt(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        let r = match n.kind() {
            Block => return self.stmts_of(n),
            // A try function called as a statement behaves like a normal method
            // (doc: TryFunction attribute), so its errors propagate.
            ExprStmt => self.expr(&kids[0]).map(|e| match e {
                ir::Expr::Builtin { id, receiver, args, .. } => ir::Stmt::Expr(ir::Expr::Builtin {
                    id,
                    receiver,
                    args,
                    ret_used: false,
                }),
                ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args,
                    ..
                } => ir::Stmt::Expr(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args,
                    try_call: false,
                }),
                e => ir::Stmt::Expr(e),
            }),
            AssignStmt => self.assign(n, &kids),
            IfStmt => {
                let else_at = sig_tokens(n)
                    .iter()
                    .find(|t| t.text().eq_ignore_ascii_case("else"))
                    .map(|t| t.text_range().start());
                let cond = self.expr(&kids[0]);
                let (mut then_s, mut else_s) = (Vec::new(), Vec::new());
                for k in kids.iter().skip(1) {
                    let lowered = self.stmt(k);
                    if else_at.is_some_and(|e| k.text_range().start() > e) {
                        else_s.extend(lowered);
                    } else {
                        then_s.extend(lowered);
                    }
                }
                cond.map(|c| ir::Stmt::If(c, then_s, else_s))
            }
            WhileStmt => {
                let cond = self.expr(&kids[0]);
                let body = self.stmt_or_empty(kids.get(1));
                cond.map(|c| ir::Stmt::While(c, body))
            }
            RepeatStmt => {
                let (until, body_nodes) = kids.split_last().expect("repeat has until");
                let mut body = Vec::new();
                for b in body_nodes {
                    body.extend(self.stmt(b));
                }
                self.expr(until).map(|u| ir::Stmt::Repeat(body, u))
            }
            ForStmt => {
                let down = sig_tokens(n).iter().any(|t| t.text().eq_ignore_ascii_case("downto"));
                let var = self.place(&kids[0]);
                let from = self.expr(&kids[1]);
                let to = self.expr(&kids[2]);
                let body = self.stmt_or_empty(kids.get(3));
                (|| {
                    Ok(ir::Stmt::For {
                        var: var?,
                        from: from?,
                        to: to?,
                        down,
                        body,
                    })
                })()
            }
            ForeachStmt => {
                let var = self.place(&kids[0]);
                let coll = self.expr(&kids[1]);
                let body = self.stmt_or_empty(kids.get(2));
                (|| {
                    Ok(ir::Stmt::Foreach {
                        var: var?,
                        collection: coll?,
                        body,
                    })
                })()
            }
            CaseStmt => self.case(&kids),
            ExitStmt => match kids.first() {
                None => Ok(ir::Stmt::Exit(None)),
                Some(e) => self.expr(e).map(|v| ir::Stmt::Exit(Some(v))),
            },
            BreakStmt => Ok(ir::Stmt::Break),
            AssertErrorStmt => Ok(ir::Stmt::AssertError(self.stmt_or_empty(kids.first()))),
            other => Err(format!("statement {other:?} is not lowered yet")),
        };
        match r {
            Ok(s) => vec![s],
            Err(e) => {
                self.reasons.push(e);
                Vec::new()
            }
        }
    }

    fn assign(&mut self, n: &SyntaxNode, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        let place = self.place(&kids[0])?;
        let slot = self.rt(&self.ty_of(&kids[0]))?;
        let rhs = self.expr(&kids[1])?;
        let op = sig_tokens(n).into_iter().find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::Assign
                    | SyntaxKind::PlusAssign
                    | SyntaxKind::MinusAssign
                    | SyntaxKind::StarAssign
                    | SyntaxKind::SlashAssign
            )
        });
        let value = match op.map(|t| t.kind()) {
            Some(SyntaxKind::PlusAssign) => {
                ir::Expr::Binary(ir::BinOp::Add, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::MinusAssign) => {
                ir::Expr::Binary(ir::BinOp::Sub, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::StarAssign) => {
                ir::Expr::Binary(ir::BinOp::Mul, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::SlashAssign) => {
                ir::Expr::Binary(ir::BinOp::Div, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            _ => rhs,
        };
        Ok(ir::Stmt::Assign(place, ir::Expr::Convert(slot, Box::new(value))))
    }

    fn case(&mut self, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        let scrutinee = self.expr(&kids[0])?;
        let mut branches = Vec::new();
        let mut otherwise = Vec::new();
        for k in &kids[1..] {
            match k.kind() {
                SyntaxKind::CaseBranch => {
                    let parts: Vec<SyntaxNode> = k.children().collect();
                    let mut labels = Vec::new();
                    let mut body = Vec::new();
                    for p in &parts {
                        if is_stmt(p.kind()) {
                            body.extend(self.stmt(p));
                        } else {
                            labels.push(self.set_elem(p)?);
                        }
                    }
                    branches.push((labels, body));
                }
                SyntaxKind::CaseElse => otherwise = self.stmts_of(k),
                _ => {}
            }
        }
        Ok(ir::Stmt::Case {
            scrutinee,
            branches,
            otherwise,
        })
    }

    fn set_elem(&mut self, n: &SyntaxNode) -> L<ir::SetElem> {
        if n.kind() != SyntaxKind::RangeExpr {
            return Ok(ir::SetElem::One(self.expr(n)?));
        }
        let dotdot = sig_tokens(n)
            .into_iter()
            .find(|t| t.kind() == SyntaxKind::DotDot)
            .expect("range has ..")
            .text_range()
            .start();
        let mut lo = None;
        let mut hi = None;
        for c in n.children() {
            let e = self.expr(&c)?;
            if c.text_range().start() < dotdot {
                lo = Some(e);
            } else {
                hi = Some(e);
            }
        }
        Ok(ir::SetElem::Range(lo, hi))
    }

    fn place(&mut self, n: &SyntaxNode) -> L<ir::Place> {
        match n.kind() {
            SyntaxKind::ParenExpr => self.place(&n.children().next().ok_or("empty parentheses")?),
            SyntaxKind::IndexExpr => {
                let kids: Vec<SyntaxNode> = n.children().collect();
                let base = self.place(&kids[0])?;
                let idx = kids[1..].iter().map(|k| self.expr(k)).collect::<L<Vec<_>>>()?;
                Ok(ir::Place::Index(Box::new(base), idx))
            }
            _ => match self.res(n) {
                Some(Res::Local(name)) => self
                    .locals
                    .get(name)
                    .map(|i| ir::Place::Local(*i))
                    .ok_or_else(|| format!("local `{name}`")),
                Some(Res::Global(name)) => self
                    .globals
                    .get(name)
                    .map(|i| ir::Place::Global(*i))
                    .ok_or_else(|| format!("implicit variable `{name}` is not lowered yet")),
                Some(Res::Field { table, id }) => {
                    let (table, id) = (*table, *id);
                    let pos = self.field_pos(table, id).ok_or("field position")?;
                    let base = match n.kind() {
                        SyntaxKind::MemberExpr => self.place(&n.children().next().ok_or("member base")?)?,
                        _ => ir::Place::Global(self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?),
                    };
                    Ok(ir::Place::Field(Box::new(base), pos))
                }
                _ => Err(format!("`{}` is not assignable", n.text())),
            },
        }
    }

    fn constant(&self, n: &SyntaxNode) -> L<ir::Const> {
        let t = sig_tokens(n).into_iter().next().ok_or("empty literal")?;
        let text = t.text();
        let bad = || format!("literal `{text}` is not lowered yet");
        Ok(match t.kind() {
            SyntaxKind::Integer => match text.parse::<i32>() {
                Ok(i) => ir::Const::Integer(i),
                Err(_) => return Err(bad()),
            },
            SyntaxKind::Decimal => ir::Const::Decimal(text.to_string()),
            SyntaxKind::StringLit => ir::Const::Text(text[1..text.len() - 1].replace("''", "'")),
            SyntaxKind::BigIntLit => ir::Const::BigInteger(text[..text.len() - 1].parse().map_err(|_| bad())?),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("true") => ir::Const::Boolean(true),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("false") => ir::Const::Boolean(false),
            SyntaxKind::DateLit => {
                let d = &text[..text.len() - 1];
                if d.chars().all(|c| c == '0') {
                    ir::Const::Date(0)
                } else if d.len() == 8 {
                    let (y, m, dd) = (
                        d[..4].parse().map_err(|_| bad())?,
                        d[4..6].parse().map_err(|_| bad())?,
                        d[6..].parse().map_err(|_| bad())?,
                    );
                    ir::Const::Date(al_builtins::format::day_from_civil(y, m, dd))
                } else {
                    return Err(bad());
                }
            }
            SyntaxKind::TimeLit => {
                let d = &text[..text.len() - 1];
                if d.chars().all(|c| c == '0') {
                    ir::Const::Time(0)
                } else {
                    let (hms, frac) = d.split_once('.').unwrap_or((d, "0"));
                    if hms.len() != 6 {
                        return Err(bad());
                    }
                    let h: i64 = hms[..2].parse().map_err(|_| bad())?;
                    let m: i64 = hms[2..4].parse().map_err(|_| bad())?;
                    let s: i64 = hms[4..].parse().map_err(|_| bad())?;
                    let ms: i64 = format!("{frac:0<3}")[..3].parse().map_err(|_| bad())?;
                    ir::Const::Time(((h * 60 + m) * 60 + s) * 1000 + ms + 1)
                }
            }
            SyntaxKind::DateTimeLit if text[..text.len() - 2].chars().all(|c| c == '0') => ir::Const::DateTime(0),
            _ => return Err(bad()),
        })
    }

    fn expr(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        match n.kind() {
            Literal => Ok(ir::Expr::Const(self.constant(n)?)),
            ParenExpr => self.expr(&kids[0]),
            PrefixExpr => {
                let op = match sig_tokens(n)[0].text().to_ascii_lowercase().as_str() {
                    "not" => ir::UnOp::Not,
                    "-" => ir::UnOp::Neg,
                    _ => ir::UnOp::Plus,
                };
                Ok(ir::Expr::Unary(op, Box::new(self.expr(&kids[0])?)))
            }
            BinExpr => {
                let op = sig_tokens(n)[0].text().to_ascii_lowercase();
                let l = self.expr(&kids[0])?;
                if op == "in" {
                    let set = &kids[1];
                    if set.kind() != SetExpr {
                        return Err("`in` with a non-literal set is not lowered yet".into());
                    }
                    let elems = set.children().map(|e| self.set_elem(&e)).collect::<L<Vec<_>>>()?;
                    return Ok(ir::Expr::In(Box::new(l), elems));
                }
                let r = self.expr(&kids[1])?;
                let op = match op.as_str() {
                    "+" => ir::BinOp::Add,
                    "-" => ir::BinOp::Sub,
                    "*" => ir::BinOp::Mul,
                    "/" => ir::BinOp::Div,
                    "div" => ir::BinOp::IntDiv,
                    "mod" => ir::BinOp::Mod,
                    "and" => ir::BinOp::And,
                    "or" => ir::BinOp::Or,
                    "xor" => ir::BinOp::Xor,
                    "=" => ir::BinOp::Eq,
                    "<>" => ir::BinOp::Ne,
                    "<" => ir::BinOp::Lt,
                    "<=" => ir::BinOp::Le,
                    ">" => ir::BinOp::Gt,
                    ">=" => ir::BinOp::Ge,
                    other => return Err(format!("operator `{other}`")),
                };
                Ok(ir::Expr::Binary(op, Box::new(l), Box::new(r)))
            }
            TernaryExpr => Ok(ir::Expr::Ternary(
                Box::new(self.expr(&kids[0])?),
                Box::new(self.expr(&kids[1])?),
                Box::new(self.expr(&kids[2])?),
            )),
            CallExpr => {
                let args: Vec<SyntaxNode> = kids.get(1).map(|a| a.children().collect()).unwrap_or_default();
                self.call(n, &kids[0], &args)
            }
            ScopeExpr => match self.res(n).cloned() {
                Some(Res::EnumValue { object, value }) => Ok(ir::Expr::Const(ir::Const::Enum(object as u32, value))),
                Some(Res::OptionValue(i)) => match self.ty_of(n) {
                    Ty::Option(m) => Ok(ir::Expr::Const(ir::Const::Option(i, m))),
                    _ => Err("option value without its option type".into()),
                },
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                _ => Err(format!("`{}` is not lowered yet", n.text())),
            },
            MemberExpr if self.is_control_access(n) => self.control(n),
            NameRef | MemberExpr | IndexExpr => match self.res(n).cloned() {
                Some(Res::Proc { .. }) | Some(Res::Builtin(_)) => self.call(n, n, &[]),
                Some(Res::Local(name)) if self.local_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.local_labels[&name].clone())))
                }
                Some(Res::Global(name)) if self.global_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.global_labels[&name].clone())))
                }
                _ => Ok(ir::Expr::Load(self.place(n)?)),
            },
            other => Err(format!("expression {other:?} `{}` is not lowered yet", n.text())),
        }
    }

    /// `TP.Ctl`: a member access on a test page or part that names a control.
    fn is_control_access(&self, n: &SyntaxNode) -> bool {
        let Some(recv) = n.children().next() else { return false };
        matches!(self.ty_of(&recv), Ty::TestPage(_) | Ty::TestPart(_))
            && (matches!(self.ty_of(n), Ty::TestPart(_))
                || matches!(self.ty_of(n), Ty::Builtin(b) if b == "TestField" || b == "TestAction"))
    }

    fn control(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        let recv = n.children().next().ok_or("control access without a receiver")?;
        let (Ty::TestPage(page) | Ty::TestPart(page)) = self.ty_of(&recv) else {
            return Err("control of a non-page".into());
        };
        let page = self.resolve_obj(ObjKind::Page, &page)?;
        let name = n
            .children()
            .last()
            .and_then(|m| {
                m.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            })
            .map(|t| ast::ident_text(&t))
            .ok_or("control name")?;
        let layout = self.layouts[page].as_ref().ok_or("page layout")?;
        let control = layout
            .controls
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| format!("control `{name}` is not in the page layout"))?;
        Ok(ir::Expr::Control {
            page: Box::new(self.expr(&recv)?),
            control,
        })
    }

    /// Lower-case attribute names of the routine at `offset` in `object`.
    fn routine_attributes(&self, object: usize, offset: usize) -> Vec<String> {
        self.ws
            .object_ast(object)
            .syntax()
            .descendants()
            .filter_map(ast::Routine::cast)
            .find(|r| usize::from(r.syntax().text_range().start()) == offset)
            .map(|r| r.attributes().map(|a| a.name().to_ascii_lowercase()).collect())
            .unwrap_or_default()
    }

    /// A call resolved on `node` (the CallExpr, or the callee for a call without parentheses).
    fn call(&mut self, node: &SyntaxNode, callee: &SyntaxNode, args: &[SyntaxNode]) -> L<ir::Expr> {
        let receiver_node = (callee.kind() == SyntaxKind::MemberExpr)
            .then(|| callee.children().next())
            .flatten();
        match self.res(node).cloned() {
            Some(Res::Proc { object, offset }) => {
                let routine = *self.routine_at.get(&(object, offset)).ok_or("procedure not found")?;
                let sig = self.members[object]
                    .procedures
                    .iter()
                    .find(|p| p.offset == offset)
                    .ok_or("procedure signature")?
                    .clone();
                let instance = match &receiver_node {
                    Some(r) if matches!(self.ty_of(r), Ty::Object(ObjKind::Codeunit, _) | Ty::Record { .. }) => {
                        Some(Box::new(self.expr(r)?))
                    }
                    Some(_) => return Err("procedure calls on this receiver are not lowered yet".into()),
                    None if object == self.oi => None,
                    // A table extension calling its table's procedure: on the same Rec.
                    None if self.table_of[object].is_some() && self.rec_slot.is_some() => Some(Box::new(
                        ir::Expr::Load(ir::Place::Global(self.rec_slot.expect("checked"))),
                    )),
                    None => return Err("implicit calls into another object are not lowered yet".into()),
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig.params.get(i).ok_or("too many arguments")?;
                    if p.by_var {
                        out.push(ir::Arg::Ref(self.place(a)?));
                    } else {
                        out.push(ir::Arg::Value(ir::Expr::Convert(
                            self.rt(&p.ty)?,
                            Box::new(self.expr(a)?),
                        )));
                    }
                }
                self.calls.push((object, routine));
                let attrs = self.routine_attributes(object, offset);
                if attrs
                    .iter()
                    .any(|a| ["integrationevent", "businessevent", "internalevent"].contains(&a.as_str()))
                {
                    return Err("event publishing is not implemented yet".into());
                }
                let try_call = attrs.iter().any(|a| a == "tryfunction");
                Ok(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args: out,
                    try_call,
                })
            }
            Some(Res::Builtin(id)) => {
                if al_builtins::native::lookup(&id).is_none() {
                    return Err(format!("builtin {id} is not implemented"));
                }
                let sig = self
                    .catalog
                    .by_id(&id)
                    .ok_or_else(|| format!("{id} not in catalog"))?
                    .clone();
                let receiver = match &receiver_node {
                    Some(r) if sig.instance => Some(Box::new(match self.place(r) {
                        Ok(p) => ir::Arg::Ref(p),
                        Err(_) => ir::Arg::Value(self.expr(r)?),
                    })),
                    // An unqualified record method in table code runs on the implicit Rec.
                    None if sig.instance && sig.receiver == "Record" => Some(Box::new(ir::Arg::Ref(
                        ir::Place::Global(self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?),
                    ))),
                    _ => None,
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig
                        .params
                        .get(i)
                        .or_else(|| sig.params.last().filter(|p| p.variadic))
                        .ok_or("too many arguments")?;
                    let field_param =
                        p.name.to_ascii_lowercase().starts_with("field") || p.name.eq_ignore_ascii_case("FromField");
                    if let (true, Some(Res::Field { table, id })) = (field_param, self.res(a).cloned()) {
                        let pos = self.field_pos(table, id).ok_or("field position")?;
                        out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(pos as u32))));
                        continue;
                    }
                    // Generic parameters convert to the collection's element types.
                    let elem = match (p.ty.as_str(), receiver_node.as_ref().map(|r| self.ty_of(r))) {
                        ("T", Some(Ty::List(el))) => Some(*el),
                        ("TKey", Some(Ty::Dictionary(k, _))) => Some(*k),
                        ("TValue", Some(Ty::Dictionary(_, v))) => Some(*v),
                        _ => None,
                    };
                    out.push(if p.by_var {
                        ir::Arg::Ref(self.place(a)?)
                    } else if let Some(el) = elem {
                        ir::Arg::Value(ir::Expr::Convert(self.rt(&el)?, Box::new(self.expr(a)?)))
                    } else {
                        ir::Arg::Value(self.expr(a)?)
                    });
                }
                Ok(ir::Expr::Builtin {
                    id,
                    receiver,
                    args: out,
                    ret_used: true,
                })
            }
            other => Err(format!("call `{}` ({other:?}) is not lowered yet", node.text())),
        }
    }
}

/// Load, check and lower in one go.
pub fn compile(
    apps: Vec<crate::workspace::AppSource>,
    symbols: &std::collections::BTreeSet<String>,
) -> (Workspace, Lowered) {
    let ws = Workspace::load(apps, symbols);
    let members = crate::members::build(&ws);
    let checked = crate::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let lowered = lower(&ws, &members, &checked);
    (ws, lowered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;

    fn lower_src(src: &str) -> (Workspace, Lowered) {
        compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        )
    }

    fn routine<'a>(l: &'a Lowered, name: &str) -> (usize, &'a ir::Routine) {
        l.program.objects[0]
            .routines
            .iter()
            .enumerate()
            .find(|(_, r)| r.name == name)
            .expect("routine")
    }

    #[test]
    fn slots_for_globals_params_and_locals() {
        let (_, l) = lower_src(
            "codeunit 50100 C { var G: Integer; procedure P(A: Text; var B: Decimal) R: Boolean var L: Code[10]; begin end; }",
        );
        let o = &l.program.objects[0];
        assert_eq!(
            o.globals,
            vec![ir::Slot {
                name: "G".into(),
                ty: RtType::Integer
            }]
        );
        let (_, r) = routine(&l, "P");
        assert_eq!(r.params, 2);
        assert_eq!(r.by_var, vec![false, true]);
        let names: Vec<_> = r.locals.iter().map(|s| (s.name.as_str(), s.ty.clone())).collect();
        assert_eq!(
            names,
            vec![
                ("A", RtType::Text(None)),
                ("B", RtType::Decimal),
                ("R", RtType::Boolean),
                ("L", RtType::Code(Some(10)))
            ]
        );
        assert_eq!(r.ret, Some(RtType::Boolean));
        assert_eq!(r.ret_local, Some(2));
    }

    #[test]
    fn assignment_converts_to_the_slot_type() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var D: Decimal; begin D := 1; end; }");
        let (_, r) = routine(&l, "P");
        assert_eq!(
            r.body,
            vec![ir::Stmt::Assign(
                ir::Place::Local(0),
                ir::Expr::Convert(RtType::Decimal, Box::new(ir::Expr::Const(ir::Const::Integer(1))))
            )]
        );
    }

    #[test]
    fn compound_assignment_reads_the_place() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var I: Integer; begin I += 2; end; }");
        let (_, r) = routine(&l, "P");
        let ir::Stmt::Assign(ir::Place::Local(0), ir::Expr::Convert(_, e)) = &r.body[0] else {
            panic!("{:?}", r.body)
        };
        assert_eq!(
            **e,
            ir::Expr::Binary(
                ir::BinOp::Add,
                Box::new(ir::Expr::Load(ir::Place::Local(0))),
                Box::new(ir::Expr::Const(ir::Const::Integer(2)))
            )
        );
    }

    #[test]
    fn builtin_calls_carry_the_overload_id() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var T: Text; begin T := Format(1); end; }");
        let (_, r) = routine(&l, "P");
        let ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) = &r.body[0] else {
            panic!()
        };
        assert!(
            matches!(&**e, ir::Expr::Builtin { id, receiver: None, args, .. } if id == "System.Format#1" && args.len() == 1)
        );
    }

    #[test]
    fn procedure_calls_and_call_graph() {
        let (_, l) = lower_src(
            "codeunit 50100 C { procedure Q(var X: Integer) begin end; procedure P() var I: Integer; begin Q(I); end; }",
        );
        let (qi, _) = routine(&l, "Q");
        let (pi, p) = routine(&l, "P");
        assert_eq!(
            p.body,
            vec![ir::Stmt::Expr(ir::Expr::Call {
                object: 0,
                routine: qi,
                instance: None,
                args: vec![ir::Arg::Ref(ir::Place::Local(0))],
                try_call: false,
            })]
        );
        assert_eq!(l.calls[0][pi], vec![(0, qi)]);
    }

    #[test]
    fn unimplemented_builtin_makes_the_routine_unsupported() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() begin Sleep(1); end; procedure Ok() begin end; }");
        let (pi, _) = routine(&l, "P");
        let (oi, _) = routine(&l, "Ok");
        assert_eq!(
            l.unsupported[0][pi],
            vec!["builtin System.Sleep#1 is not implemented".to_string()]
        );
        assert!(l.unsupported[0][oi].is_empty());
    }

    #[test]
    fn control_flow_and_asserterror() {
        let (_, l) = lower_src(
            "codeunit 50100 C { procedure P() var I: Integer; begin if I = 0 then I := 1 else exit; asserterror Error('x'); end; }",
        );
        let (pi, p) = routine(&l, "P");
        assert!(l.unsupported[0][pi].is_empty(), "{:?}", l.unsupported[0][pi]);
        assert!(matches!(p.body[0], ir::Stmt::If(_, ref t, ref e) if t.len() == 1 && e == &vec![ir::Stmt::Exit(None)]));
        assert!(matches!(p.body[1], ir::Stmt::AssertError(ref b) if b.len() == 1));
    }

    const TABLE: &str = "table 50900 \"ALT T\"
{
    fields
    {
        field(1; \"No.\"; Code[20]) { }
        field(2; Amount; Decimal) { InitValue = 5; trigger OnValidate() begin Amount := Amount * 2; end; }
        field(3; Counter; Integer) { AutoIncrement = true; }
        field(4; Total; Decimal) { FieldClass = FlowField; }
    }
    keys { key(PK; \"No.\") { } key(ByAmount; Amount) { } }
    procedure Double(X: Integer): Integer begin exit(X * 2); end;
    procedure SetAmount() begin Amount := 1; end;
}";

    fn table_ir(l: &Lowered) -> &ir::Table {
        l.program.objects[0].table.as_ref().expect("table schema")
    }

    #[test]
    fn table_schema_fields_keys_and_system_fields() {
        let (_, l) = lower_src(TABLE);
        let t = table_ir(&l);
        let names: Vec<_> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(&names[..4], ["No.", "Amount", "Counter", "Total"]);
        assert!(names.contains(&"SystemId"));
        assert_eq!(t.keys, vec![vec![0], vec![1]]);
        assert_eq!(t.fields[1].init, Some(ir::Const::Decimal("5".into())));
        assert!(t.fields[2].auto_increment);
        assert!(!t.fields[3].stored, "FlowFields are not stored");
        assert!(t.fields[1].on_validate.is_some());
        assert!(t.per_company);
    }

    #[test]
    fn without_keys_the_first_field_is_the_primary_key() {
        let (_, l) = lower_src("table 50901 K { fields { field(1; A; Integer) { } field(2; B; Integer) { } } }");
        assert_eq!(table_ir(&l).keys, vec![vec![0]]);
    }

    #[test]
    fn record_fields_are_places() {
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{ procedure P() var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; end; }}"
        );
        let (_, l) = lower_src(&src);
        let p = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        assert_eq!(p.locals[0].ty, RtType::Record(0, false));
        assert_eq!(
            p.body,
            vec![ir::Stmt::Assign(
                ir::Place::Field(Box::new(ir::Place::Local(0)), 0),
                ir::Expr::Convert(
                    RtType::Code(Some(20)),
                    Box::new(ir::Expr::Const(ir::Const::Text("a".into())))
                )
            )]
        );
    }

    #[test]
    fn implicit_rec_in_table_code() {
        let (_, l) = lower_src(TABLE);
        let o = &l.program.objects[0];
        let rec = o.rec_slot.expect("Rec slot");
        assert_eq!(o.globals[rec].ty, RtType::Record(0, false));
        let p = o.routines.iter().find(|r| r.name == "SetAmount").unwrap();
        assert!(matches!(&p.body[0], ir::Stmt::Assign(ir::Place::Field(b, 1), _) if **b == ir::Place::Global(rec)));
        let v = o.routines.iter().find(|r| r.name == "OnValidate").unwrap();
        assert_eq!(v.field, Some(1));
    }

    #[test]
    fn record_methods_know_whether_their_result_is_used() {
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{ procedure P() var R: Record \"ALT T\"; begin R.Insert(); if R.Get('a') then; end; }}"
        );
        let (_, l) = lower_src(&src);
        let p = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        assert!(
            matches!(&p.body[0], ir::Stmt::Expr(ir::Expr::Builtin { id, receiver: Some(r), ret_used: false, .. }) if id == "Record.Insert#1" && **r == ir::Arg::Ref(ir::Place::Local(0))),
            "{:?}",
            p.body[0]
        );
        assert!(
            matches!(&p.body[1], ir::Stmt::If(ir::Expr::Builtin { ret_used: true, .. }, _, _)),
            "{:?}",
            p.body[1]
        );
    }

    #[test]
    fn field_arguments_become_field_positions() {
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{ procedure P() var R: Record \"ALT T\"; begin R.SetRange(Amount, 1); end; }}"
        );
        let (_, l) = lower_src(&src);
        let p = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        let ir::Stmt::Expr(ir::Expr::Builtin { args, .. }) = &p.body[0] else {
            panic!("{:?}", p.body)
        };
        assert_eq!(args[0], ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(1))));
    }

    #[test]
    fn table_procedures_run_on_the_record() {
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{ procedure P(): Integer var R: Record \"ALT T\"; begin exit(R.Double(2)); end; }}"
        );
        let (_, l) = lower_src(&src);
        let p = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        let ir::Stmt::Exit(Some(ir::Expr::Call {
            object: 0,
            instance: Some(i),
            ..
        })) = &p.body[0]
        else {
            panic!("{:?}", p.body)
        };
        assert_eq!(**i, ir::Expr::Load(ir::Place::Local(0)));
    }

    #[test]
    fn enum_and_option_constants() {
        let (_, l) = lower_src(
            "enum 50900 E { value(0; A) { } value(7; B) { } }\ncodeunit 50100 C { procedure P() var X: Enum E; O: Option P,Q; begin X := E::B; O := O::Q; end; }",
        );
        let r = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        assert!(
            matches!(&r.body[0], ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) if **e == ir::Expr::Const(ir::Const::Enum(0, 7)))
        );
        assert!(
            matches!(&r.body[1], ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) if **e == ir::Expr::Const(ir::Const::Option(1, vec!["P".into(), "Q".into()])))
        );
    }

    const PAGE_SRC: &str = r#"
table 50000 "T" { fields { field(1; "No."; Code[20]) { } field(2; Qty; Integer) { } } keys { key(PK; "No.") { } } }
page 50001 "P"
{
    PageType = Card;
    SourceTable = "T";
    layout
    {
        area(Content)
        {
            field(NoCtl; Rec."No.") { }
            field(QtyCtl; Rec.Qty) { Visible = Show; Editable = false; }
            group(G) { field(Glob; GlobN) { trigger OnValidate() begin GlobN := 7; end; } }
        }
    }
    actions
    {
        area(Processing)
        {
            action(Act) { trigger OnAction() begin Rec.Qty := 1; end; }
        }
    }
    var
        Show: Boolean;
        GlobN: Integer;
    trigger OnOpenPage() begin Show := true; end;
}
codeunit 50002 "C" { procedure P() var TP: TestPage "P"; begin end; }
"#;

    // Corpus handlers/TestPageFieldFormats.Page.al: controls bound to `Rec.Field` and to page
    // variables; handlers/TestPageControlPropertyLiveness_Tests.al: expression-valued Visible.
    #[test]
    fn a_page_lowers_to_a_control_tree_with_bindings_and_triggers() {
        let (_, l) = lower_src(PAGE_SRC);
        let page_oi = l.program.objects.iter().position(|o| o.name == "P").unwrap();
        let o = &l.program.objects[page_oi];
        let pg = o.page.as_ref().expect("a page object has a page");
        let names: Vec<_> = pg.controls.iter().map(|c| (c.name.as_str(), c.kind.clone())).collect();
        assert_eq!(
            names,
            vec![
                ("NoCtl", ir::ControlKind::Field),
                ("QtyCtl", ir::ControlKind::Field),
                ("G", ir::ControlKind::Group),
                ("Glob", ir::ControlKind::Field),
                ("Act", ir::ControlKind::Action),
            ]
        );
        assert_eq!(pg.controls[3].parent, Some(2));
        assert_eq!(pg.controls[0].binding, ir::Binding::Field(0));
        assert_eq!(pg.controls[1].binding, ir::Binding::Field(1));
        let glob_slot = o.globals.iter().position(|g| g.name == "GlobN").unwrap();
        assert_eq!(pg.controls[3].binding, ir::Binding::Var(glob_slot));
        // Literal property stays text, expression becomes a routine of the page.
        assert_eq!(pg.controls[1].prop("editable"), Some(&ir::PropVal::Lit("false".into())));
        let Some(ir::PropVal::Routine(r)) = pg.controls[1].prop("visible") else {
            panic!("{:?}", pg.controls[1].props)
        };
        assert!(o.routines[*r].name.starts_with("__visible_"));
        assert!(pg.controls[3].trigger("onvalidate").is_some());
        assert!(pg.controls[4].trigger("onaction").is_some());
        assert!(pg.triggers.iter().any(|(n, _)| n == "onopenpage"));
        assert!(o.rec_slot.is_some() && o.xrec_slot.is_some());
        assert_eq!(o.globals[pg.curr_page_slot].name, "CurrPage");
        assert!(pg.unsupported.is_empty(), "{:?}", pg.unsupported);
    }

    #[test]
    fn a_testpage_variable_is_typed_by_its_page_and_reaches_the_page_routines() {
        let (_, l) = lower_src(PAGE_SRC);
        let page_oi = l.program.objects.iter().position(|o| o.name == "P").unwrap();
        let cu = l.program.objects.iter().position(|o| o.name == "C").unwrap();
        let p = l.program.objects[cu]
            .routines
            .iter()
            .position(|r| r.name == "P")
            .unwrap();
        assert_eq!(
            l.program.objects[cu].routines[p].locals[0].ty,
            RtType::TestPage(page_oi as u32)
        );
        assert!(l.unsupported[cu][p].is_empty(), "{:?}", l.unsupported[cu][p]);
        // The page's triggers are not called from AL, so the call graph gets edges to them.
        let n = l.program.objects[page_oi].routines.len();
        for r in 0..n {
            assert!(l.calls[cu][p].contains(&(page_oi, r)), "no edge to routine {r}");
        }
    }
}
