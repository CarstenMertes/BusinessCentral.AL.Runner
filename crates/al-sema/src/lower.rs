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

/// (lower-case attribute name, argument constants) per attribute.
type Attrs = Vec<(String, Vec<ir::Const>)>;

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
        .map(|oi| ast::routines_in(ws.object_ast(oi).syntax()))
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
        program: ir::Program {
            objects: Vec::new(),
            apps: ws
                .apps
                .iter()
                .map(|a| ir::AppInfo {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    publisher: String::new(),
                    version: String::new(),
                    dependencies: a.dependencies.clone(),
                    resources: Vec::new(),
                })
                .collect(),
        },
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
    // The page objects each page shows as parts.
    let mut page_parts: Vec<Vec<usize>> = vec![Vec::new(); ws.objects.len()];
    // The pages and codeunits each page's RunObject actions run.
    let mut page_runs: Vec<Vec<usize>> = vec![Vec::new(); ws.objects.len()];
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::Page {
            continue;
        }
        let obj = ws.object_ast(oi);
        let l = crate::pages::layout(&obj);
        page_reasons[oi].extend(l.reasons.iter().cloned());
        for c in l.controls.iter().filter(|c| c.kind == ir::ControlKind::Part) {
            let prop_of = |n: &str| {
                c.section
                    .properties()
                    .find(|p| p.name().eq_ignore_ascii_case(n))
                    .map(|p| p.value_text())
            };
            if let Some(link) = prop_of("SubPageLink") {
                if let Err(why) = crate::pages::parse_sub_page_link(&link) {
                    page_reasons[oi].push(format!("part {}: {why} is not supported yet", c.name));
                }
            }
            if let Some(provider) = prop_of("Provider") {
                let provider = provider.trim().trim_matches('"').to_string();
                if !l
                    .controls
                    .iter()
                    .any(|p| p.kind == ir::ControlKind::Part && p.name.eq_ignore_ascii_case(&provider))
                {
                    page_reasons[oi].push(format!(
                        "part {}: Provider `{provider}` is not a part of the page",
                        c.name
                    ));
                }
            }
            match c.section.args().get(1) {
                Some(n) => match ws.resolve_ref(o.file, ObjKind::Page, &crate::types::qualified_parts(n)) {
                    Resolution::Found(p) => page_parts[oi].push(p),
                    _ => page_reasons[oi].push(format!(
                        "part {} shows page {} outside the workspace",
                        c.name,
                        crate::types::display_name(n)
                    )),
                },
                None => page_reasons[oi].push(format!("part {} names no page", c.name)),
            }
        }
        // The pages and codeunits the actions run: their routines are reached through the host
        // (a RunObject target runs when the action is invoked, which the call graph cannot see).
        for c in &l.controls {
            let Some(text) = c
                .section
                .properties()
                .find(|p| p.name().eq_ignore_ascii_case("RunObject"))
                .map(|p| p.value_text())
            else {
                continue;
            };
            if let Ok((kind, parts)) = crate::pages::parse_run_object(&text) {
                let found = match kind {
                    ir::RunKind::Page => Some(ObjKind::Page),
                    ir::RunKind::Codeunit => Some(ObjKind::Codeunit),
                    _ => None,
                };
                if let Some(Resolution::Found(t)) = found.map(|k| ws.resolve_ref(o.file, k, &parts)) {
                    if !page_runs[oi].contains(&t) {
                        page_runs[oi].push(t);
                    }
                }
            }
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
        page_temporary[oi] = prop("SourceTableTemporary").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        if page_extensions.contains_key(&oi) {
            page_reasons[oi].push("page extensions are not supported by the test client yet".into());
        }
    }
    // A host cannot be driven where a part page cannot.
    for _ in 0..3 {
        for oi in 0..ws.objects.len() {
            for &p in &page_parts[oi].clone() {
                // What a part page's close triggers do when the host closes is not pinned.
                let closes = ws.object_ast(p).triggers().any(|t| {
                    let n = t.name();
                    n.eq_ignore_ascii_case("OnClosePage") || n.eq_ignore_ascii_case("OnQueryClosePage")
                });
                let reason = format!(
                    "part page `{}` declares a close trigger (what it does when the host closes is not pinned)",
                    ws.objects[p].name
                );
                if closes && !page_reasons[oi].contains(&reason) {
                    page_reasons[oi].push(reason);
                }
                if let Some(why) = page_reasons[p].first().cloned() {
                    let reason = format!("part page `{}`: {why}", ws.objects[p].name);
                    if !page_reasons[oi].contains(&reason) {
                        page_reasons[oi].push(reason);
                    }
                }
            }
        }
    }
    for (oi, o) in ws.objects.iter().enumerate() {
        match o.kind {
            ObjKind::Table => table_of[oi] = Some(oi),
            // `Rec` of a codeunit with TableNo (a RunObject action hands it the host's record).
            ObjKind::Codeunit => {
                let table_no = ws
                    .object_ast(oi)
                    .properties()
                    .find(|p| p.name().eq_ignore_ascii_case("TableNo"))
                    .map(|p| p.value_text());
                if let Some(t) = table_no {
                    if let Resolution::Found(t) =
                        ws.resolve_ref(o.file, ObjKind::Table, &[t.trim().trim_matches('"').to_string()])
                    {
                        table_of[oi] = Some(t);
                    }
                }
            }
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
    // XmlPorts: schema and the reasons the runtime cannot run one yet. The element variables
    // take the first global slots (text, attribute and table nodes in schema order), then the
    // hidden source and destination streams, so the slots are known before anything is lowered.
    let mut xml_layouts: Vec<Option<crate::xmlports::Layout>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut xml_irs: Vec<Option<ir::XmlPort>> = (0..ws.objects.len()).map(|_| None).collect();
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::XmlPort {
            continue;
        }
        let l = crate::xmlports::layout(&ws.object_ast(oi));
        let mut slots = HashMap::new();
        for n in &l.nodes {
            if matches!(
                n.kind,
                ir::XmlNodeKind::Text | ir::XmlNodeKind::TextAttr | ir::XmlNodeKind::Table
            ) {
                let next = slots.len();
                slots.entry(n.name.to_lowercase()).or_insert(next);
            }
        }
        let n = slots.len();
        let cx = crate::xmlports::Context {
            ws,
            members,
            oi,
            file: o.file,
            layout: &l,
            slots: &slots,
            routine_at: &routine_at,
            source_slot: n,
            destination_slot: n + 1,
        };
        xml_irs[oi] = Some(crate::xmlports::build(&cx, Vec::new()));
        xml_layouts[oi] = Some(l);
    }
    // Reports: the dataset, and the reasons the runtime cannot run one yet. The data item records
    // take the first global slots (in source order), then the hidden `UseRequestPage` flag, so the
    // slots are known before anything is lowered. A reportextension that changes what a run does
    // (it has a procedure, a trigger or a section other than a request page) belongs to the
    // report it extends; one that changes nothing is ignored.
    let mut report_layouts: Vec<Option<crate::reports::Layout>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut report_irs: Vec<Option<ir::Report>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut ext_layouts: Vec<Option<crate::reports::ExtLayout>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut ext_irs: Vec<Option<ir::ReportExt>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut ext_base: Vec<Option<usize>> = vec![None; ws.objects.len()];
    // Per report: the reportextensions that change a run.
    let mut changing: HashMap<usize, Vec<usize>> = HashMap::new();
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::ReportExtension {
            continue;
        }
        let ext = ws.object_ast(oi);
        let changes_a_run = ext.routines().next().is_some()
            || ext.triggers().next().is_some()
            || ext.sections().any(|s| !s.name().eq_ignore_ascii_case("requestpage"));
        if let (true, Some(base)) = (changes_a_run, ext.extends_parts()) {
            if let Resolution::Found(r) = ws.resolve_ref(o.file, ObjKind::Report, &base) {
                changing.entry(r).or_default().push(oi);
                ext_base[oi] = Some(r);
                ext_layouts[oi] = Some(crate::reports::ext_layout(&ext));
            }
        }
    }
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::Report {
            continue;
        }
        let obj = ws.object_ast(oi);
        let l = crate::reports::layout(&obj);
        let mut slots = HashMap::new();
        for i in &l.items {
            let next = slots.len();
            slots.entry(i.name.to_lowercase()).or_insert(next);
        }
        let exts = changing.get(&oi).cloned().unwrap_or_default();
        let extension_reasons = if exts.len() > 1 {
            vec!["more than one reportextension changes the run (their order is not pinned)".to_string()]
        } else {
            Vec::new()
        };
        let cx = crate::reports::Context {
            ws,
            members,
            file: o.file,
            oi,
            layout: &l,
            slots: &slots,
            request_flag_slot: slots.len(),
            routine_at: &routine_at,
            extension_reasons,
            extension: exts.first().copied().filter(|_| exts.len() == 1),
        };
        report_irs[oi] = Some(crate::reports::build(&cx, &obj, Vec::new()));
        report_layouts[oi] = Some(l);
    }
    for (ei, base) in ext_base.iter().enumerate() {
        let (Some(base), Some(layout)) = (*base, ext_layouts[ei].as_ref()) else {
            continue;
        };
        let (Some(base_layout), Some(base_ir)) = (report_layouts[base].as_ref(), report_irs[base].as_ref()) else {
            continue;
        };
        let cx = crate::reports::ExtContext {
            ws,
            members,
            file: ws.objects[ei].file,
            oi: ei,
            base,
            base_layout,
            base_items: &base_ir.items,
            layout,
            routine_at: &routine_at,
        };
        let (ir, reasons) = crate::reports::build_ext(&cx, &ws.object_ast(ei));
        ext_irs[ei] = Some(ir);
        // The extension's reasons stop the report it changes.
        let name = &ws.objects[ei].name;
        if let Some(r) = report_irs[base].as_mut() {
            r.unsupported.extend(
                reasons
                    .into_iter()
                    .map(|why| format!("reportextension \"{name}\": {why}")),
            );
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
            xmlports: &xml_irs,
            reports: &report_irs,
            attrs: Default::default(),
        };
        let mut globals = Vec::new();
        let mut global_reasons = Vec::new();
        if let Some(l) = &xml_layouts[oi] {
            for n in &l.nodes {
                let (ty, table) = match n.kind {
                    ir::XmlNodeKind::Text | ir::XmlNodeKind::TextAttr => (Ok(RtType::Text(None)), false),
                    ir::XmlNodeKind::Table => (
                        lw.rt(&Ty::Record {
                            table: n.args.get(1).cloned().unwrap_or_default(),
                            temporary: n.section.properties().any(|p| {
                                p.name().eq_ignore_ascii_case("UseTemporary")
                                    && p.value_text().trim().eq_ignore_ascii_case("true")
                            }),
                        }),
                        true,
                    ),
                    _ => continue,
                };
                let key = n.name.to_lowercase();
                if lw.globals.contains_key(&key) {
                    continue;
                }
                lw.globals.insert(key, globals.len());
                match ty {
                    Ok(rt) => globals.push(ir::Slot {
                        name: n.name.clone(),
                        ty: rt,
                    }),
                    Err(e) => {
                        global_reasons.push(format!(
                            "{} `{}`: {e}",
                            if table { "table element" } else { "variable" },
                            n.name
                        ));
                        globals.push(ir::Slot {
                            name: n.name.clone(),
                            ty: RtType::Variant,
                        });
                    }
                }
            }
            for name in ["__source", "__destination"] {
                let ty = if name == "__source" { "InStream" } else { "OutStream" };
                globals.push(ir::Slot {
                    name: name.into(),
                    ty: RtType::Library(ty.into()),
                });
            }
        }
        if let Some(l) = &report_layouts[oi] {
            for i in &l.items {
                let key = i.name.to_lowercase();
                if lw.globals.contains_key(&key) {
                    continue;
                }
                let ty = lw.rt(&Ty::Record {
                    table: i.table.clone(),
                    temporary: false,
                });
                lw.globals.insert(key, globals.len());
                match ty {
                    Ok(rt) => globals.push(ir::Slot {
                        name: i.name.clone(),
                        ty: rt,
                    }),
                    Err(e) => {
                        global_reasons.push(format!("dataitem `{}`: {e}", i.name));
                        globals.push(ir::Slot {
                            name: i.name.clone(),
                            ty: RtType::Variant,
                        });
                    }
                }
            }
            globals.push(ir::Slot {
                name: "__userequestpage".into(),
                ty: RtType::Integer,
            });
        }
        // A reportextension that changes a run names the base items' records first (the run points
        // them at the report's own), then the items it adds.
        if let (Some(base), Some(l)) = (ext_base[oi], &ext_layouts[oi]) {
            let base_items = report_layouts[base].as_ref().map_or(&[][..], |b| &b.items[..]);
            for i in base_items.iter().chain(&l.items) {
                let key = i.name.to_lowercase();
                if lw.globals.contains_key(&key) {
                    continue;
                }
                let ty = lw.rt(&Ty::Record {
                    table: i.table.clone(),
                    temporary: false,
                });
                lw.globals.insert(key, globals.len());
                match ty {
                    Ok(rt) => globals.push(ir::Slot {
                        name: i.name.clone(),
                        ty: rt,
                    }),
                    Err(e) => {
                        global_reasons.push(format!("dataitem `{}`: {e}", i.name));
                        globals.push(ir::Slot {
                            name: i.name.clone(),
                            ty: RtType::Variant,
                        });
                    }
                }
            }
        }
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
                    ty: RtType::Record(t as u32, lw.is_temporary_table(t) || page_temporary[oi]),
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
            // A trigger of a data item runs with the item's record as its `Rec`.
            if matches!(o.kind, ObjKind::Report | ObjKind::ReportExtension) {
                lw.rec_slot = r
                    .syntax()
                    .ancestors()
                    .filter_map(ast::Section::cast)
                    .find(|s| ["dataitem", "modify"].iter().any(|k| s.name().eq_ignore_ascii_case(k)))
                    .and_then(|s| s.args().into_iter().next())
                    .and_then(|n| lw.globals.get(&n.to_lowercase()).copied());
            }
            let routine = lw.routine(r);
            // A TestPage drives page routines the call graph cannot see (ADR-012).
            for s in routine.locals.iter().chain(&globals) {
                if let RtType::TestPage(p) | RtType::Page(p) = s.ty {
                    // The page and, transitively, the pages it shows as parts.
                    let mut seen = vec![p as usize];
                    let mut at = 0;
                    while at < seen.len() {
                        let q = seen[at];
                        at += 1;
                        lw.calls.extend((0..routine_nodes[q].len()).map(|ri| (q, ri)));
                        for &part in page_parts[q].iter().chain(&page_runs[q]) {
                            if !seen.contains(&part) {
                                seen.push(part);
                            }
                        }
                    }
                }
                // An xmlport's triggers are run by Import and Export, and a report's by Run, which
                // the graph cannot see.
                if let RtType::Codeunit(p) = s.ty {
                    let p = p as usize;
                    if matches!(ws.objects[p].kind, ObjKind::XmlPort | ObjKind::Report) {
                        lw.calls.extend((0..routine_nodes[p].len()).map(|ri| (p, ri)));
                    }
                    if let Some(e) = report_irs[p].as_ref().and_then(|r| r.extension) {
                        lw.calls.extend((0..routine_nodes[e].len()).map(|ri| (e, ri)));
                    }
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
            app: o.app as u32,
            xmlport: xml_irs[oi].clone(),
            report: report_irs[oi].clone(),
            report_ext: ext_irs[oi].clone(),
            query: checked.queries[oi].as_ref().map(|q| q.query.clone()),
        });
        out.unsupported.push(unsupported);
        out.calls.push(calls);
    }
    out
}

type L<T> = Result<T, String>;

/// An `InitValue` property as a constant of the field's type; `None` for
/// forms not handled yet (the field then starts at its type's default).
pub(crate) fn init_const(text: &str, ty: &Ty) -> Option<ir::Const> {
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

/// A `const(..)` value as a constant of the source field's type.
pub(crate) fn calc_const(
    ws: &Workspace,
    members: &[Members],
    file: usize,
    raw: &str,
    ty: &Ty,
) -> Result<ir::Const, String> {
    let raw = raw.trim();
    // `Database::"X"` / `Report::"X"`: the object's id (corpus
    // record/TestCalcFormulaDatabaseConst.Codeunit.al:7).
    if let Some((kind, name)) = raw.split_once("::") {
        let kind = match kind.trim().to_ascii_lowercase().as_str() {
            "database" => Some(ObjKind::Table),
            other => ObjKind::from_type_name(other),
        };
        let name = name.trim().trim_matches('"').to_string();
        let Some(kind) = kind.filter(|k| *k != ObjKind::Enum) else {
            return Err(format!("CalcFormula const({raw}) is not understood yet"));
        };
        let Resolution::Found(o) = ws.resolve_ref(file, kind, std::slice::from_ref(&name)) else {
            return Err(format!("CalcFormula const({raw}): `{name}` is outside the workspace"));
        };
        let id = ws.objects[o]
            .id
            .ok_or_else(|| format!("CalcFormula const({raw}) has no object id"))?;
        return match ty {
            Ty::Integer => Ok(ir::Const::Integer(id as i32)),
            Ty::BigInteger => Ok(ir::Const::BigInteger(i64::from(id))),
            Ty::Decimal => Ok(ir::Const::Decimal(id.to_string())),
            _ => Err(format!("CalcFormula const({raw}) on a non-numeric field")),
        };
    }
    let unquoted = raw.trim_matches('"');
    if let Ty::Enum(name) = ty {
        let Resolution::Found(e) = ws.resolve_ref(file, ObjKind::Enum, std::slice::from_ref(name)) else {
            return Err(format!(
                "CalcFormula const({raw}): enum `{name}` is outside the workspace"
            ));
        };
        let values = &members[e].enum_values;
        let ordinal = values
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(unquoted))
            .map(|(v, _)| *v as i32)
            .or_else(|| unquoted.parse().ok())
            .ok_or_else(|| format!("CalcFormula const({raw}) is not a member of enum `{name}`"))?;
        return Ok(ir::Const::Enum(e as u32, ordinal));
    }
    if let Ty::Option(members) = ty {
        if let Ok(n) = unquoted.parse::<i32>() {
            return Ok(ir::Const::Option(n, members.clone()));
        }
    }
    init_const(raw, ty).ok_or_else(|| format!("CalcFormula const({raw}) is not understood for a {ty} field"))
}

/// The runtime type of a scalar type (numbers, text, dates, options, enums); `None` for the rest.
pub(crate) fn scalar_rt(ws: &Workspace, file: usize, t: &Ty) -> Option<L<RtType>> {
    Some(Ok(match t {
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
        Ty::Enum(n) => match ws.resolve_ref(file, ObjKind::Enum, &crate::types::qualified_parts(n)) {
            Resolution::Found(i) => RtType::Enum(i as u32),
            _ => {
                return Some(Err(format!(
                    "{} `{}` is outside the workspace",
                    ObjKind::Enum.label(),
                    crate::types::display_name(n)
                )));
            }
        },
        _ => return None,
    }))
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

/// A filter expression written in AL, with double-quoted names turned into
/// the single-quoted literals the filter grammar understands. AL quotes an
/// identifier with `"`; the filter grammar treats `"` as an ordinary
/// character (corpus record/TestCalcFormulaQuotedFilterTests.Codeunit.al:9).
pub(crate) fn rewrite_quotes(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('\'');
                for n in chars.by_ref() {
                    match n {
                        '"' => break,
                        '\'' => out.push_str("''"),
                        n => out.push(n),
                    }
                }
                out.push('\'');
            }
            '\'' => {
                out.push('\'');
                while let Some(n) = chars.next() {
                    out.push(n);
                    if n == '\'' {
                        if chars.peek() == Some(&'\'') {
                            out.push(chars.next().expect("peeked"));
                        } else {
                            break;
                        }
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
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
    /// Per object: the schema of an xmlport (its `unsupported` says why a variable is refused).
    xmlports: &'a [Option<ir::XmlPort>],
    /// Per object: the dataset of a report (its `unsupported` says why a variable is refused).
    reports: &'a [Option<ir::Report>],
    /// `routine_attributes` memo, by (object, routine offset).
    attrs: std::cell::RefCell<HashMap<(usize, usize), Attrs>>,
}

/// An interface type anywhere inside a collection type.
fn holds_interface(t: &Ty) -> bool {
    match t {
        Ty::Object(ObjKind::Interface, _) => true,
        Ty::List(t) | Ty::Array(_, t) => holds_interface(t),
        Ty::Dictionary(k, v) => holds_interface(k) || holds_interface(v),
        _ => false,
    }
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

    /// The arms of a field's `TableRelation`, with names resolved from the declaring file's scope.
    fn relation_arms(&self, oi: usize, f: &crate::members::FieldInfo) -> Vec<ir::RelationArm> {
        let Some(text) = f
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TableRelation"))
            .map(|(_, v)| v)
        else {
            return Vec::new();
        };
        let Some(parsed) = crate::relation::parse(text) else {
            return Vec::new();
        };
        let file = f
            .declared_in
            .map_or(self.ws.objects[oi].file, |d| self.ws.objects[d].file);
        let own = &self.members[oi];
        let field_pos =
            |m: &crate::members::Members, name: &str| m.fields.iter().position(|x| x.name.eq_ignore_ascii_case(name));
        let mut arms = Vec::new();
        for a in parsed {
            let mut conditions = Vec::new();
            for (name, value) in &a.conditions {
                let Some(pos) = field_pos(own, name) else {
                    return Vec::new();
                };
                let Some(value) = self.condition_const(file, value, &own.fields[pos].ty) else {
                    return Vec::new();
                };
                conditions.push(ir::RelationCond { field: pos, value });
            }
            // The whole path names a table, or all but the last part do and the last is a field.
            let resolve = |parts: &[String]| match self.ws.resolve_ref_certain(file, ObjKind::Table, parts) {
                Resolution::Found(t) => Some(t),
                _ => None,
            };
            let (table, field) = match resolve(&a.path) {
                Some(t) => (Some(t), None),
                None if a.path.len() > 1 => match resolve(&a.path[..a.path.len() - 1]) {
                    Some(t) => match field_pos(&self.members[t], &a.path[a.path.len() - 1]) {
                        Some(p) => (Some(t), Some(p)),
                        None => return Vec::new(),
                    },
                    None => (None, None),
                },
                None => (None, None),
            };
            // A relation onto SystemRowVersion relates to the primary key instead (BC encodes
            // the field as id 0): corpus record/TestCalcFormulaSystemFieldsTests.Codeunit.al:229-260.
            let field = match (table, field) {
                (Some(t), Some(p)) if self.members[t].fields[p].id == 2_000_000_005 => None,
                (_, f) => f,
            };
            let wheres = match (&a.wheres, table) {
                (Some(ws), Some(t)) => ws
                    .iter()
                    .map(|w| match w {
                        crate::relation::ParsedWhere::Value { related, text } => Some(ir::RelationWhere::Value {
                            related: field_pos(&self.members[t], related)?,
                            text: text.clone(),
                        }),
                        crate::relation::ParsedWhere::Field { related, own: o } => Some(ir::RelationWhere::Field {
                            related: field_pos(&self.members[t], related)?,
                            own: field_pos(own, o)?,
                        }),
                    })
                    .collect(),
                (Some(ws), None) if ws.is_empty() => Some(Vec::new()),
                _ => None,
            };
            arms.push(ir::RelationArm {
                conditions,
                table,
                table_name: a.path.join("."),
                field,
                wheres,
            });
        }
        arms
    }

    /// The constant of a `Field = const(x)` condition, typed by the field.
    fn condition_const(&self, file: usize, text: &str, ty: &Ty) -> Option<ir::Const> {
        if let Ty::Enum(name) = ty {
            let Resolution::Found(e) = self
                .ws
                .resolve_ref_certain(file, ObjKind::Enum, std::slice::from_ref(name))
            else {
                return None;
            };
            let (value, _) = self.members[e]
                .enum_values
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(text.trim()))?;
            return Some(ir::Const::Enum(e as u32, *value as i32));
        }
        init_const(text, ty)
    }

    /// The position of field `id`, refusing the audit system fields (SystemCreatedAt/By,
    /// SystemModifiedAt/By, ids 2000000001-4): the core has no clock or user (ADR-006), so
    /// the runtime stamps no values into them and would read defaults.
    fn readable_field_pos(&self, table: usize, id: u32) -> L<usize> {
        let pos = self.field_pos(table, id).ok_or("field position")?;
        if (2_000_000_001..=2_000_000_004).contains(&id) {
            return Err(format!(
                "system field {} is not stamped by the runtime yet",
                self.members[table].fields[pos].name
            ));
        }
        Ok(pos)
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
            .map(|f| {
                let (calc, refusal) = match (&f.class, &f.calc_formula) {
                    (crate::members::FieldClass::FlowField, Some(text)) => {
                        match self.lower_calc(oi, f.declared_in.unwrap_or(oi), text) {
                            Ok(c) => (Some(c), None),
                            Err(e) => (None, Some(e)),
                        }
                    }
                    _ => (None, None),
                };
                ir::Field {
                    id: f.id,
                    name: f.name.clone(),
                    ty: self.rt(&f.ty).unwrap_or(RtType::Variant),
                    stored: f.class == crate::members::FieldClass::Normal,
                    init: f.init_value.as_ref().and_then(|v| init_const(v, &f.ty)),
                    auto_increment: f.auto_increment,
                    relation: f.table_relation,
                    init_text: f.init_value.clone(),
                    relation_table: f.relation_table.clone(),
                    relation_tables: f.relation_tables.clone(),
                    properties: f.properties.clone(),
                    relation_arms: self.relation_arms(oi, f),
                    on_validate: match (f.declared_in, f.on_validate) {
                        (Some(o), Some(off)) => routine_at.get(&(o, off)).map(|r| (o, *r)),
                        _ => None,
                    },
                    flow_filter: f.class == crate::members::FieldClass::FlowFilter,
                    calc,
                    calc_refusal: refusal,
                }
            })
            .collect();
        let mut keys = Vec::new();
        let mut unique_keys = Vec::new();
        for o in std::iter::once(oi).chain(extensions.iter().copied()) {
            for sec in self
                .ws
                .object_ast(o)
                .sections()
                .filter(|s| s.name().eq_ignore_ascii_case("keys"))
            {
                for k in sec.sections().filter(|k| k.name().eq_ignore_ascii_case("key")) {
                    // A Removed key is gone: it takes no index position
                    // (corpus recordref/TestKeyMetadataRowVersionRemoved.al:10-20, :150-170).
                    if k.properties().any(|p| {
                        p.name().eq_ignore_ascii_case("ObsoleteState")
                            && p.value_text().trim().eq_ignore_ascii_case("Removed")
                    }) {
                        continue;
                    }
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
                        unique_keys.push(k.properties().any(|p| {
                            p.name().eq_ignore_ascii_case("Unique")
                                && p.value_text().trim().eq_ignore_ascii_case("true")
                        }));
                    }
                }
            }
        }
        if keys.is_empty() {
            // Without a keys section the first field is the primary key (doc: Table keys).
            keys.push(vec![0]);
            unique_keys.push(false);
        }
        let per_company = !obj.properties().any(|p| {
            p.name().eq_ignore_ascii_case("DataPerCompany") && p.value_text().trim().eq_ignore_ascii_case("false")
        });
        ir::Table {
            fields,
            keys,
            unique_keys,
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
            let part_link = match (part_page, raw.kind == ir::ControlKind::Part) {
                (Some(pp), true) => match self.part_link(oi, layout, sec, pp) {
                    Ok(l) => Some(l),
                    Err(why) => {
                        unsupported.push(format!("part {}: {why}", raw.name));
                        None
                    }
                },
                _ => None,
            };
            let action_ref = if sec.name().eq_ignore_ascii_case("actionref") {
                Some(match sec.args().get(1) {
                    None => ir::ActionRef::Unresolved("an actionref names no action".into()),
                    Some(t) => match layout.controls.iter().position(|c| {
                        c.kind == ir::ControlKind::Action
                            && !c.section.name().eq_ignore_ascii_case("actionref")
                            && c.name.eq_ignore_ascii_case(t.trim().trim_matches('"'))
                    }) {
                        Some(i) => ir::ActionRef::Target(i),
                        None => {
                            ir::ActionRef::Unresolved(format!("actionref to `{t}`, which is not an action of the page"))
                        }
                    },
                })
            } else {
                None
            };
            let run_object = match self.run_object(oi, sec) {
                Some(r) => {
                    // Which of the two an action runs first is not pinned: no corpus action
                    // declares both.
                    if sec.triggers().any(|t| t.name().eq_ignore_ascii_case("OnAction")) {
                        unsupported.push(format!(
                            "action {} declares both RunObject and OnAction (their order is not pinned)",
                            raw.name
                        ));
                    }
                    Some(r)
                }
                None => None,
            };
            controls.push(ir::Control {
                name: raw.name.clone(),
                kind: raw.kind.clone(),
                parent: raw.parent,
                binding,
                props,
                triggers,
                part_page,
                part_link,
                run_object,
                action_ref,
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

    /// The `RunObject` of action `sec` of page `oi` with its `RunPageLink`, `RunPageView`,
    /// `RunPageMode` and `RunPageOnRec`; `None` for an action without one. What the test client
    /// cannot follow yet becomes `unsupported` (the action is refused when it is invoked).
    fn run_object(&self, oi: usize, sec: &ast::Section) -> Option<ir::RunObject> {
        let prop = |n: &str| {
            sec.properties()
                .find(|p| p.name().eq_ignore_ascii_case(n))
                .map(|p| p.value_text())
        };
        let text = prop("RunObject")?;
        let mut out = ir::RunObject {
            kind: ir::RunKind::Page,
            object: None,
            on_rec: false,
            mode: ir::RunMode::Default,
            link: None,
            unsupported: None,
        };
        let (kind, parts) = match crate::pages::parse_run_object(&text) {
            Ok(k) => k,
            Err(why) => {
                out.unsupported = Some(why);
                return Some(out);
            }
        };
        out.kind = kind;
        match prop("RunPageOnRec").map(|v| v.trim().to_ascii_lowercase()) {
            None => {}
            Some(v) if v == "true" => out.on_rec = true,
            Some(v) if v == "false" => {}
            Some(v) => out.unsupported = Some(format!("RunPageOnRec = {v}")),
        }
        match prop("RunPageMode").map(|v| v.trim().to_ascii_lowercase()) {
            None => {}
            Some(v) if v == "view" => out.mode = ir::RunMode::View,
            Some(v) if v == "edit" => out.mode = ir::RunMode::Edit,
            Some(v) if v == "create" => out.mode = ir::RunMode::Create,
            Some(v) => out.unsupported = Some(format!("RunPageMode = {v}")),
        }
        // The test surface refuses a report, xmlport or query before it looks at the target
        // (corpus handlers/TestPageActionRunObjectKinds_Tests.al:RunObjectNamingAReportIsRefused...,
        // ...KindsPrecompiled_Tests.al: the same for objects outside the workspace), so none of
        // them needs resolving.
        let target = match kind {
            ir::RunKind::Page => ObjKind::Page,
            ir::RunKind::Codeunit => ObjKind::Codeunit,
            ir::RunKind::Report | ir::RunKind::XmlPort | ir::RunKind::Query => return Some(out),
        };
        match self.ws.resolve_ref(self.file, target, &parts) {
            Resolution::Found(t) => out.object = Some(t),
            _ => {
                out.unsupported = Some(format!("RunObject `{}` is outside the workspace", text.trim()));
                return Some(out);
            }
        }
        let link = prop("RunPageLink");
        let view = prop("RunPageView");
        if kind == ir::RunKind::Codeunit {
            if link.is_some() || view.is_some() {
                out.unsupported = Some("RunPageLink or RunPageView on a codeunit target".into());
            }
            return Some(out);
        }
        if link.is_some() || view.is_some() {
            let mut pl = ir::PartLink {
                view: view.map(|v| v.trim().to_string()),
                ..Default::default()
            };
            if let Some(link) = link {
                let entries = (|| {
                    let target_table = self.table_of[out.object.expect("resolved")]
                        .ok_or("RunPageLink on a target page without a source table")?;
                    self.link_entries("RunPageLink", &link, self.table_of[oi], target_table)
                })();
                match entries {
                    Ok(e) => pl.entries = e,
                    Err(why) => out.unsupported = Some(why),
                }
            }
            out.link = Some(pl);
        }
        Some(out)
    }

    /// The entries of a `SubPageLink` or `RunPageLink` text: `what` names the property in errors,
    /// `host_table` is the table `field(...)` entries read, `target_table` the linked page's.
    fn link_entries(
        &self,
        what: &str,
        text: &str,
        host_table: Option<usize>,
        target_table: usize,
    ) -> L<Vec<ir::LinkEntry>> {
        use crate::pages::LinkRhs;
        let field_pos = |t: usize, name: &str| {
            self.members[t]
                .fields
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("{what} names field `{name}`, which table does not have"))
        };
        let mut entries = Vec::new();
        for (left, rhs) in crate::pages::parse_run_page_link(text).map_err(|e| e.replace("RunPageLink", what))? {
            let pf = field_pos(target_table, &left)?;
            entries.push(match rhs {
                LinkRhs::Field(f) => {
                    let ht = host_table
                        .ok_or_else(|| format!("{what} reads a host field, but the host has no source table"))?;
                    ir::LinkEntry::Field(pf, field_pos(ht, &f)?)
                }
                LinkRhs::Filter(t) => ir::LinkEntry::Filter(pf, t),
                LinkRhs::TableId(name) => {
                    let t = self.resolve_obj(ObjKind::Table, &name)?;
                    let id = self.ws.objects[t].id.ok_or("table without an id")?;
                    ir::LinkEntry::Filter(pf, id.to_string())
                }
            });
        }
        Ok(entries)
    }

    /// `SubPageLink`, `SubPageView` and `Provider` of the part control `sec` of page `oi`, which
    /// shows page `part_page`. `Err` is why the link cannot be followed yet.
    fn part_link(
        &self,
        oi: usize,
        layout: &crate::pages::Layout,
        sec: &ast::Section,
        part_page: usize,
    ) -> L<ir::PartLink> {
        let prop = |n: &str| {
            sec.properties()
                .find(|p| p.name().eq_ignore_ascii_case(n))
                .map(|p| p.value_text())
        };
        let mut link = ir::PartLink::default();
        if let Some(pname) = prop("Provider") {
            let pname = pname.trim().trim_matches('"');
            let idx = layout
                .controls
                .iter()
                .position(|c| c.kind == ir::ControlKind::Part && c.name.eq_ignore_ascii_case(pname));
            link.provider = Some(idx.ok_or_else(|| format!("Provider `{pname}` is not a part of the page"))?);
        }
        link.view = prop("SubPageView").map(|v| v.trim().to_string());
        let Some(text) = prop("SubPageLink") else {
            return Ok(link);
        };
        // The host side: the page's own table, or the Provider part's table.
        let host_table = match link.provider {
            Some(pi) => {
                let Some(ppage) = layout.controls[pi]
                    .section
                    .args()
                    .get(1)
                    .and_then(|n| self.resolve_obj(ObjKind::Page, n).ok())
                else {
                    return Err("the Provider part shows a page outside the workspace".into());
                };
                self.table_of[ppage]
            }
            None => self.table_of[oi],
        };
        let part_table = self.table_of[part_page].ok_or("the part page has no source table to link")?;
        link.entries = self.link_entries("SubPageLink", &text, host_table, part_table)?;
        Ok(link)
    }

    /// A FlowField's `CalcFormula` with field names resolved against the table
    /// `oi` it belongs to and object names in the file of `declared_in` (the table or table
    /// extension that wrote it). `Err` is why it cannot be calculated yet.
    fn lower_calc(&self, oi: usize, declared_in: usize, text: &str) -> Result<ir::CalcFormula, String> {
        use crate::calcformula::{CondValue, parse};
        let f = parse(text)?;
        // Names resolve where the field was written: its table or table extension.
        let file = self.ws.objects[declared_in].file;
        let Resolution::Found(src) = self
            .ws
            .resolve_ref(file, ObjKind::Table, std::slice::from_ref(&f.table))
        else {
            return Err(format!(
                "CalcFormula source table `{}` is outside the workspace",
                f.table
            ));
        };
        let src_fields = &self.members[src].fields;
        let src_pos = |name: &str| {
            src_fields
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("CalcFormula field `{name}` is not in table `{}`", f.table))
        };
        let own = &self.members[oi].fields;
        let own_pos = |name: &str| {
            own.iter()
                .position(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("CalcFormula refers to field `{name}`, which the table does not have"))
        };
        // SystemCreatedAt/By and SystemModifiedAt/By are not stamped on writes yet, so a formula
        // reading them would calculate on defaults.
        let unmaintained = |fields: &[crate::members::FieldInfo], pos: usize| {
            if (2_000_000_001..=2_000_000_004).contains(&fields[pos].id) {
                Err(format!(
                    "CalcFormula reads system field {}, which the runtime does not maintain yet",
                    fields[pos].name
                ))
            } else {
                Ok(pos)
            }
        };
        let src_pos = |name: &str| src_pos(name).and_then(|p| unmaintained(src_fields, p));
        let own_pos = |name: &str| own_pos(name).and_then(|p| unmaintained(own, p));
        let field = f.field.as_deref().map(src_pos).transpose()?;
        let mut conds = Vec::new();
        for c in &f.conds {
            let pos = src_pos(&c.field)?;
            let decl = src_fields[pos].declared_in.map_or(file, |d| self.ws.objects[d].file);
            let value = match &c.value {
                CondValue::Const(raw) => ir::CalcValue::Const(self.calc_const(decl, raw, &src_fields[pos].ty)?),
                CondValue::Filter(raw) => ir::CalcValue::Filter(rewrite_quotes(raw)),
                CondValue::Field(n) => ir::CalcValue::Field(own_pos(n)?),
                CondValue::UpperLimit(n) => ir::CalcValue::UpperLimit(own_pos(n)?),
                CondValue::FieldFilter(n) => ir::CalcValue::FieldFilter(own_pos(n)?),
            };
            conds.push(ir::CalcCond { field: pos, value });
        }
        // Two conditions on one source field: only a link replaced by a
        // field(filter()) on the same field is pinned (corpus
        // record/TestCalcFormulaFlowFiltersTests.Codeunit.al:230); anything
        // else is refused rather than guessed.
        for (i, c) in conds.iter().enumerate() {
            for d in &conds[i + 1..] {
                let replaces = matches!(
                    (&c.value, &d.value),
                    (ir::CalcValue::Field(_), ir::CalcValue::FieldFilter(_))
                        | (ir::CalcValue::FieldFilter(_), ir::CalcValue::Field(_))
                );
                if c.field == d.field && !replaces {
                    return Err(format!(
                        "CalcFormula has two conditions on the same source field `{}` (their combination is not pinned yet)",
                        src_fields[c.field].name
                    ));
                }
            }
        }
        Ok(ir::CalcFormula {
            method: f.method,
            negate: f.negate,
            table: src,
            field,
            conds,
        })
    }

    /// A `const(..)` value as a constant of the source field's type.
    fn calc_const(&self, file: usize, raw: &str, ty: &Ty) -> Result<ir::Const, String> {
        calc_const(self.ws, self.members, file, raw, ty)
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
        match self
            .ws
            .resolve_ref(self.file, kind, &crate::types::qualified_parts(name))
        {
            Resolution::Found(i) => Ok(i),
            _ => Err(format!(
                "{} `{}` is outside the workspace",
                kind.label(),
                crate::types::display_name(name)
            )),
        }
    }

    fn is_temporary_table(&self, table: usize) -> bool {
        self.ws.object_ast(table).properties().any(|p| {
            p.name().eq_ignore_ascii_case("TableType") && p.value_text().trim().eq_ignore_ascii_case("temporary")
        })
    }

    fn rt(&self, t: &Ty) -> L<RtType> {
        if let Some(r) = scalar_rt(self.ws, self.file, t) {
            return r;
        }
        Ok(match t {
            Ty::Object(ObjKind::Codeunit, n) => RtType::Codeunit(self.resolve_obj(ObjKind::Codeunit, n)? as u32),
            // An xmlport variable is an instance with its own globals, like a codeunit's.
            Ty::Object(ObjKind::XmlPort, n) => {
                let x = self.resolve_obj(ObjKind::XmlPort, n)?;
                match self.xmlports[x].as_ref().and_then(|p| p.unsupported.first()) {
                    None => RtType::Codeunit(x as u32),
                    Some(why) => return Err(format!("XmlPort \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            // A report variable is an instance with its own globals, like a codeunit's.
            Ty::Object(ObjKind::Report, n) => {
                let r = self.resolve_obj(ObjKind::Report, n)?;
                match self.reports[r].as_ref().and_then(|p| p.unsupported.first()) {
                    None => RtType::Codeunit(r as u32),
                    Some(why) => return Err(format!("Report \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            Ty::Object(ObjKind::Page, n) => {
                let p = self.resolve_obj(ObjKind::Page, n)?;
                match self.page_reasons[p].first() {
                    None => RtType::Page(p as u32),
                    Some(why) => return Err(format!("Page \"{n}\": {why}")),
                }
            }
            Ty::Object(ObjKind::Query, n) => {
                let q = self.resolve_obj(ObjKind::Query, n)?;
                match self.checked.queries[q].as_ref().and_then(|l| l.reasons.first()) {
                    None => RtType::Query(q as u32),
                    Some(why) => return Err(format!("Query \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            Ty::TestPage(n) => {
                let p = self.resolve_obj(ObjKind::Page, n)?;
                match self.page_reasons[p].first() {
                    None => RtType::TestPage(p as u32),
                    Some(why) => return Err(format!("TestPage \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            Ty::Object(ObjKind::Interface, n) => {
                let i = self.resolve_obj(ObjKind::Interface, n)?;
                // Which procedures an extending interface inherits is not pinned by the corpus.
                if self.ws.object_ast(i).extends_parts().is_some() {
                    return Err(format!(
                        "interface \"{}\" extends another interface (`extends` is not lowered yet)",
                        crate::types::display_name(n)
                    ));
                }
                RtType::Interface(i as u32)
            }
            Ty::List(_) | Ty::Dictionary(..) | Ty::Array(..) if holds_interface(t) => {
                return Err("an interface inside a List, Dictionary or array is not lowered yet (not pinned)".into());
            }
            Ty::List(t) => RtType::List(Box::new(self.rt(t)?)),
            Ty::Dictionary(k, v) => RtType::Dictionary(Box::new(self.rt(k)?), Box::new(self.rt(v)?)),
            Ty::Array(d, t) => RtType::Array(d.clone(), Box::new(self.rt(t)?)),
            Ty::Builtin(n) if al_builtins::sysenum::members(n).is_some() => {
                RtType::Option(al_builtins::sysenum::option_members(n).expect("checked"))
            }
            Ty::Builtin(n) => RtType::Library(n.clone()),
            Ty::Record { table, temporary } => {
                let t = self.resolve_obj(ObjKind::Table, table)?;
                // `TableType = Temporary` makes every record of the table temporary (corpus
                // record/TestTempTableContracts.al:16).
                RtType::Record(t as u32, *temporary || self.is_temporary_table(t))
            }
            other => return Err(format!("type {other} is not lowered yet")),
        })
    }

    /// Whether codeunit `cu` lists interface `iface` after `implements`.
    fn implements(&self, cu: usize, iface: usize) -> bool {
        let file = self.ws.objects[cu].file;
        self.ws
            .object_ast(cu)
            .implements_parts()
            .iter()
            .any(|p| matches!(self.ws.resolve_ref(file, ObjKind::Interface, p), Resolution::Found(i) if i == iface))
    }

    /// `value` (of type `src`) converted for a slot of type `target`. An interface slot takes a
    /// codeunit that implements it (corpus codeunit/TestCodeunitInterface.al:14-26) or an enum
    /// that implements it (types/TestEnumDefaultImplTests.Codeunit.al:30-42); everything else
    /// is refused.
    fn convert_to(&self, target: &Ty, src: &Ty, value: ir::Expr) -> L<ir::Expr> {
        let slot = self.rt(target)?;
        let RtType::Interface(iface) = slot else {
            return Ok(ir::Expr::Convert(slot, Box::new(value)));
        };
        let iface = iface as usize;
        let iname = &self.ws.objects[iface].name;
        match src {
            Ty::Object(ObjKind::Interface, n) if self.rt(src)? == slot => {
                let _ = n;
                Ok(ir::Expr::Convert(slot, Box::new(value)))
            }
            Ty::Object(ObjKind::Codeunit, n) => {
                let cu = self.resolve_obj(ObjKind::Codeunit, n)?;
                if !self.implements(cu, iface) {
                    return Err(format!(
                        "codeunit \"{}\" does not implement interface \"{iname}\"",
                        crate::types::display_name(n)
                    ));
                }
                Ok(ir::Expr::Convert(slot, Box::new(value)))
            }
            Ty::Enum(n) => self.enum_impl(n, iface, value),
            other => Err(format!("{other} into interface \"{iname}\" is not lowered yet")),
        }
    }

    /// The cast of an enum value to interface `iface`: per declared ordinal the value's own
    /// `Implementation`, else the enum's `DefaultImplementation`; undeclared ordinals take
    /// `UnknownValueImplementation` (corpus types/TestEnumDefaultImplTests.Codeunit.al:58-100,
    /// types/TestEnumExtImplTests.Codeunit.al:33-76: an enumextension adds values with their own
    /// `Implementation` and falls back to the base enum's default).
    fn enum_impl(&self, enum_name: &str, iface: usize, value: ir::Expr) -> L<ir::Expr> {
        use crate::members::ImplSlot;
        let e = self.resolve_obj(ObjKind::Enum, enum_name)?;
        let iname = &self.ws.objects[iface].name;
        let find = |slot: &ImplSlot| -> L<Option<usize>> {
            for im in self.members[e].enum_impls.iter().filter(|i| i.slot == *slot) {
                let file = self.ws.objects[im.declared_in].file;
                if !matches!(self.ws.resolve_ref(file, ObjKind::Interface, &im.interface), Resolution::Found(i) if i == iface)
                {
                    continue;
                }
                return match self.ws.resolve_ref(file, ObjKind::Codeunit, &im.codeunit) {
                    Resolution::Found(c) => Ok(Some(c)),
                    _ => Err(format!("codeunit `{}` is outside the workspace", im.codeunit.join("."))),
                };
            }
            Ok(None)
        };
        let default = find(&ImplSlot::Default)?;
        let unknown = find(&ImplSlot::Unknown)?;
        let mut by_ordinal = Vec::new();
        for (ordinal, vname) in &self.members[e].enum_values {
            match find(&ImplSlot::Value(*ordinal))?.or(default) {
                Some(c) => by_ordinal.push((*ordinal as i32, c)),
                None => {
                    return Err(format!(
                        "enum \"{enum_name}\" value `{vname}` has no implementation of interface \"{iname}\""
                    ));
                }
            }
        }
        Ok(ir::Expr::EnumImpl {
            value: Box::new(value),
            by_ordinal,
            unknown,
        })
    }

    /// A call of an interface procedure on `receiver`: it runs the same-named procedure of
    /// whichever codeunit the variable holds, so every implementation is a possible target.
    fn interface_call(
        &mut self,
        iface: usize,
        offset: usize,
        receiver: Option<&SyntaxNode>,
        args: &[SyntaxNode],
    ) -> L<ir::Expr> {
        let Some(receiver) = receiver else {
            return Err("an interface procedure called without a receiver is not lowered yet".into());
        };
        if !matches!(self.ty_of(receiver), Ty::Object(ObjKind::Interface, _)) {
            return Err("an interface procedure called on something that is not an interface variable".into());
        }
        let sig = self.members[iface]
            .procedures
            .iter()
            .find(|p| p.offset == offset)
            .ok_or("interface procedure signature")?
            .clone();
        let mut targets = Vec::new();
        for cu in 0..self.ws.objects.len() {
            if self.ws.objects[cu].kind != ObjKind::Codeunit || !self.implements(cu, iface) {
                continue;
            }
            let same_shape: Vec<_> = self.members[cu]
                .procedures
                .iter()
                .filter(|p| {
                    !p.local
                        && p.name.eq_ignore_ascii_case(&sig.name)
                        && p.params.len() == sig.params.len()
                        && p.params.iter().zip(&sig.params).all(|(a, b)| a.by_var == b.by_var)
                })
                .collect();
            let exact = same_shape
                .iter()
                .find(|p| p.params.iter().zip(&sig.params).all(|(a, b)| a.ty == b.ty));
            let Some(imp) = exact.or(same_shape.first()) else {
                return Err(format!(
                    "codeunit \"{}\" implements \"{}\" without a procedure `{}`",
                    self.ws.objects[cu].name, self.ws.objects[iface].name, sig.name
                ));
            };
            let routine = *self.routine_at.get(&(cu, imp.offset)).ok_or("procedure not found")?;
            self.calls.push((cu, routine));
            targets.push((cu, routine));
        }
        if targets.is_empty() {
            return Err(format!(
                "interface \"{}\" has no implementing codeunit in the workspace",
                self.ws.objects[iface].name
            ));
        }
        let receiver = Box::new(self.expr(receiver)?);
        let mut out = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let p = sig.params.get(i).ok_or("too many arguments")?;
            if p.by_var {
                out.push(ir::Arg::Ref(self.place(a)?));
            } else {
                let value = self.expr(a)?;
                out.push(ir::Arg::Value(self.convert_to(&p.ty, &self.ty_of(a), value)?));
            }
        }
        Ok(ir::Expr::InterfaceCall {
            receiver,
            targets,
            args: out,
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
            if p.is_var() && matches!(ty, Ty::Object(ObjKind::Interface, _)) {
                self.reasons.push(format!(
                    "`{}`: a var interface parameter is not lowered yet (not pinned)",
                    p.name()
                ));
            }
            add(self, p.name(), ty, &mut locals);
        }
        let params = locals.len();
        let ret_ty = r.return_type().map(|t| Ty::from_type_ref(&t));
        let ret = match &ret_ty {
            Some(Ty::Object(ObjKind::Interface, _)) => {
                self.reasons
                    .push("return type: an interface return value is not lowered yet (not pinned)".into());
                None
            }
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
            if n.eq_ignore_ascii_case("ErrorBehavior") {
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
        // `FieldRef.Value := x` is the setter form of the `Value` method
        // (corpus recordref/TestRecordRefCRUD.al:20-23): not a place, a builtin call.
        if let Some(Res::Builtin(id)) = self.res(&kids[0]).cloned() {
            let setter = self
                .catalog
                .by_id(&id)
                .is_some_and(|s| s.receiver == "FieldRef" && s.name == "Value");
            if setter && kids[0].kind() == SyntaxKind::MemberExpr {
                if al_builtins::native::lookup(&id).is_none() {
                    return Err(format!("builtin {id} is not implemented"));
                }
                let compound = sig_tokens(n).iter().any(|t| {
                    matches!(
                        t.kind(),
                        SyntaxKind::PlusAssign
                            | SyntaxKind::MinusAssign
                            | SyntaxKind::StarAssign
                            | SyntaxKind::SlashAssign
                    )
                });
                if compound {
                    return Err("compound assignment to FieldRef.Value is not lowered yet".into());
                }
                let recv = kids[0].children().next().ok_or("member base")?;
                let receiver = match self.place(&recv) {
                    Ok(p) => ir::Arg::Ref(p),
                    Err(_) => ir::Arg::Value(self.expr(&recv)?),
                };
                let rhs = self.expr(&kids[1])?;
                return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                    id,
                    receiver: Some(Box::new(receiver)),
                    args: vec![ir::Arg::Value(rhs)],
                    ret_used: false,
                }));
            }
        }
        let place = self.place(&kids[0])?;
        let target_ty = self.ty_of(&kids[0]);
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
        let op_kind = op.map(|t| t.kind());
        let value = match op_kind {
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
        let value = match op_kind {
            Some(SyntaxKind::Assign) | None => self.convert_to(&target_ty, &self.ty_of(&kids[1]), value)?,
            Some(_) => ir::Expr::Convert(self.rt(&target_ty)?, Box::new(value)),
        };
        Ok(ir::Stmt::Assign(place, value))
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
                    let pos = self.readable_field_pos(table, id)?;
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
                    Ty::Builtin(b) if al_builtins::sysenum::members(&b).is_some() => Ok(ir::Expr::Const(
                        ir::Const::Option(i, al_builtins::sysenum::option_members(&b).expect("checked")),
                    )),
                    _ => Err("option value without its option type".into()),
                },
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                _ => Err(format!("`{}` is not lowered yet", n.text())),
            },
            MemberExpr if self.is_control_access(n) => self.control(n),
            // `CurrPage.<Part>.Page`: the page the part shows.
            MemberExpr if self.is_curr_page_part_page(n) => {
                let part = n.children().next().ok_or("part page without a part")?;
                self.control(&part)
            }
            NameRef | MemberExpr | IndexExpr => match self.res(n).cloned() {
                Some(Res::Proc { .. }) | Some(Res::Builtin(_)) => self.call(n, n, &[]),
                // `Q.Amount`: the current row's value of a query column.
                Some(Res::QueryColumn { query, column }) if n.kind() == MemberExpr => {
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(self.expr(&kids[0])?),
                        column,
                    })
                }
                // `Database::A.B."Name"`: a namespace-qualified object id.
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
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

    /// `CurrPage.<Part>.Page`
    fn is_curr_page_part_page(&self, n: &SyntaxNode) -> bool {
        let mut kids = n.children();
        let (Some(part), Some(_)) = (kids.next(), kids.next()) else {
            return false;
        };
        let last_is_page = n
            .children()
            .last()
            .and_then(|m| {
                m.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            })
            .is_some_and(|t| ast::ident_text(&t).eq_ignore_ascii_case("Page"));
        last_is_page
            && matches!(self.ty_of(&part), Ty::TestPart(_))
            && part.kind() == SyntaxKind::MemberExpr
            && part
                .children()
                .next()
                .is_some_and(|r| matches!(self.ty_of(&r), Ty::Object(ObjKind::Page, _)))
    }

    fn control(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        let recv = n.children().next().ok_or("control access without a receiver")?;
        let (Ty::TestPage(page) | Ty::TestPart(page) | Ty::Object(ObjKind::Page, page)) = self.ty_of(&recv) else {
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

    /// (lower-case name, argument constants) of each attribute of the routine
    /// at `offset` in `object`.
    fn routine_attributes(&self, object: usize, offset: usize) -> Attrs {
        if let Some(hit) = self.attrs.borrow().get(&(object, offset)) {
            return hit.clone();
        }
        let attrs = self.routine_attributes_uncached(object, offset);
        self.attrs.borrow_mut().insert((object, offset), attrs.clone());
        attrs
    }

    fn routine_attributes_uncached(&self, object: usize, offset: usize) -> Attrs {
        // The routine starts at `offset`, so it encloses the token there: walk up
        // from it instead of scanning every node of the object.
        let obj = self.ws.object_ast(object);
        let Some(tok) = obj.syntax().token_at_offset((offset as u32).into()).right_biased() else {
            return Vec::new();
        };
        tok.parent_ancestors()
            .filter_map(ast::Routine::cast)
            .find(|r| usize::from(r.syntax().text_range().start()) == offset)
            .map(|r| {
                r.attributes()
                    .map(|a| {
                        let args = a
                            .args()
                            .map(|l| l.children().filter_map(|e| self.constant(&e).ok()).collect())
                            .unwrap_or_default();
                        (a.name().to_ascii_lowercase(), args)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A call resolved on `node` (the CallExpr, or the callee for a call without parentheses).
    fn call(&mut self, node: &SyntaxNode, callee: &SyntaxNode, args: &[SyntaxNode]) -> L<ir::Expr> {
        let receiver_node = (callee.kind() == SyntaxKind::MemberExpr)
            .then(|| callee.children().next())
            .flatten();
        match self.res(node).cloned() {
            Some(Res::Proc { object, offset }) if self.ws.objects[object].kind == ObjKind::Interface => {
                self.interface_call(object, offset, receiver_node.as_ref(), args)
            }
            Some(Res::Proc { object, offset }) => {
                let routine = *self.routine_at.get(&(object, offset)).ok_or("procedure not found")?;
                let sig = self.members[object]
                    .procedures
                    .iter()
                    .find(|p| p.offset == offset)
                    .ok_or("procedure signature")?
                    .clone();
                let instance = match &receiver_node {
                    Some(r)
                        if matches!(
                            self.ty_of(r),
                            Ty::Object(
                                ObjKind::Codeunit | ObjKind::Page | ObjKind::XmlPort | ObjKind::Report,
                                _
                            ) | Ty::Record { .. }
                        ) =>
                    {
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
                        let value = self.expr(a)?;
                        out.push(ir::Arg::Value(self.convert_to(&p.ty, &self.ty_of(a), value)?));
                    }
                }
                self.calls.push((object, routine));
                let attrs = self.routine_attributes(object, offset);
                let try_call = attrs.iter().any(|(a, _)| a == "tryfunction");
                Ok(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args: out,
                    try_call,
                })
            }
            // MaxStrLen reads the declared length of its argument's type, which values do not
            // carry (text/TestTextExtended.al MaxStrLen_*: 50 for Text[50], 2147483647 for Text).
            Some(Res::Builtin(id)) if id.starts_with("Text.MaxStrLen#") => {
                let arg = args.first().ok_or("MaxStrLen without an argument")?;
                match self.ty_of(arg) {
                    Ty::Text(Some(n)) | Ty::Code(Some(n)) => Ok(ir::Expr::Const(ir::Const::Integer(n as i32))),
                    Ty::Text(None) | Ty::Code(None) => Ok(ir::Expr::Const(ir::Const::Integer(i32::MAX))),
                    other => Err(format!("MaxStrLen of a value of type {other}")),
                }
            }
            // ArrayLen reads the shape of its argument's type, which array values do not keep
            // (streams/TestArrayStreamContracts.al ArrayLen_*).
            Some(Res::Builtin(id)) if id.starts_with("System.ArrayLen#") => {
                let arg = args.first().ok_or("ArrayLen without an argument")?;
                let Ty::Array(dims, _) = self.ty_of(arg) else {
                    return Err("ArrayLen of something that is not an array variable".into());
                };
                let dim = match args.get(1) {
                    Some(d) => match self.expr(d)? {
                        ir::Expr::Const(ir::Const::Integer(k)) => Some(k),
                        _ => return Err("ArrayLen with a dimension that is not a constant".into()),
                    },
                    None if dims.len() == 1 => Some(1),
                    None => return Err("ArrayLen without a dimension on a multi-dimensional array (not pinned)".into()),
                };
                match dim.and_then(|k| usize::try_from(k - 1).ok()).and_then(|k| dims.get(k)) {
                    Some(n) => Ok(ir::Expr::Const(ir::Const::Integer(*n as i32))),
                    None => Err("ArrayLen with a dimension outside the array (error text not pinned)".into()),
                }
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
                    // `CurrReport` is the report whose code is running: the VM takes the running
                    // instance when a report builtin has no receiver.
                    Some(r)
                        if sig.instance
                            && sig.receiver == "Report"
                            && self.ws.objects[self.oi].kind == ObjKind::Report
                            && r.text().to_string().trim().eq_ignore_ascii_case("CurrReport") =>
                    {
                        None
                    }
                    // Enum metadata calls (`"Name".Names()`, `Var.Ordinals()`, `"Name".FromInteger(n)`)
                    // receive the enum itself: its object index, as an enum constant.
                    Some(r) if sig.receiver == "Enum" && sig.name != "AsInteger" => {
                        let Ty::Enum(name) = self.ty_of(r) else {
                            return Err(format!("{id}: the receiver is not an enum type"));
                        };
                        let object = self.resolve_obj(ObjKind::Enum, &name)?;
                        Some(Box::new(ir::Arg::Value(ir::Expr::Const(ir::Const::Enum(
                            object as u32,
                            0,
                        )))))
                    }
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
                    let field_param = p.name.to_ascii_lowercase().starts_with("field")
                        || p.name.eq_ignore_ascii_case("FromField")
                        || p.name.to_ascii_lowercase().starts_with("column");
                    match (field_param, self.res(a).cloned()) {
                        (true, Some(Res::Field { table, id })) => {
                            let pos = self.readable_field_pos(table, id)?;
                            out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(pos as u32))));
                            continue;
                        }
                        // A query column is named by its position in the query's column list.
                        (true, Some(Res::QueryColumn { column, .. })) => {
                            out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(column as u32))));
                            continue;
                        }
                        _ => {}
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
    fn event_publisher_calls_lower_including_isolated_ones() {
        let (_, l) = lower_src(
            "codeunit 50100 C {
                [IntegrationEvent(false, false)] procedure OnA() begin end;
                [BusinessEvent(false)] procedure OnB() begin end;
                [InternalEvent(false)] procedure OnC() begin end;
                [IntegrationEvent(false, false, true)] procedure OnIso() begin end;
                [BusinessEvent(false, true)] procedure OnIsoB() begin end;
                procedure Plain() begin OnA(); OnB(); OnC(); end;
                procedure Iso() begin OnIso(); OnIsoB(); end;
            }",
        );
        let reasons = |n: &str| l.unsupported[0][routine(&l, n).0].clone();
        assert!(reasons("Plain").is_empty(), "{:?}", reasons("Plain"));
        assert!(reasons("Iso").is_empty(), "{:?}", reasons("Iso"));
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

    const FLOW_TABLES: &str = "
enum 50910 \"ALT Kind\" { value(0; \" \") { } value(1; \"Initial Entry\") { } value(5; Application) { } }
table 50911 \"ALT Line\"
{
    fields
    {
        field(1; \"Entry No.\"; Integer) { }
        field(2; \"Doc No.\"; Code[20]) { }
        field(3; Amount; Decimal) { }
        field(4; Kind; Enum \"ALT Kind\") { }
        field(5; St; Option) { OptionMembers = Open,Released,Closed; }
        field(6; Open; Boolean) { }
        field(7; \"Table ID\"; Integer) { }
    }
}
table 50912 \"ALT Head\"
{
    fields
    {
        field(1; \"No.\"; Code[20]) { }
        field(2; \"Date Filter\"; Date) { FieldClass = FlowFilter; }
        field(3; Totaling; Text[250]) { }
        field(10; Total; Decimal) { FieldClass = FlowField; CalcFormula = sum(\"ALT Line\".Amount where(\"Doc No.\" = field(\"No.\"))); }
        field(11; Neg; Integer) { FieldClass = FlowField; CalcFormula = -count(\"ALT Line\" where(\"Doc No.\" = field(\"No.\"), Open = const(true), St = const(Closed), \"Table ID\" = const(Database::\"ALT Line\"))); }
        field(12; Quoted; Decimal) { FieldClass = FlowField; CalcFormula = sum(\"ALT Line\".Amount where(Kind = filter(\"Initial Entry\" | Application))); }
        field(13; Limit; Decimal) { FieldClass = FlowField; CalcFormula = sum(\"ALT Line\".Amount where(\"Doc No.\" = field(\"No.\"), Kind = const(\"Initial Entry\"))); }
        field(14; Lim2; Decimal) { FieldClass = FlowField; CalcFormula = sum(\"ALT Line\".Amount where(\"Entry No.\" = field(upperlimit(\"Date Filter\")), \"Doc No.\" = field(filter(Totaling)))); }
        field(15; NoFormula; Decimal) { FieldClass = FlowField; }
        field(16; Outside; Integer) { FieldClass = FlowField; CalcFormula = count(Date where(\"Period Type\" = const(Date))); }
        field(17; BadField; Integer) { FieldClass = FlowField; CalcFormula = count(\"ALT Line\" where(Nope = const(1))); }
        field(18; Both; Integer) { FieldClass = FlowField; CalcFormula = count(\"ALT Line\" where(Open = const(true), Open = const(false))); }
    }
}";

    fn flow(l: &Lowered, table: &str, field: &str) -> ir::Field {
        let o = l.program.objects.iter().find(|o| o.name == table).unwrap();
        o.table
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == field)
            .unwrap_or_else(|| panic!("{field}"))
            .clone()
    }

    #[test]
    fn flow_filters_and_flow_fields_are_marked_and_lowered() {
        let (_, l) = lower_src(FLOW_TABLES);
        let df = flow(&l, "ALT Head", "Date Filter");
        assert!(df.flow_filter && !df.stored && df.calc.is_none());
        let t = flow(&l, "ALT Head", "Total");
        assert!(!t.flow_filter && !t.stored);
        // Source table "ALT Line" is object 1 (enum 0, line 1, head 2); Amount is position 2.
        assert_eq!(
            t.calc,
            Some(ir::CalcFormula {
                method: ir::CalcMethod::Sum,
                negate: false,
                table: 1,
                field: Some(2),
                conds: vec![ir::CalcCond {
                    field: 1,
                    value: ir::CalcValue::Field(0)
                }],
            })
        );
        assert_eq!(t.calc_refusal, None);
    }

    #[test]
    fn calc_formula_consts_are_typed_as_the_source_field() {
        let (_, l) = lower_src(FLOW_TABLES);
        let n = flow(&l, "ALT Head", "Neg");
        let f = n.calc.clone().unwrap_or_else(|| panic!("{:?}", n.calc_refusal));
        assert!(f.negate && f.method == ir::CalcMethod::Count && f.field.is_none());
        let consts: Vec<_> = f.conds[1..].iter().map(|c| c.value.clone()).collect();
        assert_eq!(
            consts,
            vec![
                ir::CalcValue::Const(ir::Const::Boolean(true)),
                ir::CalcValue::Const(ir::Const::Option(
                    2,
                    vec!["Open".into(), "Released".into(), "Closed".into()]
                )),
                // Database::"ALT Line" is that table's object id.
                ir::CalcValue::Const(ir::Const::Integer(50911)),
            ]
        );
        let k = flow(&l, "ALT Head", "Limit").calc.expect("lowered");
        assert_eq!(k.conds[1].value, ir::CalcValue::Const(ir::Const::Enum(0, 1)));
    }

    #[test]
    fn calc_formula_filters_rewrite_double_quotes_and_flow_references_resolve() {
        let (_, l) = lower_src(FLOW_TABLES);
        let q = flow(&l, "ALT Head", "Quoted").calc.expect("lowered");
        assert_eq!(
            q.conds[0].value,
            ir::CalcValue::Filter("'Initial Entry' | Application".into())
        );
        let m = flow(&l, "ALT Head", "Lim2").calc.expect("lowered");
        // "Date Filter" is position 1 of ALT Head, Totaling position 2.
        assert_eq!(m.conds[0].value, ir::CalcValue::UpperLimit(1));
        assert_eq!(m.conds[1].value, ir::CalcValue::FieldFilter(2));
    }

    #[test]
    fn unusable_formulas_carry_a_reason_and_a_missing_formula_carries_none() {
        let (_, l) = lower_src(FLOW_TABLES);
        let nf = flow(&l, "ALT Head", "NoFormula");
        assert!(nf.calc.is_none() && nf.calc_refusal.is_none() && !nf.stored && !nf.flow_filter);
        let out = flow(&l, "ALT Head", "Outside");
        assert!(out.calc.is_none());
        assert!(
            out.calc_refusal.as_deref().unwrap().contains("Date"),
            "{:?}",
            out.calc_refusal
        );
        let bad = flow(&l, "ALT Head", "BadField");
        assert!(
            bad.calc_refusal.as_deref().unwrap().contains("Nope"),
            "{:?}",
            bad.calc_refusal
        );
        let both = flow(&l, "ALT Head", "Both");
        assert!(
            both.calc_refusal.as_deref().unwrap().contains("same source field"),
            "{:?}",
            both.calc_refusal
        );
    }

    // The runtime stamps SystemRowVersion and SystemId on writes but not the created/modified
    // stamps, so a formula that reads them would calculate on defaults.
    #[test]
    fn formulas_over_unmaintained_system_fields_are_refused_but_system_id_and_row_version_are_not() {
        let src = "table 50920 L { fields { field(1; K; Integer) { } field(2; Ref; Guid) { } } }
table 50921 H { fields { field(1; K; Integer) { }
    field(2; A; DateTime) { FieldClass = FlowField; CalcFormula = max(L.SystemCreatedAt where(K = field(K))); }
    field(3; B; Guid) { FieldClass = FlowField; CalcFormula = lookup(L.SystemModifiedBy where(K = field(K))); }
    field(4; C; Integer) { FieldClass = FlowField; CalcFormula = count(L where(SystemCreatedBy = field(SystemId))); }
    field(5; D; Integer) { FieldClass = FlowField; CalcFormula = count(L where(Ref = field(SystemId))); }
    field(6; E; BigInteger) { FieldClass = FlowField; CalcFormula = max(L.SystemRowVersion where(K = field(K))); } } }";
        let (_, l) = lower_src(src);
        for name in ["A", "B", "C"] {
            let f = flow(&l, "H", name);
            assert!(
                f.calc.is_none() && f.calc_refusal.as_deref().unwrap_or("").contains("System"),
                "{name}: {:?}",
                f.calc_refusal
            );
        }
        for name in ["D", "E"] {
            let f = flow(&l, "H", name);
            assert!(f.calc.is_some(), "{name}: {:?}", f.calc_refusal);
        }
    }

    // Reading SystemCreatedAt would give the default DateTime, not the time of the insert.
    #[test]
    fn routines_reading_unstamped_system_fields_are_refused() {
        let src = format!(
            "codeunit 50100 C {{ procedure Stamped() var R: Record \"ALT T\"; D: DateTime; begin D := R.SystemModifiedAt; end;
             procedure Id() var R: Record \"ALT T\"; G: Guid; begin G := R.SystemId; end; }}\n{TABLE}"
        );
        let (_, l) = lower_src(&src);
        let (stamped, _) = routine(&l, "Stamped");
        let (id, _) = routine(&l, "Id");
        let oi = 0;
        assert!(
            l.unsupported[oi][stamped]
                .iter()
                .any(|r| r.contains("SystemModifiedAt")),
            "{:?}",
            l.unsupported[oi][stamped]
        );
        assert!(l.unsupported[oi][id].is_empty(), "{:?}", l.unsupported[oi][id]);
    }

    // corpus _fixtures/ALTRelationScopeExtension.al: a name in a tableextension's CalcFormula means
    // what it means in the extension's file (own namespace first), not in the extended table's.
    #[test]
    fn a_table_extension_formula_resolves_names_in_the_extensions_file() {
        let files = vec![
            (
                "a.al".to_string(),
                "namespace N1;
                table 70001 Pkg { fields { field(1; Code; Code[10]) { } } }
                table 70000 Base { fields { field(1; Code; Code[10]) { } } }"
                    .to_string(),
            ),
            (
                "b.al".to_string(),
                "namespace N2;
                using N1;
                table 70002 Pkg { fields { field(1; Code; Code[10]) { } } }
                tableextension 70003 Ext extends Base { fields {
                    field(5; Rows; Integer) { FieldClass = FlowField; CalcFormula = count(Pkg where(Code = field(Code))); } } }"
                    .to_string(),
            ),
        ];
        let (_, l) = crate::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files,
            }],
            &Default::default(),
        );
        let local = l.program.objects.iter().position(|o| o.id == Some(70002)).unwrap();
        let f = flow(&l, "Base", "Rows");
        assert_eq!(f.calc.map(|c| c.table), Some(local), "{:?}", f.calc_refusal);
    }

    #[test]
    fn namespace_qualified_object_types_resolve_in_the_workspace() {
        // tableextensiontrigger/TestTableExtTargetNamespace_Tests.al:58 declares
        // `Record ALLanguage.Coverage.TxnLocal."Shipment Method"`, a workspace table.
        let files = vec![
            (
                "t.al".to_string(),
                "namespace A.B; table 70010 \"Ship. Method\" { fields { field(1; Code; Code[10]) { } } }".to_string(),
            ),
            (
                "c.al".to_string(),
                "codeunit 70011 C { procedure P() var R: Record A.B.\"Ship. Method\"; begin end;
                    procedure Q(): Integer begin exit(Database::A.B.\"Ship. Method\"); end; }"
                    .to_string(),
            ),
        ];
        let (_, l) = crate::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files,
            }],
            &Default::default(),
        );
        let c = l.program.objects.iter().position(|o| o.id == Some(70011)).unwrap();
        let p = l.program.objects[c]
            .routines
            .iter()
            .position(|r| r.name == "P")
            .unwrap();
        assert!(l.unsupported[c][p].is_empty(), "{:?}", l.unsupported[c][p]);
        // TestTableExtTargetNamespace_Tests.al: `Database::ALLanguage.Coverage.TxnLocal."Shipment Method"`.
        let q = l.program.objects[c]
            .routines
            .iter()
            .position(|r| r.name == "Q")
            .unwrap();
        assert!(l.unsupported[c][q].is_empty(), "{:?}", l.unsupported[c][q]);
        assert!(format!("{:?}", l.program.objects[c].routines[q].body).contains("Integer(70010)"));
    }

    #[test]
    fn option_fields_carry_their_option_members() {
        // types/TestOption.al:Option_Format_ReturnsName -- Format(option=1) is 'Draft'.
        let (_, l) = lower_src(
            "table 50901 K { fields {
                field(1; A; Integer) { }
                field(2; O; Option) { OptionMembers = \" \",Draft,\"In Work\"; }
                field(3; P; Option) { OptionMembers = ,Alpha; }
                field(4; Q; Option) { }
            } }",
        );
        let t = table_ir(&l);
        assert_eq!(
            t.fields[1].ty,
            RtType::Option(vec![" ".into(), "Draft".into(), "In Work".into()])
        );
        assert_eq!(t.fields[2].ty, RtType::Option(vec!["".into(), "Alpha".into()]));
        assert_eq!(t.fields[3].ty, RtType::Option(vec!["".into()]));
    }

    #[test]
    fn audit_system_fields_are_refused_until_they_are_written() {
        // record/TestBCSystemFieldContracts.al needs a clock and the user's security id for
        // SystemCreatedAt/By and SystemModifiedAt/By; neither exists yet, and a default
        // value would silently pass for a populated one.
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{
                procedure Read() var R: Record \"ALT T\"; D: DateTime; begin D := R.SystemModifiedAt; end;
                procedure Filter() var R: Record \"ALT T\"; begin R.SetRange(SystemCreatedBy); end;
                procedure Ok() var R: Record \"ALT T\"; G: Guid; begin G := R.SystemId; end;
            }}"
        );
        let (_, l) = lower_src(&src);
        let find = |n: &str| l.program.objects[1].routines.iter().position(|r| r.name == n).unwrap();
        assert_eq!(
            l.unsupported[1][find("Read")],
            vec!["system field SystemModifiedAt is not stamped by the runtime yet".to_string()]
        );
        assert_eq!(
            l.unsupported[1][find("Filter")],
            vec!["system field SystemCreatedBy is not stamped by the runtime yet".to_string()]
        );
        assert!(l.unsupported[1][find("Ok")].is_empty());
    }

    #[test]
    fn a_removed_key_takes_no_index_position() {
        // corpus recordref/TestKeyMetadataRowVersionRemoved.al:10-20: KeyIndex() skips the
        // Removed key's position while a Pending key stays.
        let (_, l) = lower_src(
            "table 50901 R { fields { field(1; A; Integer) { } field(2; B; Integer) { } field(3; C; Integer) { } }
             keys { key(PK; A) { } key(P; B) { ObsoleteState = Pending; ObsoleteReason = 'x'; ObsoleteTag = '1'; }
                    key(Gone; B) { ObsoleteState = Removed; ObsoleteReason = 'x'; ObsoleteTag = '1'; } key(Live; C) { } } }",
        );
        assert_eq!(table_ir(&l).keys, vec![vec![0], vec![1], vec![2]]);
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
