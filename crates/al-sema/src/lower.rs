//! Lowering resolved syntax to the typed IR (ADR-011). Anything the IR or the
//! builtins registry cannot express yet becomes a reason on the routine
//! (`Routine::unsupported` in `Lowered`), never a guess.

use crate::check::{CheckResult, Res};
use crate::members::MemberTable;
use crate::types::Ty;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_builtins::value::RtType;
use al_ir as ir;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use std::collections::HashMap;

/// Wall time of the parts of a compile (`--timings`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CompileTimings {
    /// Reading the apps: parsing the workspace's files, scanning the dependencies'.
    pub load: std::time::Duration,
    pub members: std::time::Duration,
    pub check: crate::check::CheckTimings,
    pub lower: std::time::Duration,
    /// With the compile cache: checking the cache against the apps and reading its blocks.
    pub cache_blocks: std::time::Duration,
}

pub struct Lowered {
    /// Zero unless the compile ran through `compile_apps_with`.
    pub timings: CompileTimings,
    /// Shared with the engine that runs it: the program is big, and copying it cost as much as
    /// lowering the workspace's own code.
    pub program: std::sync::Arc<ir::Program>,
    /// Per object, per routine: why it cannot run (empty = it can).
    pub unsupported: Vec<Vec<Vec<String>>>,
    /// Per object, per routine: the routines it calls directly, as (object, routine).
    pub calls: Vec<Vec<Vec<(usize, usize)>>>,
    /// Per object: it belongs to a dependency app, so no test or install code of it runs (ADR-013).
    pub dependency: Vec<bool>,
    /// Per object: its lowered form came from a compile cache (`provided.rs`). `calls` holds only
    /// the direct calls of such an object; `calls_of` adds the rest.
    pub provided: Vec<bool>,
    /// What a variable of a page, report or xmlport can drive (see `Driven`); set with `provided`.
    driven: Option<DrivenCtx>,
    /// With `lower_for_cache`: the lowered dependency objects and what the lowering of others
    /// reads about them, ready to be stored.
    pub exported: Option<crate::provided::DepSet>,
    /// The diagnostics with the AL compiler's own codes (`alcodes.rs`) that the check found in the
    /// workspace's apps. Code that BC rejects; the compile goes on.
    pub al_diagnostics: Vec<crate::workspace::Diagnostic>,
}

/// The routines a variable of a page, report or xmlport type can run besides the ones the code
/// calls: a test page drives every routine of its page, the page's extensions and its parts.
#[derive(Clone)]
pub(crate) struct DrivenCtx {
    page_extensions: HashMap<usize, Vec<usize>>,
    page_parts: Vec<Vec<usize>>,
    page_runs: Vec<Vec<usize>>,
    /// Per object: an xmlport or report, whose triggers Import, Export and Run call.
    runs_triggers: Vec<bool>,
    report_extension: Vec<Vec<usize>>,
    /// Per object: whether it belongs to a dependency app.
    dependency: Vec<bool>,
    /// Per object: whether it is a codeunit (a `RunObject` target that runs its `OnRun`).
    codeunit: Vec<bool>,
    /// Per object: whether it is a report, report extension, xmlport or query. Using one reaches
    /// its triggers and the routines made for its properties, not its other procedures.
    triggers_only: Vec<bool>,
    /// Per object and routine: the `ROLE_*` bits of the routine. Structure only: the same for
    /// every compile of the same sources, whatever of them was compiled.
    routine_roles: Vec<Vec<u8>>,
}

/// Routine roles (`DrivenCtx::routine_roles`).
pub(crate) const ROLE_TRIGGER: u8 = 1;
/// A routine `pages::expand` generated for a control property (its name starts `__`).
pub(crate) const ROLE_SYNTHETIC: u8 = 2;
pub(crate) const ROLE_ON_RUN: u8 = 4;

/// The role bits of a routine.
pub(crate) fn role_of(r: &ast::Routine) -> u8 {
    let name = r.name();
    let mut role = 0;
    if r.is_trigger() {
        role |= ROLE_TRIGGER;
        if name.eq_ignore_ascii_case("OnRun") {
            role |= ROLE_ON_RUN;
        }
    }
    if name.starts_with("__") {
        role |= ROLE_SYNTHETIC;
    }
    role
}

impl Lowered {
    /// The routines routine `r` of object `o` can call: the ones its code names and the ones a
    /// variable of a page, report or xmlport type drives.
    pub fn calls_of(&self, o: usize, r: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        self.calls_into(o, r, &mut out);
        out
    }

    /// [`calls_of`](Self::calls_of), appended to `out`.
    pub fn calls_into(&self, o: usize, r: usize, out: &mut Vec<(usize, usize)>) {
        out.extend(self.calls[o][r].iter().copied());
        if !self.provided.get(o).copied().unwrap_or(false) {
            return;
        }
        if let Some(d) = &self.driven {
            let object = &self.program.objects[o];
            let routine = &object.routines[r];
            driven_edges(d, routine.locals.iter().chain(&object.globals), out);
        }
    }
}

/// The routines driven by the slots of a routine (its locals) and its object (its globals).
///
/// The edges are a function of the sources alone, never of how much of them was compiled
/// (owner decision of 2026-10-14: the demand-driven, the complete and the cached compile give one
/// outcome). The demand-driven compile reaches exactly these routines for the same slots
/// (`check/demand.rs`: `activate` for a page, report or xmlport that is used, `reach_run_objects`
/// for the `RunObject` of a workspace page), so that an edge never leads to a routine that was
/// not compiled.
fn driven_edges<'a>(d: &DrivenCtx, slots: impl Iterator<Item = &'a ir::Slot>, out: &mut Vec<(usize, usize)>) {
    let with_role = |q: usize, mask: u8, out: &mut Vec<(usize, usize)>| {
        out.extend(
            d.routine_roles[q]
                .iter()
                .enumerate()
                .filter(|(_, role)| **role & mask != 0)
                .map(|(ri, _)| (q, ri)),
        );
    };
    // The routines using object `q` reaches: all of a page's, the triggers of a report's.
    let used = |q: usize, out: &mut Vec<(usize, usize)>| {
        let mask = if d.triggers_only[q] {
            ROLE_TRIGGER | ROLE_SYNTHETIC
        } else {
            u8::MAX
        };
        out.extend(
            d.routine_roles[q]
                .iter()
                .enumerate()
                .filter(|(_, role)| mask == u8::MAX || **role & mask != 0)
                .map(|(ri, _)| (q, ri)),
        );
    };
    for s in slots {
        // `CurrPage` is the page's handle on itself, not a variable that drives it.
        if s.name == "CurrPage" {
            continue;
        }
        if let RtType::TestPage(p) | RtType::Page(p) = s.ty {
            let p = p as usize;
            // The demand-driven compile reaches the code of a `Page` variable's page when the
            // variable runs it (`Page.Run`), not when it is declared, so a dependency page
            // that a `Page` variable names adds no edge. A workspace page is always compiled.
            if matches!(s.ty, RtType::Page(_)) && d.dependency[p] {
                continue;
            }
            // The page and, transitively, the pages it shows as parts.
            let mut seen = vec![p];
            let mut at = 0;
            while at < seen.len() {
                let q = seen[at];
                at += 1;
                used(q, out);
                // The extensions' triggers and procedures run with the page.
                for &e in d.page_extensions.get(&q).map(|v| v.as_slice()).unwrap_or(&[]) {
                    used(e, out);
                }
                for &part in &d.page_parts[q] {
                    if !seen.contains(&part) {
                        seen.push(part);
                    }
                }
                // What the `RunObject` actions of workspace pages run: a page like a part, a
                // codeunit by its `OnRun`. Those of a dependency page are compiled (the test can
                // invoke the action, `Demand::reach_run_targets`) but add no edge: nothing says
                // that the test invokes it, and the code behind it refuses when it runs.
                if d.dependency[q] {
                    continue;
                }
                for &t in &d.page_runs[q] {
                    if d.codeunit[t] {
                        with_role(t, ROLE_ON_RUN, out);
                    } else if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
            }
        }
        // An xmlport's triggers are run by Import and Export, and a report's by Run, which
        // the graph cannot see.
        if let RtType::Codeunit(p) = s.ty {
            let p = p as usize;
            if d.runs_triggers[p] {
                with_role(p, ROLE_TRIGGER | ROLE_SYNTHETIC, out);
            }
            for &e in &d.report_extension[p] {
                with_role(e, ROLE_TRIGGER | ROLE_SYNTHETIC, out);
            }
        }
    }
}

impl Lowered {
    /// The program, to change: copies it only if an engine already holds it.
    pub fn program_mut(&mut self) -> &mut ir::Program {
        std::sync::Arc::make_mut(&mut self.program)
    }
}

pub fn lower(ws: &Workspace, members: &MemberTable<'_>, checked: &CheckResult) -> Lowered {
    lower_with_positions(ws, members, checked, true)
}

/// `lower`, choosing whether statements carry source positions (`ir::Stmt::Pos`). Unit tests
/// that check the shape of lowered bodies turn them off.
pub fn lower_with_positions(
    ws: &Workspace,
    members: &MemberTable<'_>,
    checked: &CheckResult,
    positions: bool,
) -> Lowered {
    lower_with_workers(ws, members, checked, positions, lower_workers())
}

/// `lower_with_positions` with `workers` threads lowering the objects of dependency apps next to
/// the one that lowers the workspace's. The result does not depend on the number.
pub fn lower_with_workers(
    ws: &Workspace,
    members: &MemberTable<'_>,
    checked: &CheckResult,
    positions: bool,
    workers: usize,
) -> Lowered {
    lower_core(ws, members, checked, positions, workers, None, false).expect("no cache, no cache error")
}

/// What a compile cache supplies to a run (see `provided.rs`).
pub(crate) struct Provided<'a> {
    pub src: &'a dyn crate::provided::DepSource,
    /// The blocks of the objects that are supplied, read and checked.
    pub blocks: Vec<Option<crate::provided::DepBlock>>,
}

impl Provided<'_> {
    /// Whether object `oi` comes from the cache.
    fn has(&self, oi: usize) -> bool {
        self.blocks.get(oi).is_some_and(Option::is_some)
    }
}

/// The routine numbering of the objects: routine `r` of object `o` starts at `offsets[o][r]`.
/// Offsets ascend, so the number of a routine is found by search.
#[derive(Clone)]
pub struct RoutineAt(pub std::sync::Arc<Vec<Vec<usize>>>);

impl RoutineAt {
    pub fn get(&self, key: &(usize, usize)) -> Option<usize> {
        self.0.get(key.0)?.binary_search(&key.1).ok()
    }
}

pub(crate) fn lower_core(
    ws: &Workspace,
    members: &MemberTable<'_>,
    checked: &CheckResult,
    positions: bool,
    workers: usize,
    provided: Option<Provided<'_>>,
    export: bool,
) -> Result<Lowered, String> {
    let catalog = al_builtins::catalog::Catalog::load();
    let (src, mut blocks): (
        Option<&dyn crate::provided::DepSource>,
        Vec<Option<crate::provided::DepBlock>>,
    ) = match provided {
        Some(p) => (Some(p.src), p.blocks),
        None => (None, Vec::new()),
    };
    let flags: Vec<bool> = (0..ws.objects.len())
        .map(|oi| blocks.get(oi).is_some_and(Option::is_some))
        .collect();
    let prov = |oi: usize| flags[oi];
    // With dependency apps the checker found what the workspace reaches in them (ADR-013);
    // the rest is not compiled and no longer resolves.
    let demand = checked.demand.as_ref();
    if let Some(d) = demand {
        ws.restrict_to((0..ws.objects.len()).map(|i| d.active(i)).collect());
    }
    if let (Some(d), Some(path)) = (demand, std::env::var_os("AL_DUMP_REACH")) {
        // Debug aid: what the demand reached, one line per object (level) and routine.
        let mut lines: Vec<String> = Vec::new();
        for (i, o) in ws.objects.iter().enumerate() {
            if ws.is_dependency(i) && d.active(i) {
                lines.push(format!("{} {} level {}", o.kind.label(), o.name, d.level[i]));
            }
        }
        for &(i, off) in &d.reached {
            lines.push(format!(
                "{} {} routine {off}",
                ws.objects[i].kind.label(),
                ws.objects[i].name
            ));
        }
        lines.sort();
        let _ = std::fs::write(path, lines.join("\n"));
    }
    let active = |oi: usize| demand.is_none_or(|d| !ws.is_dependency(oi) || d.active(oi));
    let uncompiled_subscribers = if demand.is_some() {
        crate::check::demand::uncompiled_subscribers(ws, &active)
    } else {
        Vec::new()
    };
    // Routine numbering: every procedure and trigger node of an object, in source order. The
    // nodes of the objects this thread lowers stay in `nodes`; the start offset of every routine
    // is what other threads need, so it is asked of a node once.
    let active_v: Vec<bool> = (0..ws.objects.len()).map(active).collect();
    // Each thread builds the nodes of the objects it takes and keeps only the offsets.
    let (routine_offsets, routine_roles): (Vec<Vec<usize>>, Vec<Vec<u8>>) = {
        use rayon::prelude::*;
        let both: Vec<(Vec<usize>, Vec<u8>)> = (0..ws.objects.len())
            .into_par_iter()
            .map(|oi| {
                if prov(oi) {
                    let f = src.expect("provided").facts(oi);
                    return (
                        f.routine_offsets.iter().map(|&o| o as usize).collect(),
                        f.routine_roles.clone(),
                    );
                }
                if !active_v[oi] {
                    return (Vec::new(), Vec::new());
                }
                ast::routines_in(ws.object_ast(oi).syntax())
                    .iter()
                    .map(|r| (usize::from(r.syntax().text_range().start()), role_of(r)))
                    .unzip()
            })
            .collect();
        both.into_iter().unzip()
    };
    let routine_offsets = std::sync::Arc::new(routine_offsets);
    let routine_at = RoutineAt(routine_offsets.clone());
    let mut program = ir::Program {
        objects: Vec::new(),
        apps: ws
            .apps
            .iter()
            .enumerate()
            .map(|(i, a)| ir::AppInfo {
                id: a.id.clone(),
                name: a.name.clone(),
                publisher: String::new(),
                version: String::new(),
                dependencies: a.dependencies.clone(),
                resources: Vec::new(),
                dependency: ws.kinds[i] == crate::workspace::AppKind::Dependency,
                include_source: false,
            })
            .collect(),
        dotnet_types: ws
            .dotnet
            .iter()
            .map(|d| ir::DotNetDecl {
                alias: d.decl.alias.clone(),
                assembly: d.decl.assembly.clone(),
                version: d.decl.version.clone(),
                culture: d.decl.culture.clone(),
                public_key_token: d.decl.public_key_token.clone(),
                type_name: d.decl.type_name.clone(),
            })
            .collect(),
        uncompiled_subscribers,
        ..Default::default()
    };
    let mut out = Lowered {
        timings: CompileTimings::default(),
        program: Default::default(),
        unsupported: Vec::new(),
        calls: Vec::new(),
        dependency: (0..ws.objects.len()).map(|oi| ws.is_dependency(oi)).collect(),
        provided: (0..ws.objects.len()).map(prov).collect(),
        driven: None,
        exported: None,
        al_diagnostics: checked
            .diagnostics
            .iter()
            .filter(|d| d.code.starts_with("AL"))
            .cloned()
            .collect(),
    };
    if positions {
        program.sources = ws
            .files
            .iter()
            .map(|f| ir::SourceFile {
                app: f.app as u32,
                path: f.path.clone(),
                lines: Vec::new(),
            })
            .collect();
    }
    // The table whose record is the implicit `Rec` of each object, and each table's extensions.
    let mut table_of: Vec<Option<usize>> = vec![None; ws.objects.len()];
    let mut extensions: HashMap<usize, Vec<usize>> = HashMap::new();
    // Pages: the control tree, the source table, and what the client cannot drive yet.
    let mut layouts: Vec<Option<crate::pages::Layout>> = (0..ws.objects.len()).map(|_| None).collect();
    let mut page_temporary: Vec<bool> = vec![false; ws.objects.len()];
    let mut page_extensions: HashMap<usize, Vec<usize>> = HashMap::new();
    for (oi, o) in ws.objects.iter().enumerate() {
        if prov(oi) {
            continue;
        }
        if o.kind == ObjKind::PageExtension && active(oi) {
            if let Some(base) = ws.object_ast(oi).extends_parts() {
                if let Resolution::Found(p) = ws.resolve_ref(o.file, ObjKind::Page, &base) {
                    page_extensions.entry(p).or_default().push(oi);
                }
            }
        }
        // A reportextension that extends the request page merges into it the way a page extension
        // merges into its page (corpus handlers/TestRequestPageControlErrorName.al).
        if o.kind == ObjKind::ReportExtension && active(oi) {
            let ext = ws.object_ast(oi);
            if ext.sections().any(|s| s.name().eq_ignore_ascii_case("requestpage")) {
                if let Some(base) = ext.extends_parts() {
                    if let Resolution::Found(r) = ws.resolve_ref(o.file, ObjKind::Report, &base) {
                        page_extensions.entry(r).or_default().push(oi);
                    }
                }
            }
        }
    }
    // Extensions apply dependencies first, then by object id: when two modify one property, the
    // later one wins (corpus handlers/TestPageExtensionModifyCaption_Tests.al: a higher object id wins
    // within an app, `Twice2Ctl`; the app that depends on the other wins whatever the ids, `FxBoth2Ctl`).
    for list in page_extensions.values_mut() {
        list.sort_by_key(|&e| (ws.app_depth(ws.objects[e].app), ws.objects[e].id, ws.objects[e].offset));
    }
    let mut page_reasons: Vec<Vec<String>> = vec![Vec::new(); ws.objects.len()];
    // The page objects each page shows as parts.
    let mut page_parts: Vec<Vec<usize>> = vec![Vec::new(); ws.objects.len()];
    // The pages and codeunits each page's RunObject actions run.
    let mut page_runs: Vec<Vec<usize>> = vec![Vec::new(); ws.objects.len()];
    // What the cache supplied about the objects it supplies.
    let usize_of = |v: &[u32]| -> Vec<usize> { v.iter().map(|&i| i as usize).collect() };
    if let Some(src) = src {
        for oi in (0..ws.objects.len()).filter(|&oi| prov(oi)) {
            let f = src.facts(oi);
            table_of[oi] = f.table_of.map(|t| t as usize);
            page_temporary[oi] = f.page_temporary;
            page_reasons[oi] = f.page_reasons.clone();
            page_parts[oi] = usize_of(&f.page_parts);
            page_runs[oi] = usize_of(&f.page_runs);
            if !f.page_extensions.is_empty() {
                page_extensions.insert(oi, usize_of(&f.page_extensions));
            }
        }
    }
    for (oi, o) in ws.objects.iter().enumerate() {
        if o.kind != ObjKind::Page || !active(oi) || prov(oi) {
            continue;
        }
        let obj = ws.object_ast(oi);
        let exts: Vec<(usize, ast::Object)> = page_extensions
            .get(&oi)
            .map(|v| v.iter().map(|&e| (e, ws.object_ast(e))).collect())
            .unwrap_or_default();
        let l = crate::pages::layout_with(&obj, &exts);
        page_reasons[oi].extend(l.reasons.iter().cloned());
        page_reasons[oi].extend(extension_reasons(&exts));
        for c in l.controls.iter().filter(|c| c.kind == ir::ControlKind::Part) {
            // A control an extension declares names its page the way the extension's file does.
            let file = ws.objects[c.owner.unwrap_or(oi)].file;
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
            match crate::types::section_object_name(&c.section, 1) {
                Some(n) => match ws.resolve_ref(file, ObjKind::Page, &crate::types::qualified_parts(&n)) {
                    Resolution::Found(p) => page_parts[oi].push(p),
                    _ => page_reasons[oi].push(format!(
                        "part {} shows page {} outside the workspace",
                        c.name,
                        crate::types::display_name(&n)
                    )),
                },
                None => page_reasons[oi].push(format!("part {} names no page", c.name)),
            }
        }
        // The pages and codeunits the actions run: their routines are reached through the host
        // (a RunObject target runs when the action is invoked, which the call graph cannot see).
        for c in &l.controls {
            // How a `modify` block's RunObject would combine with the control's own is not pinned.
            if c.modifies
                .iter()
                .any(|(_, m)| m.properties().any(|p| p.name().eq_ignore_ascii_case("RunObject")))
            {
                page_reasons[oi].push(format!(
                    "modify({}) sets RunObject in a page extension (not supported yet)",
                    c.name
                ));
            }
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
                if let Some(Resolution::Found(t)) =
                    found.map(|k| ws.resolve_ref(ws.objects[c.owner.unwrap_or(oi)].file, k, &parts))
                {
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
                _ => page_reasons[oi].push(format!("source table: {}", ws.outside_workspace(ObjKind::Table, &t))),
            }
        }
        page_temporary[oi] = prop("SourceTableTemporary").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        // The values a SaveValues page stores are those of its variable-bound controls (corpus
        // handlers/TestPageSaveValues.al). An extension control bound to `Rec.Field` adds none, so
        // the stored set is the page's own. One bound to a variable is a control whose stored value
        // no corpus test pins (docs/probes-needed.md, "SaveValues with page extensions").
        if prop("SaveValues").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
            && layouts[oi].iter().flat_map(|l| l.controls.iter()).any(|c| {
                c.owner.is_some()
                    && c.kind == ir::ControlKind::Field
                    && !c.section.name().eq_ignore_ascii_case("label")
                    && c.section.args().get(1).is_some_and(|src| {
                        !matches!(
                            crate::pages::classify_source(src, &std::collections::BTreeSet::new()),
                            crate::pages::Source::RecField(_)
                        ) && crate::pages::plain_name(src).is_some()
                    })
            })
        {
            page_reasons[oi].push(
                "SaveValues on a page that a page extension extends with a control bound to a variable: whether BC stores that variable is not pinned (docs/probes-needed.md)".into(),
            );
        }
    }
    // A page extension shares the page's `Rec`.
    for (&p, list) in &page_extensions {
        for &e in list {
            table_of[e] = table_of[p];
            page_temporary[e] = page_temporary[p];
        }
    }
    // A host cannot be driven where a part page cannot.
    for _ in 0..3 {
        for oi in 0..ws.objects.len() {
            for &p in &page_parts[oi].clone() {
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
            _ if prov(oi) => {}
            // `Rec` of a codeunit with TableNo (a RunObject action hands it the host's record).
            ObjKind::Codeunit if active(oi) => {
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
            ObjKind::TableExtension if active(oi) => {
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
        if o.kind != ObjKind::XmlPort || !active(oi) || prov(oi) {
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
        // Every reportextension of a report that is compiled counts, whether or not it was
        // compiled itself: what changes a run is decided by the installed apps' sources, never by
        // what a selection reached (issue 11). The demand-driven compile activates an extension with
        // its report, so the two agree today; this keeps them from drifting.
        if o.kind != ObjKind::ReportExtension || prov(oi) {
            continue;
        }
        if !active(oi) && !ws.extension_target(oi).is_some_and(&active) {
            continue;
        }
        if !ws.usable(oi) {
            continue;
        }
        let ext = ws.object_ast(oi);
        // The routines `pages::expand` adds for expression-valued control properties and sources of the
        // request page are not the author's: they do not change a run.
        let changes_a_run = ext.routines().any(|r| !crate::pages::is_synthetic(&r.name()))
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
        if o.kind != ObjKind::Report || !active(oi) || prov(oi) {
            continue;
        }
        let obj = ws.object_ast(oi);
        let l = crate::reports::layout(&obj);
        let mut slots = HashMap::new();
        for i in &l.items {
            let next = slots.len();
            slots.entry(i.name.to_lowercase()).or_insert(next);
        }
        // The request page: its controls and the data items a handler can filter. Why the test
        // client cannot drive it yet becomes a reason on the page, not on the report: a run
        // that never opens the page (UseRequestPage = false) does not need it.
        let exts: Vec<(usize, ast::Object)> = page_extensions
            .get(&oi)
            .map(|v| v.iter().map(|&e| (e, ws.object_ast(e))).collect())
            .unwrap_or_default();
        let (request, request_section) = crate::reports::request_layout(&obj, &l.items, &exts);
        page_reasons[oi].extend(request.reasons.iter().cloned());
        // A request page that declares `SourceTable` has a `Rec` of that table (corpus
        // handlers/TestReportRequestPageSourceTableCompiledContracts.al).
        if let Some(t) = request_section
            .as_ref()
            .and_then(|s| s.properties().find(|p| p.name().eq_ignore_ascii_case("SourceTable")))
            .map(|p| p.value_text())
        {
            match ws.resolve_ref(o.file, ObjKind::Table, &[t.trim().trim_matches('"').to_string()]) {
                Resolution::Found(t) => table_of[oi] = Some(t),
                _ => page_reasons[oi].push(format!("source table: {}", ws.outside_workspace(ObjKind::Table, &t))),
            }
        }
        // Its variables would follow the base items' records in the extension's globals, which the
        // slots the request page binds to do not account for.
        for (e, _) in &exts {
            if changing.get(&oi).is_some_and(|c| c.contains(e)) {
                page_reasons[oi].push(format!(
                    "reportextension `{}` changes the run and extends the request page (not supported yet)",
                    ws.objects[*e].name
                ));
            }
        }
        layouts[oi] = Some(request);
        // Several extensions apply in object id order (corpus reportextensiontrigger/
        // TestReportExtensionBaseMembersContracts.al:256, ADR-025). Their order across apps is not
        // pinned.
        let mut exts = changing.get(&oi).cloned().unwrap_or_default();
        exts.sort_by_key(|&e| (ws.objects[e].id, e));
        let extension_reasons = if exts.len() > 1
            && (exts.iter().any(|&e| ws.objects[e].id.is_none())
                || exts.iter().any(|&e| ws.objects[e].app != ws.objects[exts[0]].app))
        {
            vec![
                "reportextensions of different apps change the run (their order across apps is not pinned)".to_string(),
            ]
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
            extensions: exts.clone(),
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
    // Why a variable of an xmlport or report is refused, and the report extension that changes a
    // run: the cache supplies them for its objects.
    let n_objects = ws.objects.len();
    let mut xml_reasons: Vec<Option<String>> = (0..n_objects)
        .map(|oi| xml_irs[oi].as_ref().and_then(|p| p.unsupported.first().cloned()))
        .collect();
    let mut report_reasons: Vec<Option<String>> = (0..n_objects)
        .map(|oi| report_irs[oi].as_ref().and_then(|p| p.unsupported.first().cloned()))
        .collect();
    let mut report_extension: Vec<Vec<usize>> = (0..n_objects)
        .map(|oi| {
            report_irs[oi]
                .as_ref()
                .map(|r| r.extensions.clone())
                .unwrap_or_default()
        })
        .collect();
    if let Some(src) = src {
        for oi in (0..n_objects).filter(|&oi| prov(oi)) {
            let f = src.facts(oi);
            xml_reasons[oi] = f.xml_reason.clone();
            report_reasons[oi] = f.report_reason.clone();
            report_extension[oi] = f.report_extensions.iter().map(|&e| e as usize).collect();
        }
    }
    let driven = DrivenCtx {
        page_extensions,
        page_parts,
        page_runs,
        runs_triggers: ws
            .objects
            .iter()
            .map(|o| matches!(o.kind, ObjKind::XmlPort | ObjKind::Report))
            .collect(),
        report_extension,
        dependency: (0..n_objects).map(|oi| ws.is_dependency(oi)).collect(),
        codeunit: ws.objects.iter().map(|o| o.kind == ObjKind::Codeunit).collect(),
        triggers_only: ws
            .objects
            .iter()
            .map(|o| {
                matches!(
                    o.kind,
                    ObjKind::Report | ObjKind::ReportExtension | ObjKind::XmlPort | ObjKind::Query
                )
            })
            .collect(),
        routine_roles,
    };
    // From here on objects are lowered one at a time, each a function of the data above.
    let mut shared = Shared {
        ws,
        members,
        checked,
        catalog: &catalog,
        demand,
        positions,
        active: active_v,
        table_of,
        page_temporary,
        page_reasons,
        driven,
        xml_irs,
        report_irs,
        xml_reasons,
        report_reasons,
        ext_irs,
        ext_slots: Vec::new(),
        routine_offsets,
        routine_at,
        extensions,
        ext_base,
        export,
    };
    // The global slot of each variable of a page extension, by lower-case name. The page's control
    // tree is built before the extensions are lowered, and binds their controls to these slots.
    let mut ext_slot_table: Vec<HashMap<String, usize>> = vec![HashMap::new(); ws.objects.len()];
    {
        let local = Local::new(Nodes::new(ws, shared.active.clone()));
        let seq = Seq::new(&shared);
        for exts in shared.driven.page_extensions.values() {
            for &e in exts {
                if prov(e) {
                    continue;
                }
                let lw = make_lowerer(&shared, &local, &seq, e);
                let mut slots = HashMap::new();
                for v in ws.object_ast(e).var_decls() {
                    let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
                    for n in v.names() {
                        if lw.rt(&ty).is_ok() {
                            let next = slots.len();
                            slots.insert(n.to_lowercase(), next);
                        }
                    }
                }
                ext_slot_table[e] = slots;
            }
        }
    }
    if let Some(src) = src {
        for e in (0..n_objects).filter(|&e| prov(e)) {
            ext_slot_table[e] = src
                .facts(e)
                .ext_slots
                .iter()
                .map(|(n, i)| (n.clone(), *i as usize))
                .collect();
        }
    }
    shared.ext_slots = ext_slot_table;
    let n = ws.objects.len();
    // Worker threads and this one take the active objects in turn, the workspace's own first (they
    // are the biggest). Each object is lowered with its source positions numbered from 0; they
    // are moved to their place in the program's table below, in object order, so the numbering is
    // the one a single thread would have made.
    let mut theirs: Vec<usize> = (0..n)
        .filter(|&oi| shared.active[oi] && !ws.is_dependency(oi) && !prov(oi))
        .collect();
    theirs.extend((0..n).filter(|&oi| shared.active[oi] && ws.is_dependency(oi) && !prov(oi)));
    let workers = workers.min(theirs.len() / PARALLEL_OBJECTS_PER_WORKER);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let take_some = |local: &Local<'_>, seq: &Seq<'_>| -> Vec<(usize, Piece)> {
        let mut out = Vec::new();
        let mut line_cache: HashMap<usize, std::rc::Rc<FileLines>> = HashMap::new();
        loop {
            let k = next.fetch_add(LOWER_CHUNK, std::sync::atomic::Ordering::Relaxed);
            if k >= theirs.len() {
                return out;
            }
            for &oi in &theirs[k..(k + LOWER_CHUNK).min(theirs.len())] {
                out.push((oi, lower_object(&shared, local, seq, oi, &mut line_cache)));
            }
        }
    };
    let mut pieces: Vec<Option<Piece>> = (0..n).map(|_| None).collect();
    let local = Local::new(Nodes::new(ws, shared.active.clone()));
    let seq = Seq::new(&shared);
    let done: Vec<Vec<(usize, Piece)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(LOWER_STACK)
                    .spawn_scoped(scope, || {
                        let local = Local::new(Nodes::new(ws, shared.active.clone()));
                        let seq = Seq::new(&shared);
                        take_some(&local, &seq)
                    })
                    .expect("spawn a lowering worker")
            })
            .collect();
        let mut done = vec![take_some(&local, &seq)];
        done.extend(
            handles
                .into_iter()
                .map(|h| h.join().expect("a lowering worker panicked")),
        );
        done
    });
    for (oi, piece) in done.into_iter().flatten() {
        pieces[oi] = Some(piece);
    }
    // Objects the cache supplies.
    {
        for (oi, slot) in blocks.iter_mut().enumerate() {
            if let Some(b) = slot.take() {
                pieces[oi] = Some(Piece {
                    object: b.object,
                    positions: Vec::new(),
                    unsupported: b.unsupported,
                    calls: b
                        .calls
                        .iter()
                        .map(|r| r.iter().map(|&(o, r)| (o as usize, r as usize)).collect())
                        .collect(),
                    direct: Vec::new(),
                    embeds: Vec::new(),
                });
            }
        }
    }
    // Objects nothing compiled are shells.
    let mut line_cache: HashMap<usize, std::rc::Rc<FileLines>> = HashMap::new();
    #[allow(clippy::needless_range_loop)] // `oi` is the object, not just a slot of `pieces`
    for oi in 0..n {
        if pieces[oi].is_none() {
            pieces[oi] = Some(lower_object(&shared, &local, &seq, oi, &mut line_cache));
        }
    }
    let mut exported = export.then(crate::provided::DepSet::default);
    let embeds_of_all: Vec<Vec<usize>> = pieces
        .iter()
        .map(|p| p.as_ref().map_or_else(Vec::new, |p| p.embeds.clone()))
        .collect();
    for (oi, mut piece) in pieces.into_iter().flatten().enumerate() {
        if let Some(set) = exported.as_mut() {
            set.blocks.push(crate::provided::DepBlock {
                object: piece.object.clone(),
                unsupported: piece.unsupported.clone(),
                calls: piece
                    .direct
                    .iter()
                    .map(|r| r.iter().map(|&(o, r)| (o as u32, r as u32)).collect())
                    .collect(),
            });
            let o = &ws.objects[oi];
            let recorded: std::collections::BTreeSet<usize> = embeds_of_all[oi].iter().copied().collect();
            set.facts.push(crate::provided::DepFacts {
                kind: Some(o.kind),
                id: o.id,
                name: o.name.clone(),
                app: o.app as u32,
                embeds: crate::provided::embeds_of(oi, &recorded, &|p| shared.driven.page_parts[p].clone()),
                table_of: shared.table_of[oi].map(|t| t as u32),
                page_temporary: shared.page_temporary[oi],
                page_reasons: shared.page_reasons[oi].clone(),
                page_parts: shared.driven.page_parts[oi].iter().map(|&p| p as u32).collect(),
                page_runs: shared.driven.page_runs[oi].iter().map(|&p| p as u32).collect(),
                page_extensions: shared
                    .driven
                    .page_extensions
                    .get(&oi)
                    .map(|v| v.iter().map(|&p| p as u32).collect())
                    .unwrap_or_default(),
                ext_slots: {
                    let mut v: Vec<(String, u32)> = shared.ext_slots[oi]
                        .iter()
                        .map(|(n, &i)| (n.clone(), i as u32))
                        .collect();
                    v.sort();
                    v
                },
                routine_offsets: shared.routine_offsets[oi].iter().map(|&o| o as u32).collect(),
                routine_roles: shared.driven.routine_roles[oi].clone(),
                xml_reason: shared.xml_reasons[oi].clone(),
                report_reason: shared.report_reasons[oi].clone(),
                report_extensions: shared.driven.report_extension[oi].iter().map(|&e| e as u32).collect(),
                query: checked.queries[oi].clone(),
            });
        }
        if !piece.positions.is_empty() {
            let base = program.positions.len() as u32;
            for r in &mut piece.object.routines {
                r.shift_positions(base);
            }
            program.positions.append(&mut piece.positions);
        }
        program.objects.push(piece.object);
        out.unsupported.push(piece.unsupported);
        out.calls.push(piece.calls);
    }
    out.exported = exported;
    if src.is_some() {
        out.driven = Some(shared.driven.clone());
    }
    out.program = std::sync::Arc::new(program);
    Ok(out)
}

/// The lowering of one object: its IR, why each routine cannot run, and the routines each calls.
struct Piece {
    object: ir::Object,
    /// The source positions of its statements, numbered from 0.
    positions: Vec<ir::SrcPos>,
    unsupported: Vec<Vec<String>>,
    calls: Vec<Vec<(usize, usize)>>,
    /// With `Shared::export`: `calls` without what a page, report or xmlport variable drives.
    direct: Vec<Vec<(usize, usize)>>,
    /// The objects whose extensions or implementers the lowering read (see `provided.rs`).
    embeds: Vec<usize>,
}

/// How many threads lower dependency objects. `AL_LOWER_THREADS` sets it; the default is the
/// number of cores.
fn lower_workers() -> usize {
    std::env::var("AL_LOWER_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, 32)
}

/// A thread is started for at least this many objects.
const PARALLEL_OBJECTS_PER_WORKER: usize = 16;
/// Objects a lowering worker takes at a time.
const LOWER_CHUNK: usize = 4;
/// Stack of a lowering worker: lowering recurses on the nesting of the code.
const LOWER_STACK: usize = 64 << 20;

/// What the lowering of an object reads that is plain data, built once and read by every thread.
struct Shared<'a> {
    ws: &'a Workspace,
    members: &'a MemberTable<'a>,
    checked: &'a CheckResult,
    catalog: &'a al_builtins::catalog::Catalog,
    demand: Option<&'a crate::check::demand::Demand>,
    /// Statements carry source positions (`lower_with_positions`).
    positions: bool,
    /// Per object: it is part of the compile (see `Workspace::restrict_to`).
    active: Vec<bool>,
    /// The table whose record is the implicit `Rec` of each object.
    table_of: Vec<Option<usize>>,
    page_temporary: Vec<bool>,
    page_reasons: Vec<Vec<String>>,
    /// What a variable of a page, report or xmlport type drives: the page extensions of each
    /// page, the parts and actions of each page, the report extension of each report.
    driven: DrivenCtx,
    xml_irs: Vec<Option<ir::XmlPort>>,
    report_irs: Vec<Option<ir::Report>>,
    /// Per object: the first reason a variable of the xmlport or report is refused.
    xml_reasons: Vec<Option<String>>,
    report_reasons: Vec<Option<String>>,
    ext_irs: Vec<Option<ir::ReportExt>>,
    /// The global slot of each variable of a page extension, by lower-case name.
    ext_slots: Vec<HashMap<String, usize>>,
    /// The start offset of every routine, numbered as `Nodes` numbers them.
    routine_offsets: std::sync::Arc<Vec<Vec<usize>>>,
    routine_at: RoutineAt,
    /// The extensions of each table.
    extensions: HashMap<usize, Vec<usize>>,
    /// For a report extension that changes a run, the report it belongs to.
    ext_base: Vec<Option<usize>>,
    /// Keep what a cache needs (`Lowered::exported`).
    export: bool,
}

/// The routine nodes of the objects, built when first asked for. A syntax tree belongs to the
/// thread that made it, so each thread has its own.
struct Nodes<'a> {
    ws: &'a Workspace,
    active: Vec<bool>,
    cells: Vec<std::cell::OnceCell<Vec<ast::Routine>>>,
}

impl<'a> Nodes<'a> {
    fn new(ws: &'a Workspace, active: Vec<bool>) -> Nodes<'a> {
        Nodes {
            ws,
            active,
            cells: (0..ws.objects.len()).map(|_| std::cell::OnceCell::new()).collect(),
        }
    }

    /// The procedures and triggers of object `oi` in source order (none if it is not compiled).
    fn get(&self, oi: usize) -> &[ast::Routine] {
        self.cells[oi].get_or_init(|| {
            if self.active[oi] {
                ast::routines_in(self.ws.object_ast(oi).syntax())
            } else {
                Vec::new()
            }
        })
    }
}

/// What one thread keeps while it lowers objects.
struct Local<'a> {
    nodes: Nodes<'a>,
    memo: Memo,
}

impl<'a> Local<'a> {
    fn new(nodes: Nodes<'a>) -> Local<'a> {
        Local {
            nodes,
            memo: Memo::default(),
        }
    }
}

/// The layouts of pages, report request pages, XML ports and report extensions, built from the
/// syntax tree of an object when first asked for. The prologue of `lower_with_positions` builds
/// the same layouts to find what the objects need of each other; a layout holds syntax nodes,
/// which belong to the thread that made them, so each thread builds the ones it reads.
struct Seq<'a> {
    ws: &'a Workspace,
    active: &'a [bool],
    page_extensions: &'a HashMap<usize, Vec<usize>>,
    ext_base: &'a [Option<usize>],
    page: Vec<std::cell::OnceCell<Option<crate::pages::Layout>>>,
    xml: Vec<std::cell::OnceCell<Option<crate::xmlports::Layout>>>,
    report: Vec<std::cell::OnceCell<Option<crate::reports::Layout>>>,
    ext: Vec<std::cell::OnceCell<Option<crate::reports::ExtLayout>>>,
}

impl<'a> Seq<'a> {
    fn new(sh: &'a Shared<'a>) -> Seq<'a> {
        fn cells<T>(n: usize) -> Vec<std::cell::OnceCell<T>> {
            (0..n).map(|_| std::cell::OnceCell::new()).collect()
        }
        let n = sh.ws.objects.len();
        Seq {
            ws: sh.ws,
            active: &sh.active,
            page_extensions: &sh.driven.page_extensions,
            ext_base: &sh.ext_base,
            page: cells(n),
            xml: cells(n),
            report: cells(n),
            ext: cells(n),
        }
    }

    /// The extensions of page or report `oi` with their syntax trees.
    fn extensions(&self, oi: usize) -> Vec<(usize, ast::Object)> {
        self.page_extensions
            .get(&oi)
            .map(|v| v.iter().map(|&e| (e, self.ws.object_ast(e))).collect())
            .unwrap_or_default()
    }

    /// The control tree of a page; of a report, its request page.
    fn page_layout(&self, oi: usize) -> Option<&crate::pages::Layout> {
        self.page[oi]
            .get_or_init(|| match self.ws.objects[oi].kind {
                ObjKind::Page if self.active[oi] => {
                    Some(crate::pages::layout_with(&self.ws.object_ast(oi), &self.extensions(oi)))
                }
                ObjKind::Report if self.active[oi] => {
                    let l = self.report(oi)?;
                    Some(crate::reports::request_layout(&self.ws.object_ast(oi), &l.items, &self.extensions(oi)).0)
                }
                _ => None,
            })
            .as_ref()
    }

    fn xml(&self, oi: usize) -> Option<&crate::xmlports::Layout> {
        self.xml[oi]
            .get_or_init(|| {
                (self.ws.objects[oi].kind == ObjKind::XmlPort && self.active[oi])
                    .then(|| crate::xmlports::layout(&self.ws.object_ast(oi)))
            })
            .as_ref()
    }

    fn report(&self, oi: usize) -> Option<&crate::reports::Layout> {
        self.report[oi]
            .get_or_init(|| {
                (self.ws.objects[oi].kind == ObjKind::Report && self.active[oi])
                    .then(|| crate::reports::layout(&self.ws.object_ast(oi)))
            })
            .as_ref()
    }

    /// The report a report extension that changes a run belongs to.
    fn ext_base(&self, oi: usize) -> Option<usize> {
        self.ext_base[oi]
    }

    fn ext(&self, oi: usize) -> Option<&crate::reports::ExtLayout> {
        self.ext[oi]
            .get_or_init(|| self.ext_base[oi].map(|_| crate::reports::ext_layout(&self.ws.object_ast(oi))))
            .as_ref()
    }
}

fn make_lowerer<'a>(sh: &'a Shared<'a>, tl: &'a Local<'a>, seq: &'a Seq<'a>, oi: usize) -> Lowerer<'a> {
    Lowerer {
        rec_slot: None,
        table_of: &sh.table_of,
        ws: sh.ws,
        members: sh.members,
        checked: sh.checked,
        catalog: sh.catalog,
        routine_at: &sh.routine_at,
        oi,
        file: sh.ws.objects[oi].file,
        globals: HashMap::new(),
        locals: HashMap::new(),
        global_labels: HashMap::new(),
        local_labels: HashMap::new(),
        reasons: Vec::new(),
        calls: Vec::new(),
        page_reasons: &sh.page_reasons,
        layouts: seq,
        xml_reasons: &sh.xml_reasons,
        report_reasons: &sh.report_reasons,
        memo: &tl.memo,
        ret_ty: None,
        ret_decl: None,
        routine_nodes: &tl.nodes,
        ext_slots: &sh.ext_slots,
        page_extensions: &sh.driven.page_extensions,
        embeds: Default::default(),
        positions: Vec::new(),
        lines: None,
        emit_pos: false,
        routine_idx: 0,
        assumed: Vec::new(),
    }
}

/// Lowers object `oi`. `positions` and `line_cache` are the source positions and file line tables
/// of the objects lowered so far on this thread.
fn lower_object<'a>(
    sh: &'a Shared<'a>,
    tl: &'a Local<'a>,
    seq: &'a Seq<'a>,
    oi: usize,
    line_cache: &mut HashMap<usize, std::rc::Rc<FileLines>>,
) -> Piece {
    let ws = sh.ws;
    let members = sh.members;
    let checked = sh.checked;
    let demand = sh.demand;
    let table_of = &sh.table_of;
    let page_temporary = &sh.page_temporary;
    let page_extensions = &sh.driven.page_extensions;
    let page_reasons = &sh.page_reasons;
    let xml_irs = &sh.xml_irs;
    let report_irs = &sh.report_irs;
    let ext_irs = &sh.ext_irs;
    let routine_at = &sh.routine_at;
    let routine_offsets = &sh.routine_offsets;
    let extensions = &sh.extensions;
    let o = &ws.objects[oi];
    if !sh.active[oi] {
        // Not compiled: a shell keeps object numbers stable. Its kind is `uncompiled table` and
        // so on, so nothing the runtime looks up by kind and id (`Codeunit.Run(80)`) finds it
        // and runs it as an empty object; the few lookups that would answer "no such object"
        // (`RecordRef.Open(18)`) look for the shell and refuse instead.
        return Piece {
            object: ir::Object {
                kind: format!("uncompiled {}", o.kind.label()),
                id: o.id,
                name: o.name.clone(),
                globals: Vec::new(),
                routines: Vec::new(),
                // What the declaration scan read of the object body, so the metadata tables can
                // answer for an object nothing compiled.
                properties: o.scanned_properties.clone(),
                enum_values: Vec::new(),
                enum_captions: Vec::new(),
                table: None,
                rec_slot: None,
                xrec_slot: None,
                page: None,
                app: o.app as u32,
                query: None,
                xmlport: None,
                report: None,
                report_ext: None,
                namespace: o.namespace.clone(),
                extends_id: ws.extension_target(oi).and_then(|t| ws.objects[t].id),
                layouts: o
                    .scanned_layouts
                    .iter()
                    .map(|l| ir::LayoutDecl {
                        name: l.name.clone(),
                        properties: l.properties.clone(),
                    })
                    .collect(),
            },
            positions: Vec::new(),
            unsupported: Vec::new(),
            calls: Vec::new(),
            direct: Vec::new(),
            embeds: Vec::new(),
        };
    }
    let obj = ws.object_ast(oi);
    let mut lw = make_lowerer(sh, tl, seq, oi);
    // Positions only feed call stacks and error lines; for a dependency app they cost a third of
    // lowering and no corpus test reads them, so its routines carry none (measured in ADR-013).
    lw.lines = (sh.positions && !ws.is_dependency(oi)).then(|| {
        line_cache
            .entry(o.file)
            .or_insert_with(|| std::rc::Rc::new(FileLines::new(ws, o.file)))
            .clone()
    });
    let mut globals = Vec::new();
    let mut global_reasons = Vec::new();
    if let Some(l) = seq.xml(oi) {
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
    if let Some(l) = seq.report(oi) {
        for i in &l.items {
            let key = i.name.to_lowercase();
            if lw.globals.contains_key(&key) {
                continue;
            }
            let ty = lw.rt(&Ty::Record {
                table: i.table.clone(),
                temporary: crate::reports::uses_temporary(&i.section),
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
    if let (Some(base), Some(l)) = (seq.ext_base(oi), seq.ext(oi)) {
        let base_items = seq.report(base).map_or(&[][..], |b| &b.items[..]);
        for i in base_items.iter().chain(&l.items) {
            let key = i.name.to_lowercase();
            if lw.globals.contains_key(&key) {
                continue;
            }
            let ty = lw.rt(&Ty::Record {
                table: i.table.clone(),
                temporary: crate::reports::uses_temporary(&i.section),
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
            // A codeunit has no xRec.
            if name == "xRec" && o.kind == ObjKind::Codeunit {
                continue;
            }
            *slot = Some(globals.len());
            lw.globals.insert(name.to_lowercase(), globals.len());
            globals.push(ir::Slot {
                name: name.into(),
                ty: RtType::Record(t as u32, lw.is_temporary_table(t) || page_temporary[oi]),
            });
        }
    }
    // A query's own code runs on the query it was called on: its state sits in a hidden slot
    // the VM fills when it makes the instance (`Session::query_instance`).
    if o.kind == ObjKind::Query {
        rec_slot = Some(globals.len());
        // `CurrQuery` in the query's triggers and procedures
        // (query/TestQueryOnBeforeOpenVariablesContracts.al:75-77).
        lw.globals.insert("currquery".into(), globals.len());
        globals.push(ir::Slot {
            name: "__query".into(),
            ty: RtType::Query(oi as u32),
        });
    }
    lw.rec_slot = rec_slot;
    if o.kind == ObjKind::Report {
        // `CurrPage` of the request page. Typed Variant: the test client puts the page there
        // when it opens, and AL code cannot name it.
        globals.push(ir::Slot {
            name: "CurrPage".into(),
            ty: RtType::Variant,
        });
    }
    // The page an extension extends, when it is in the workspace.
    let extended_page = (o.kind == ObjKind::PageExtension)
        .then(|| page_extensions.iter().find(|(_, v)| v.contains(&oi)).map(|(&p, _)| p))
        .flatten();
    if o.kind == ObjKind::Page || extended_page.is_some() {
        lw.globals.insert("currpage".into(), globals.len());
        globals.push(ir::Slot {
            name: "CurrPage".into(),
            ty: RtType::TestPage(extended_page.unwrap_or(oi) as u32),
        });
    }
    if extended_page.is_some() {
        debug_assert!(
            sh.ext_slots[oi]
                .iter()
                .all(|(n, &i)| globals.get(i).is_some_and(|g| g.name.eq_ignore_ascii_case(n))),
            "the slots of a page extension's variables moved"
        );
    }
    let mut routines = Vec::new();
    let mut unsupported = Vec::new();
    let mut calls = Vec::new();
    let mut direct = Vec::new();
    for (ri, r) in tl.nodes.get(oi).iter().enumerate() {
        lw.routine_idx = ri;
        lw.reasons = global_reasons.clone();
        lw.calls.clear();
        // A trigger of a data item runs with the item's record as its `Rec`.
        if matches!(o.kind, ObjKind::Report | ObjKind::ReportExtension) {
            lw.rec_slot = r
                .syntax()
                .ancestors()
                .filter_map(ast::Section::cast)
                .find(|s| {
                    ["dataitem", "modify", "add"]
                        .iter()
                        .any(|k| s.name().eq_ignore_ascii_case(k))
                })
                .and_then(|s| s.args().into_iter().next())
                .and_then(|n| lw.globals.get(&n.to_lowercase()).copied());
            // A trigger of the request page runs with the page's `Rec`.
            if lw.rec_slot.is_none()
                && o.kind == ObjKind::Report
                && r.syntax()
                    .ancestors()
                    .filter_map(ast::Section::cast)
                    .any(|s| s.name().eq_ignore_ascii_case("requestpage"))
            {
                lw.rec_slot = rec_slot;
            }
        }
        // A dependency routine nothing reached was never checked: a stub that refuses.
        let reached = !checked.is_skipped(oi)
            && !ws.routine_unreached(oi, routine_offsets[oi][ri])
            && demand.is_none_or(|d| !ws.is_dependency(oi) || d.reached(oi, routine_offsets[oi][ri]));
        // A routine whose check panicked has no types and resolutions to lower from: it refuses.
        let routine = if let Some(why) = checked.panic_of(oi, routine_offsets[oi][ri]) {
            lw.refused(r, format!("internal error while checking: {why}"))
        } else if reached {
            // A panic lowering one routine is a bug in the compiler; the routine refuses and the
            // run goes on.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lw.routine(r))) {
                Ok(routine) => routine,
                Err(p) => {
                    lw.calls.clear();
                    lw.reasons = global_reasons.clone();
                    lw.refused(
                        r,
                        format!("internal error while lowering: {}", crate::check::panic_message(&*p)),
                    )
                }
            }
        } else {
            lw.stub(r)
        };
        if sh.export {
            direct.push(lw.calls.clone());
        }
        // A TestPage drives page routines the call graph cannot see (ADR-012). An xmlport's
        // triggers are run by Import and Export, and a report's by Run, which the graph cannot
        // see either.
        driven_edges(&sh.driven, routine.locals.iter().chain(&globals), &mut lw.calls);
        unsupported.push(std::mem::take(&mut lw.reasons));
        calls.push(std::mem::take(&mut lw.calls));
        routines.push(routine);
    }
    let request_section = (o.kind == ObjKind::Report)
        .then(|| obj.sections().find(|s| s.name().eq_ignore_ascii_case("requestpage")))
        .flatten();
    let page = seq
        .page_layout(oi)
        .map(|l| lw.page(oi, (&obj, request_section.as_ref()), l, &globals, &page_reasons[oi]));
    // A report's request page properties (`SaveValues`, `Caption`) read like a page's: the
    // report's own `Caption` wins over the request page's
    // (corpus handlers/TestReportRequestPageCaption.al).
    let mut properties: Vec<(String, String)> = obj.properties().map(|p| (p.name(), p.value_text())).collect();
    if let Some(sec) = &request_section {
        for p in sec.properties() {
            let name = p.name();
            if name.eq_ignore_ascii_case("Caption") && properties.iter().any(|(k, _)| k.eq_ignore_ascii_case("Caption"))
            {
                continue;
            }
            properties.push((name, p.value_text()));
        }
    }
    let object = ir::Object {
        kind: o.kind.label().to_string(),
        id: o.id,
        name: o.name.clone(),
        globals,
        routines,
        properties,
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
        // A table whose source cannot be used (a platform table nothing provides, a dependency
        // file that does not parse) has no members, so it has no schema: the Field and Key
        // tables report it as not compiled instead of reading fields it does not have.
        table: (o.kind == ObjKind::Table && ws.usable(oi))
            .then(|| lw.table_schema(oi, &obj, extensions.get(&oi).cloned().unwrap_or_default(), routine_at)),
        rec_slot,
        xrec_slot,
        page,
        app: o.app as u32,
        xmlport: xml_irs[oi].clone(),
        report: report_irs[oi].clone(),
        report_ext: ext_irs[oi].clone(),
        namespace: o.namespace.clone(),
        extends_id: ws.extension_target(oi).and_then(|t| ws.objects[t].id),
        layouts: if matches!(o.kind, ObjKind::Report | ObjKind::ReportExtension) {
            crate::reports::layouts_of(&obj)
        } else {
            Vec::new()
        },
        query: checked.queries[oi].as_ref().map(|q| q.query.clone()),
    };
    let positions = std::mem::take(&mut lw.positions);
    // Itself, whatever the lowering read about the extensions and implementers of others, and
    // what `provided::embeds_of` adds from the pages' parts.
    let embeds = lw.embeds.take().into_iter().collect();
    Piece {
        object,
        positions,
        unsupported,
        calls,
        direct,
        embeds,
    }
}

type L<T> = Result<T, String>;

/// The page triggers an extension can declare that the client runs after the page's own trigger.
/// The others answer a Boolean or serve rows, and how an extension's answer combines with the
/// page's is not pinned. OnInit, OnFindRecord and OnNextRecord are page-only in BC: an extension
/// that declares one is not called for it (AL Runner v2 docs/../RecordPatches.PageTriggerMetadata.cs,
/// BC 27.5 and 28.4 NCLMetaForm.DefinedTriggers), so they are refused here.
const EXTENSION_PAGE_TRIGGERS: &[&str] = &[
    "onopenpage",
    "onclosepage",
    "onaftergetrecord",
    "onaftergetcurrrecord",
    "onnewrecord",
    // Both the page's and the extensions' answers count, and both must be true to close
    // (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:
    // OnQueryClosePage_PageAndExtension_BothRun_AndBothMustAgreeToClose).
    "onqueryclosepage",
];

/// The triggers a `modify` block can add to a control, which the client runs.
pub(crate) const MODIFY_TRIGGERS: &[&str] = &[
    "onbeforevalidate",
    "onaftervalidate",
    "ondrilldown",
    "onlookup",
    "onassistedit",
];

/// Why the client cannot drive a page once the extensions `exts` are merged into it: the
/// triggers it does not run. Several extensions may declare one trigger: they run in the order
/// the extensions apply (docs/probes-needed.md, "Page extensions").
fn extension_reasons(exts: &[(usize, ast::Object)]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, e) in exts {
        let ename = e.name();
        for t in e.triggers() {
            if !EXTENSION_PAGE_TRIGGERS.contains(&t.name().to_ascii_lowercase().as_str()) {
                out.push(format!(
                    "page extension `{ename}`: trigger {} is not supported yet",
                    t.name()
                ));
            }
        }
        // Triggers inside `modify` blocks: only the ones the client dispatches.
        for sec in ast::sections_in(e.syntax()) {
            if !sec.name().eq_ignore_ascii_case("modify") {
                continue;
            }
            for t in sec.triggers() {
                if !MODIFY_TRIGGERS.contains(&t.name().to_ascii_lowercase().as_str()) {
                    out.push(format!(
                        "page extension `{ename}`: trigger {} in modify({}) is not supported yet",
                        t.name(),
                        sec.args().first().cloned().unwrap_or_default()
                    ));
                }
            }
        }
    }
    out
}

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
        // The same reading as `Evaluate` into a DateFormula (types/TestBlankEnumInitTests.Codeunit.al:88-95
        // says InitValue and Evaluate share one evaluator): a calculable formula is kept as written.
        // Blank, garbled and not-yet-supported formulas stay refused (the field is not initialised).
        Ty::DateFormula => {
            let f = t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).unwrap_or(t);
            if al_builtins::datefns::is_blank_formula(f)
                || al_builtins::datefns::is_garbage_formula(f)
                || al_builtins::datefns::check_formula(f).is_err()
            {
                return None;
            }
            ir::Const::DateFormula(f.to_string())
        }
        _ => return None,
    })
}

/// A `const(..)` value as a constant of the source field's type.
pub(crate) fn calc_const(
    ws: &Workspace,
    members: &MemberTable<'_>,
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
        let Resolution::Found(e) = ws.resolve_ref(file, ObjKind::Enum, &crate::types::qualified_parts(name)) else {
            return Err(format!(
                "CalcFormula const({raw}): enum `{}` is outside the workspace",
                crate::types::display_name(name)
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
    members: &'a MemberTable<'a>,
    checked: &'a CheckResult,
    catalog: &'a al_builtins::catalog::Catalog,
    routine_at: &'a RoutineAt,
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
    layouts: &'a Seq<'a>,
    /// Per object: why a variable of an xmlport is refused.
    xml_reasons: &'a [Option<String>],
    /// Per object: why a variable of a report is refused.
    report_reasons: &'a [Option<String>],
    /// The objects whose extensions or implementers the lowering read (`provided.rs`).
    embeds: std::cell::RefCell<std::collections::BTreeSet<usize>>,
    /// Answers many routines ask for, computed once per program.
    memo: &'a Memo,
    /// Return type of the routine being lowered (a DotNet result in `exit` is coerced to it).
    ret_ty: Option<RtType>,
    /// The declared return type of the routine being lowered (an interface result converts in `exit`).
    ret_decl: Option<Ty>,
    /// The routine nodes of every object, numbered as `routine_at` numbers them.
    routine_nodes: &'a Nodes<'a>,
    /// The global slot of each variable of a page extension (see `lower`).
    ext_slots: &'a [HashMap<String, usize>],
    /// The page extensions of each page, in the order they apply.
    page_extensions: &'a HashMap<usize, Vec<usize>>,
    /// `Program::positions` while the object is lowered (taken from and given back to the program).
    positions: Vec<ir::SrcPos>,
    /// The line table of the object's file; `None` when positions are off.
    lines: Option<std::rc::Rc<FileLines>>,
    /// Statements of the routine being lowered get position markers (off for made-up routines).
    emit_pos: bool,
    /// Index of the routine being lowered in its object.
    routine_idx: usize,
    /// The assumptions the lowering of each statement being lowered rests on (innermost last).
    assumed: Vec<Vec<al_builtins::assumptions::Assumption>>,
}

/// What lowering asks about an object again and again. Each answer is computed on first use and
/// shared by every routine of the program: walking the syntax tree for it per use was a third of
/// the lowering time.
#[derive(Default)]
struct Memo {
    /// The codeunits that implement an interface (or one that extends it), in object order.
    implementers: std::cell::RefCell<HashMap<usize, std::rc::Rc<Vec<usize>>>>,
    /// The `TableType` property of a table, trimmed.
    table_type: std::cell::RefCell<HashMap<usize, std::rc::Rc<Vec<String>>>>,
}

/// Line starts of a source file, for turning offsets into the author's line and column.
struct FileLines {
    file: u32,
    /// Offset of the first byte of every line of the file as the author wrote it.
    starts: Vec<u32>,
    /// Byte ranges the compiler added to the parsed text (page expression routines).
    synthetic: Vec<(usize, usize)>,
}

impl FileLines {
    fn new(ws: &Workspace, file: usize) -> FileLines {
        let text = ws.text(file).as_bytes();
        let mut starts = vec![0u32];
        starts.extend(
            text.iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        );
        FileLines {
            file: file as u32,
            starts,
            synthetic: ws.synthetic(file).to_vec(),
        }
    }

    /// The 1-based (line, column) the author sees at `offset` of the parsed text.
    fn locate(&self, offset: usize) -> (u32, u32) {
        let added: usize = self
            .synthetic
            .iter()
            .take_while(|(_, b)| *b <= offset)
            .map(|(a, b)| b - a)
            .sum();
        let offset = offset - added;
        let line = self.starts.partition_point(|&s| s as usize <= offset) - 1;
        (line as u32 + 1, (offset - self.starts[line] as usize) as u32 + 1)
    }
}

/// The first and last token of `n` that are not whitespace or comments.
fn significant_span(n: &SyntaxNode) -> Option<(usize, usize)> {
    let mut first = n.first_token()?;
    while first.kind().is_trivia() {
        first = first.next_token()?;
    }
    let mut last = n.last_token()?;
    while last.kind().is_trivia() {
        last = last.prev_token()?;
    }
    Some((
        usize::from(first.text_range().start()),
        usize::from(last.text_range().end()).max(usize::from(first.text_range().start())),
    ))
}

fn key(file: usize, n: &SyntaxNode) -> (usize, usize, usize) {
    let r = n.text_range();
    (file, r.start().into(), r.end().into())
}

pub(crate) fn is_stmt(k: SyntaxKind) -> bool {
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

/// The builtin members AL writes as a property: `X.Name := v` is the call `X.Name(v)`. Each is a
/// property of its type in the AL language, so valid code assigns it. Listed by name, not by
/// shape: a method with one optional parameter is not a property (`Rec.Find(Which)`).
///
/// - `FieldRef.Value`: corpus recordref/TestRecordRefCRUD.al:20-23.
/// - `Page.Editable`, `Page.PromptMode`: handlers/TestPageModalNewRowLine_Objects.al:69;
///   `Page.Caption`: handlers/TestPageCaption_Page.al:44; `Page.LookupMode`: the property of the
///   same name on a page.
/// - `Notification.Id`, `.Message`, `.Scope`: handlers/TestNotificationHandler.al:21 and :55.
/// - `Record.ReadIsolation`, `RecordRef.ReadIsolation` and `.SecurityFiltering`: Base
///   Application (`Item.ReadIsolation := IsolationLevel::ReadUncommitted`).
/// - `ErrorInfo.Message`, `.DetailedMessage`, `.Title`, `.ControlName`, `.FieldNo`, `.PageNo`,
///   `.TableId`, `.Verbosity`, `.DataClassification`, `.Collectible`, `.ErrorType`,
///   `.CustomDimensions`: the call form pins getter and storage (errorinfo/TestErrorInfoType.al:
///   249-271, :284-307, :313-345; TestErrorInfoErrorType.al:25-45). Nothing sets `RecordId` in
///   the corpus (only `Create(.., Record)` does, :360-377): its native raises `Unsupported` when
///   run, so the assignment is refused when it is reached, not before. `SystemId` has no native.
/// - `Record.Ascending`, `.Mark`, `.MarkedOnly`: record/TestRecordFind.al:237-284 (`Mark`,
///   `MarkedOnly`), handlers/TestPartOwnSortVsSubPageView.al:187 and :300 (`Ascending(false)`),
///   handlers/TestPageActionRunPageView.al:134 (`Ascending()`).
/// - `Report.Language`, `.FormatRegion`: handlers/TestReportCurrReportLanguage.al:58-103.
/// - `Report.UseRequestPage`: handlers/TestReportGetFilterInTriggers.al:66 and
///   codeunit/TestWriteTxTestBoundary.al:272 (`UseRequestPage(false)`).
/// - `System.WorkDate`: session/TestSessionFunctions.al:63-65 (`WorkDate(20240101D)`).
/// - `HttpRequestMessage.Method`: network/TestHttpRequestMessage.al:23; `.Content`: :39.
/// - `File.TextMode`, `File.WriteMode`: Base Application's "Export Consolidation" report.
fn is_property_setter(receiver: &str, name: &str) -> bool {
    matches!(
        (receiver, name),
        ("FieldRef" | "TestField", "Value")
            | ("InStream" | "OutStream", "Position")
            | ("Page", "Editable" | "PromptMode" | "Caption" | "LookupMode")
            | ("Notification", "Id" | "Message" | "Scope")
            | (
                "ErrorInfo",
                "Message"
                    | "DetailedMessage"
                    | "Title"
                    | "ControlName"
                    | "FieldNo"
                    | "PageNo"
                    | "TableId"
                    | "Verbosity"
                    | "DataClassification"
                    | "Collectible"
                    | "ErrorType"
                    | "CustomDimensions"
                    | "RecordId"
            )
            | ("Record" | "RecordRef", "ReadIsolation" | "SecurityFiltering")
            | ("Record", "FilterGroup" | "Ascending" | "Mark" | "MarkedOnly")
            | ("RecordRef", "FilterGroup")
            | ("Report", "UseRequestPage" | "Language" | "FormatRegion")
            | ("System", "WorkDate")
            | ("HttpRequestMessage", "Method" | "Content")
            | ("File", "TextMode" | "WriteMode")
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
    fn relation_arms(&self, oi: usize, f: &crate::members::FieldInfo, with_filters: bool) -> Vec<ir::RelationArm> {
        let Some(text) = f
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TableRelation"))
            .map(|(_, v)| v)
        else {
            return Vec::new();
        };
        let Some(parsed) = (if with_filters {
            crate::relation::parse_lenient(text)
        } else {
            crate::relation::parse(text)
        }) else {
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
            for (k, (name, value)) in a.conditions.iter().enumerate() {
                let Some(pos) = field_pos(own, name) else {
                    return Vec::new();
                };
                if a.filter_conditions.get(k).copied().unwrap_or(false) {
                    // The filter text as written; AL quotes an identifier with `"`, the filter grammar with `'`.
                    let raw = a.condition_raw.get(k).cloned().unwrap_or_default();
                    let raw = if raw.contains('"') { rewrite_quotes(&raw) } else { raw };
                    conditions.push(ir::RelationCond {
                        field: pos,
                        value: ir::Const::Text(raw),
                        filter: true,
                    });
                    continue;
                }
                let Some(value) = self.condition_const(file, value, &own.fields[pos].ty) else {
                    return Vec::new();
                };
                conditions.push(ir::RelationCond {
                    field: pos,
                    value,
                    filter: false,
                });
            }
            // The whole path names a table, or all but the last part do and the last is a field.
            let resolve = |parts: &[String]| match self.ws.resolve_relation_target(file, ObjKind::Table, parts) {
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
                        crate::relation::ParsedWhere::Value { related, text } => {
                            let p = field_pos(&self.members[t], related)?;
                            // `Database::"X"` / `Report::"X"` etc.: the object's id, as for a
                            // CalcFormula const (corpus record/CdcOwner.Table.al:67,79).
                            let text = if !text.starts_with('\'') && text.contains("::") {
                                match calc_const(self.ws, self.members, file, text, &self.members[t].fields[p].ty) {
                                    Ok(ir::Const::Integer(id)) => id.to_string(),
                                    Ok(ir::Const::BigInteger(id)) => id.to_string(),
                                    Ok(ir::Const::Decimal(id)) => id,
                                    _ => return None,
                                }
                            } else if text.contains('"') {
                                // `filter(Inventory | "Non-Inventory")`: AL quotes an identifier with `"`,
                                // the filter grammar with `'` (corpus
                                // record/TestCalcFormulaQuotedFilterTests.Codeunit.al:9 pins it for a
                                // CalcFormula filter; docs/probes-needed.md for the TableRelation).
                                rewrite_quotes(text)
                            } else {
                                text.clone()
                            };
                            Some(ir::RelationWhere::Value { related: p, text })
                        }
                        crate::relation::ParsedWhere::Field { related, own: o } => Some(ir::RelationWhere::Field {
                            related: field_pos(&self.members[t], related)?,
                            own: field_pos(own, o)?,
                        }),
                    })
                    .collect(),
                (Some(ws), None) if ws.is_empty() => Some(Vec::new()),
                _ => None,
            };
            // A filter text with an AL-quoted identifier read as a filter text: only the CalcFormula and
            // DataItemTableView forms are pinned.
            let quoted = a.wheres.iter().flatten().any(|w| {
                matches!(w, crate::relation::ParsedWhere::Value { text, .. }
                    if !text.starts_with('\'') && !text.contains("::") && text.contains('"'))
            }) || a
                .condition_raw
                .iter()
                .zip(&a.filter_conditions)
                .any(|(raw, filter)| *filter && raw.contains('"'));
            arms.push(ir::RelationArm {
                conditions,
                table,
                table_name: a.path.join("."),
                field,
                wheres,
                assumes: if quoted {
                    vec![al_builtins::assumptions::Assumption::TableRelationQuotes]
                } else {
                    Vec::new()
                },
            });
        }
        arms
    }

    /// The relation of a field the runtime does not understand (`relation_arms` is empty), as far
    /// as the Table Relations Metadata table lists it: per arm the related table and field and the
    /// own fields its conditions name, whatever their constants. Empty when even that is unclear.
    fn relation_meta(&self, oi: usize, f: &crate::members::FieldInfo) -> Vec<ir::MetaRelationArm> {
        let Some(text) = f
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TableRelation"))
            .map(|(_, v)| v)
        else {
            return Vec::new();
        };
        let Some(parsed) = crate::relation::parse_lenient(text) else {
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
            let mut filters = Vec::new();
            let mut values = Vec::new();
            for (k, (name, text)) in a.conditions.iter().enumerate() {
                let Some(pos) = field_pos(own, name) else {
                    return Vec::new();
                };
                conditions.push(pos);
                let is_filter = a.filter_conditions.get(k).copied().unwrap_or(false);
                filters.push(is_filter);
                // The value the table lists: the ordinal of an option member, the ordinals of an alternative
                // `A | B` of members joined by `|` (session/PWPlatformTables.al:109-123). Any other
                // shape of a filter is not pinned.
                let members = match &own.fields[pos].ty {
                    Ty::Option(m) => Some(m),
                    _ => None,
                };
                let ordinal = |word: &str| {
                    let word = word.trim().trim_matches('"');
                    members?
                        .iter()
                        .position(|m| m.eq_ignore_ascii_case(word))
                        .map(|i| i.to_string())
                };
                values.push(if is_filter {
                    text.split('|')
                        .map(ordinal)
                        .collect::<Option<Vec<_>>>()
                        .map(|v| v.join("|"))
                } else {
                    ordinal(text)
                });
            }
            let resolve = |parts: &[String]| match self.ws.resolve_relation_target(file, ObjKind::Table, parts) {
                Resolution::Found(t) => Some(t),
                _ => None,
            };
            let (table, field) = match resolve(&a.path) {
                Some(t) => (t, None),
                None if a.path.len() > 1 => match resolve(&a.path[..a.path.len() - 1]) {
                    Some(t) => match field_pos(&self.members[t], &a.path[a.path.len() - 1]) {
                        Some(p) => (t, Some(p)),
                        None => return Vec::new(),
                    },
                    None => return Vec::new(),
                },
                None => return Vec::new(),
            };
            arms.push(ir::MetaRelationArm {
                conditions,
                filters,
                values,
                table,
                field,
            });
        }
        arms
    }

    /// The constant of a `Field = const(x)` condition, typed by the field.
    fn condition_const(&self, file: usize, text: &str, ty: &Ty) -> Option<ir::Const> {
        if let Ty::Enum(name) = ty {
            // The enum is named in the file that declares the field, as a relation target is: a
            // dependency's file resolves it among the apps it saw (its `using`s included), so that an
            // enum of another namespace is found (Gen. Journal Line."IC Account No.", issue 23).
            let Resolution::Found(e) =
                self.ws
                    .resolve_relation_target(file, ObjKind::Enum, &crate::types::qualified_parts(name))
            else {
                return None;
            };
            let (value, _) = self.members[e]
                .enum_values
                .iter()
                // Both sides trimmed: the blank member `" "` is named `" "` in `const(" ")`
                // (Base Application "Standard Service Line"."No.").
                .find(|(_, n)| n.trim().eq_ignore_ascii_case(text.trim()))?;
            return Some(ir::Const::Enum(e as u32, *value as i32));
        }
        init_const(text, ty)
    }

    /// The position of field `id`.
    fn readable_field_pos(&self, table: usize, id: u32) -> L<usize> {
        // A column of a platform table that no corpus test pins is refused, not defaulted. The
        // table's own triggers read its columns as they are declared.
        if let Some(why) = self.ws.objects[table]
            .id
            .filter(|_| self.oi != table && self.ws.platform_table(table))
            .and_then(|t| al_builtins::platform::unpinned_column(t, id))
        {
            return Err(why);
        }
        self.field_pos(table, id).ok_or_else(|| "field position".to_string())
    }

    /// The triggers named `trigger` that `modify(field)` blocks of the table's `extensions` declare.
    fn ext_field_triggers(
        &self,
        extensions: &[usize],
        field: &str,
        trigger: &str,
        routine_at: &RoutineAt,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &e in extensions {
            if !self.ws.usable(e) {
                continue;
            }
            for sec in ast::sections_in(self.ws.object_ast(e).syntax()) {
                if !sec.name().eq_ignore_ascii_case("modify")
                    || !sec.args().first().is_some_and(|a| a.eq_ignore_ascii_case(field))
                {
                    continue;
                }
                for t in sec.triggers().filter(|t| t.name().eq_ignore_ascii_case(trigger)) {
                    let offset: usize = t.syntax().text_range().start().into();
                    if let Some(r) = routine_at.get(&(e, offset)) {
                        out.push((e, r));
                    }
                }
            }
        }
        out
    }

    fn table_schema(&self, oi: usize, obj: &ast::Object, extensions: Vec<usize>, routine_at: &RoutineAt) -> ir::Table {
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
                let arms = self.relation_arms(oi, f, false);
                let runtime_arms = if arms.is_empty() && f.table_relation {
                    self.relation_arms(oi, f, true)
                } else {
                    Vec::new()
                };
                ir::Field {
                    id: f.id,
                    name: f.name.clone(),
                    ty: self.rt(&f.ty).unwrap_or(RtType::Variant),
                    stored: f.class == crate::members::FieldClass::Normal,
                    init: f.init_value.as_ref().and_then(|v| {
                        init_const(v, &f.ty).or_else(|| match f.ty {
                            // A member of the field's enum, by name; a blank-named member too
                            // (types/TestBlankEnumInitTests.Codeunit.al).
                            Ty::Enum(_) => {
                                let file = self.ws.objects[f.declared_in.unwrap_or(oi)].file;
                                self.calc_const(file, v, &f.ty).ok()
                            }
                            _ => None,
                        })
                    }),
                    auto_increment: f.auto_increment,
                    relation: f.table_relation,
                    init_text: f.init_value.clone(),
                    relation_table: f.relation_table.clone(),
                    relation_tables: f.relation_tables.clone(),
                    properties: f.properties.clone(),
                    relation_arms: arms.clone(),
                    relation_runtime: runtime_arms,
                    relation_meta: if arms.is_empty() && f.table_relation {
                        self.relation_meta(oi, f)
                    } else {
                        Vec::new()
                    },
                    on_validate: match (f.declared_in, f.on_validate) {
                        (Some(o), Some(off)) => routine_at.get(&(o, off)).map(|r| (o, r)),
                        _ => None,
                    },
                    ext_before_validate: self.ext_field_triggers(&extensions, &f.name, "OnBeforeValidate", routine_at),
                    ext_after_validate: self.ext_field_triggers(&extensions, &f.name, "OnAfterValidate", routine_at),
                    on_lookup: match (f.declared_in, f.on_lookup) {
                        (Some(o), Some(off)) => routine_at.get(&(o, off)).map(|r| (o, r)),
                        _ => None,
                    },
                    flow_filter: f.class == crate::members::FieldClass::FlowFilter,
                    calc,
                    calc_refusal: refusal,
                    declared_in: f.declared_in.map(|o| o as u32),
                }
            })
            .collect();
        let mut keys = Vec::new();
        let mut unique_keys = Vec::new();
        let mut key_props: Vec<Vec<(String, String)>> = Vec::new();
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
                        key_props.push(k.properties().map(|p| (p.name(), p.value_text())).collect());
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
            key_props.push(Vec::new());
        }
        let per_company = !obj.properties().any(|p| {
            p.name().eq_ignore_ascii_case("DataPerCompany") && p.value_text().trim().eq_ignore_ascii_case("false")
        });
        ir::Table {
            fields,
            keys,
            unique_keys,
            key_props,
            per_company,
            extensions,
        }
    }

    /// The control tree of page `oi` with its extensions merged in, with bindings and routine
    /// indices resolved. A control an extension declares keeps the extension as its `owner`: its
    /// routines and variable slots are the extension's.
    fn page(
        &self,
        oi: usize,
        (obj, request_section): (&ast::Object, Option<&ast::Section>),
        layout: &crate::pages::Layout,
        globals: &[ir::Slot],
        reasons: &[String],
    ) -> ir::Page {
        let mut unsupported = reasons.to_vec();
        let table = self.table_of[oi];
        let extensions: Vec<usize> = self.page_extensions.get(&oi).cloned().unwrap_or_default();
        // The code a control's text names lives in its owner: the page, or the extension.
        struct Owner {
            object: usize,
            obj: ast::Object,
            names: std::collections::BTreeSet<String>,
            base: usize,
        }
        let owner_of = |o: Option<usize>| {
            let object = o.unwrap_or(oi);
            let obj = if o.is_none() {
                obj.clone()
            } else {
                self.ws.object_ast(object)
            };
            Owner {
                object,
                names: crate::pages::global_names(&obj),
                base: usize::from(obj.syntax().text_range().start()),
                obj,
            }
        };
        let routine_named =
            |ow: &Owner, name: &str| self.routine_nodes.get(ow.object).iter().position(|r| r.name() == name);
        let routine_of = |ow: &Owner, r: &ast::Routine| {
            self.routine_at
                .get(&(ow.object, usize::from(r.syntax().text_range().start())))
        };
        // The routine behind an expression-valued property written at `p` in `ow`.
        let prop_value = |ow: &Owner, p: &ast::Property, unsupported: &mut Vec<String>, control: &str| {
            let name = p.name().to_ascii_lowercase();
            let value = p.value_text();
            let off = usize::from(p.syntax().text_range().start()) - ow.base;
            let dynamic = crate::pages::dynamic_type(&name).filter(|t| !crate::pages::is_literal(*t, &value));
            let val = match dynamic {
                Some(_) => match routine_named(ow, &crate::pages::synthetic_name(&name, off)) {
                    Some(r) => Some(r),
                    None => {
                        unsupported.push(format!("property {name} of control {control} has no routine"));
                        None
                    }
                },
                None => None,
            };
            (name, value, val)
        };
        let mut controls = Vec::new();
        for raw in &layout.controls {
            let sec = &raw.section;
            // A data item of a request page: the test client reads and writes the filters of its
            // record. The item's own properties and triggers belong to the report run.
            if raw.kind == ir::ControlKind::DataItem {
                let binding = match self.globals.get(&raw.name.to_lowercase()) {
                    Some(slot) => ir::Binding::Var(*slot),
                    None => {
                        unsupported.push(format!("data item {} has no record", raw.name));
                        ir::Binding::None
                    }
                };
                controls.push(ir::Control {
                    name: raw.name.clone(),
                    kind: ir::ControlKind::DataItem,
                    parent: None,
                    binding,
                    props: Vec::new(),
                    triggers: Vec::new(),
                    part_page: None,
                    part_link: None,
                    owner: None,
                    ext_triggers: Vec::new(),
                    run_object: None,
                    action_ref: None,
                });
                continue;
            }
            let ow = owner_of(raw.owner);
            let mut props: Vec<(String, ir::PropVal)> = Vec::new();
            for p in sec.properties() {
                let (name, value, routine) = prop_value(&ow, &p, &mut unsupported, &raw.name);
                props.push((
                    name,
                    match routine {
                        Some(r) => ir::PropVal::Routine(r),
                        None => ir::PropVal::Lit(value),
                    },
                ));
            }
            // `modify` blocks replace what the control states, in the order the extensions apply.
            let mut ext_triggers = Vec::new();
            for (e, msec) in &raw.modifies {
                let mow = owner_of(Some(*e));
                for p in msec.properties() {
                    let (name, value, routine) = prop_value(&mow, &p, &mut unsupported, &raw.name);
                    let val = match routine {
                        Some(r) => ir::PropVal::Ext(*e, r),
                        None => ir::PropVal::Lit(value),
                    };
                    match props.iter_mut().find(|(n, _)| *n == name) {
                        Some(slot) => slot.1 = val,
                        None => props.push((name, val)),
                    }
                }
                for t in msec.triggers() {
                    match routine_of(&mow, &t) {
                        Some(r) => ext_triggers.push((t.name().to_ascii_lowercase(), *e, r)),
                        None => {
                            unsupported.push(format!("trigger {} of control {} has no routine", t.name(), raw.name))
                        }
                    }
                }
            }
            let mut binding = ir::Binding::None;
            if raw.kind == ir::ControlKind::Field && sec.name().eq_ignore_ascii_case("label") {
                binding = ir::Binding::Caption;
            } else if raw.kind == ir::ControlKind::Field {
                let args = sec.args();
                match args.get(1).map(|a| crate::pages::classify_source(a, &ow.names)) {
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
                        let slot = match raw.owner {
                            None => globals.iter().position(|g| g.name.eq_ignore_ascii_case(&v)),
                            Some(e) => self.ext_slots[e].get(&v.to_lowercase()).copied(),
                        };
                        match slot {
                            Some(slot) => binding = ir::Binding::Var(slot),
                            None => unsupported.push(format!(
                                "control {} is bound to variable {v} of an unsupported type",
                                raw.name
                            )),
                        }
                    }
                    Some(crate::pages::Source::Expr) => {
                        let off = usize::from(sec.syntax().text_range().start()) - ow.base;
                        match routine_named(&ow, &crate::pages::synthetic_name("source", off)) {
                            Some(r) => binding = ir::Binding::Expr(r),
                            // A control of an extension that names a variable of the base object
                            // (`pages::needed` made no routine for it).
                            None => {
                                let base_slot = raw
                                    .owner
                                    .and_then(|_| crate::pages::plain_name(args.get(1)?))
                                    .and_then(|n| globals.iter().position(|g| g.name.eq_ignore_ascii_case(&n)));
                                match base_slot {
                                    Some(slot) => binding = ir::Binding::BaseVar(slot),
                                    None => unsupported.push(format!(
                                        "control {} is bound to a name that is no variable of its base object",
                                        raw.name
                                    )),
                                }
                            }
                        }
                    }
                    None => {}
                }
            }
            let triggers = sec
                .triggers()
                .filter_map(|t| routine_of(&ow, &t).map(|r| (t.name().to_ascii_lowercase(), r)))
                .collect();
            let file = self.ws.objects[ow.object].file;
            let part_page = if raw.kind == ir::ControlKind::Part {
                match crate::types::section_object_name(sec, 1).map(|n| self.resolve_obj_in(file, ObjKind::Page, &n)) {
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
                (Some(pp), true) => match self.part_link(oi, file, layout, sec, pp) {
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
            // An action that declares both RunObject and OnAction runs only the RunObject
            // (handlers/TestPageRunObjectContracts.al:371-390).
            let run_object = self.run_object(oi, file, sec);
            controls.push(ir::Control {
                name: raw.name.clone(),
                kind: raw.kind.clone(),
                parent: raw.parent,
                binding,
                props,
                triggers,
                part_page,
                part_link,
                owner: raw.owner,
                ext_triggers,
                run_object,
                action_ref,
            });
        }
        let base = owner_of(None);
        let triggers = match request_section {
            Some(sec) => sec
                .triggers()
                .filter_map(|t| routine_of(&base, &t).map(|r| (t.name().to_ascii_lowercase(), r)))
                .collect(),
            None if obj.kind().eq_ignore_ascii_case("report") => Vec::new(),
            None => obj
                .triggers()
                .filter_map(|t| routine_of(&base, &t).map(|r| (t.name().to_ascii_lowercase(), r)))
                .collect(),
        };
        let mut ext_triggers = Vec::new();
        for &e in &extensions {
            let ow = owner_of(Some(e));
            // The triggers of a reportextension's request page are in its `requestpage` section.
            let triggers: Vec<ast::Routine> = if request_section.is_some() || obj.kind().eq_ignore_ascii_case("report")
            {
                ow.obj
                    .sections()
                    .filter(|s| s.name().eq_ignore_ascii_case("requestpage"))
                    .flat_map(|s| s.triggers().collect::<Vec<_>>())
                    .collect()
            } else {
                ow.obj.triggers().collect()
            };
            for t in triggers {
                if let Some(r) = routine_of(&ow, &t) {
                    ext_triggers.push((t.name().to_ascii_lowercase(), e, r));
                }
            }
        }
        ir::Page {
            controls,
            triggers,
            curr_page_slot: globals.iter().position(|g| g.name == "CurrPage").unwrap_or(0),
            unsupported,
            extensions,
            ext_triggers,
            action_areas: layout.action_areas.clone(),
            assumes: layout.assumes.clone(),
        }
    }

    /// The `RunObject` of action `sec` of page `oi` with its `RunPageLink`, `RunPageView`,
    /// `RunPageMode` and `RunPageOnRec`; `None` for an action without one. What the test client
    /// cannot follow yet becomes `unsupported` (the action is refused when it is invoked).
    fn run_object(&self, oi: usize, file: usize, sec: &ast::Section) -> Option<ir::RunObject> {
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
        match self.ws.resolve_ref(file, target, &parts) {
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
                    self.link_entries("RunPageLink", &link, file, self.table_of[oi], target_table)
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
        file: usize,
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
                    let t = self.resolve_obj_in(file, ObjKind::Table, &name)?;
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
        file: usize,
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
                    .and_then(|n| self.resolve_obj_in(file, ObjKind::Page, n).ok())
                else {
                    return Err("the Provider part shows a page outside the workspace".into());
                };
                self.table_of[ppage]
            }
            None => self.table_of[oi],
        };
        let part_table = self.table_of[part_page].ok_or("the part page has no source table to link")?;
        link.entries = self.link_entries("SubPageLink", &text, file, host_table, part_table)?;
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
                "CalcFormula source: {}",
                self.ws.outside_workspace(ObjKind::Table, &f.table)
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
        self.resolve_obj_in(self.file, kind, name)
    }

    /// Object `name` as the file `file` sees it.
    fn resolve_obj_in(&self, file: usize, kind: ObjKind, name: &str) -> L<usize> {
        match self.ws.resolve_ref(file, kind, &crate::types::qualified_parts(name)) {
            Resolution::Found(i) => Ok(i),
            _ => Err(self.ws.outside_workspace(kind, &crate::types::display_name(name))),
        }
    }

    /// The `TableType` properties of table `table`, trimmed, in source order.
    fn table_types(&self, table: usize) -> std::rc::Rc<Vec<String>> {
        if let Some(hit) = self.memo.table_type.borrow().get(&table) {
            return hit.clone();
        }
        let kinds: std::rc::Rc<Vec<String>> = std::rc::Rc::new(
            self.ws
                .object_ast(table)
                .properties()
                .filter(|p| p.name().eq_ignore_ascii_case("TableType"))
                .map(|p| p.value_text().trim().to_string())
                .collect(),
        );
        self.memo.table_type.borrow_mut().insert(table, kinds.clone());
        kinds
    }

    fn is_temporary_table(&self, table: usize) -> bool {
        self.table_types(table)
            .iter()
            .any(|k| k.eq_ignore_ascii_case("temporary"))
    }

    /// The codeunits that implement `iface` (or an interface that extends it), in object order.
    fn implementers(&self, iface: usize) -> std::rc::Rc<Vec<usize>> {
        self.embeds.borrow_mut().insert(iface);
        if let Some(hit) = self.memo.implementers.borrow().get(&iface) {
            return hit.clone();
        }
        let found: std::rc::Rc<Vec<usize>> = std::rc::Rc::new(
            (0..self.ws.objects.len())
                .filter(|&cu| self.ws.objects[cu].kind == ObjKind::Codeunit && self.implements(cu, iface))
                .collect(),
        );
        self.memo.implementers.borrow_mut().insert(iface, found.clone());
        found
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
                self.embeds.borrow_mut().insert(x);
                match self.xml_reasons[x].as_ref() {
                    None => RtType::Codeunit(x as u32),
                    Some(why) => return Err(format!("XmlPort \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            // A report variable is an instance with its own globals, like a codeunit's.
            Ty::Object(ObjKind::Report, n) => {
                let r = self.resolve_obj(ObjKind::Report, n)?;
                self.embeds.borrow_mut().insert(r);
                match self.report_reasons[r].as_ref() {
                    None => RtType::Codeunit(r as u32),
                    Some(why) => return Err(format!("Report \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            Ty::Object(ObjKind::Page, n) => {
                let p = self.resolve_obj(ObjKind::Page, n)?;
                self.embeds.borrow_mut().insert(p);
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
                self.embeds.borrow_mut().insert(p);
                match self.page_reasons[p].first() {
                    None => RtType::TestPage(p as u32),
                    Some(why) => return Err(format!("TestPage \"{}\": {why}", crate::types::display_name(n))),
                }
            }
            // A request page is the page of a report; the handler's variable is typed by the report.
            Ty::TestRequestPage(n) => {
                let r = self.resolve_obj(ObjKind::Report, n)?;
                self.embeds.borrow_mut().insert(r);
                match self.page_reasons[r].first() {
                    None => RtType::TestPage(r as u32),
                    Some(why) => {
                        return Err(format!("TestRequestPage \"{}\": {why}", crate::types::display_name(n)));
                    }
                }
            }
            Ty::Object(ObjKind::Interface, n) => {
                let i = self.resolve_obj(ObjKind::Interface, n)?;
                RtType::Interface(i as u32)
            }
            Ty::List(t) => RtType::List(Box::new(self.rt(t)?)),
            Ty::Dictionary(k, v) => RtType::Dictionary(Box::new(self.rt(k)?), Box::new(self.rt(v)?)),
            Ty::Array(d, t) => RtType::Array(d.clone(), Box::new(self.rt(t)?)),
            Ty::Builtin(n) if al_builtins::sysenum::members(n).is_some() => {
                RtType::Option(al_builtins::sysenum::option_members(n).expect("checked"))
            }
            Ty::Builtin(n) => RtType::Library(n.clone()),
            Ty::DotNet(alias) if alias.is_empty() => {
                return Err("a DotNet value whose type is only known at run time cannot be stored as such".into());
            }
            Ty::DotNet(alias) => match self.ws.resolve_dotnet(self.file, alias) {
                Some(i) => RtType::DotNet(i as u32),
                None => {
                    return Err(format!(
                        "DotNet '{alias}' has no dotnet {{ }} declaration in this app or its dependencies"
                    ));
                }
            },
            Ty::Record { table, temporary } => {
                let t = self.resolve_obj(ObjKind::Table, table)?;
                // A table of another type (CRM, ExternalSQL, ...) is served by a table connection.
                let kind = self.table_types(t).first().cloned();
                // A CRM table is lowered: the VM refuses its first use (`Session::connection_check`).
                if let Some(k) = kind.filter(|k| {
                    !k.eq_ignore_ascii_case("normal")
                        && !k.eq_ignore_ascii_case("temporary")
                        && !k.eq_ignore_ascii_case("crm")
                }) {
                    return Err(format!(
                        "table \"{}\" has TableType = {k}: it is served by a table connection, which the runtime does not model",
                        crate::types::display_name(table)
                    ));
                }
                // `TableType = Temporary` makes every record of the table temporary (corpus
                // record/TestTempTableContracts.al:16).
                RtType::Record(t as u32, *temporary || self.is_temporary_table(t))
            }
            other => return Err(format!("type {other} is not lowered yet")),
        })
    }

    /// Whether codeunit `cu` implements interface `iface`: it lists `iface`, or an interface that
    /// extends it, after `implements` (codeunit/TestInterfaceRuntimeContracts.al:
    /// Interface_ExtendedInterface_IsAndAs: a codeunit that implements `Probe IBar` answers a call
    /// through a `Probe IFoo` variable, and `is "Probe IFoo"` holds).
    fn implements(&self, cu: usize, iface: usize) -> bool {
        let file = self.ws.objects[cu].file;
        let family = self.ws.interface_family(iface);
        self.ws.objects[cu].implements.iter().any(
            |p| matches!(self.ws.resolve_ref(file, ObjKind::Interface, p), Resolution::Found(i) if family.contains(&i)),
        )
    }

    /// `value` (of type `src`) converted for a slot of type `target`. An interface slot takes a
    /// codeunit that implements it (corpus codeunit/TestCodeunitInterface.al:14-26) or an enum
    /// that implements it (types/TestEnumDefaultImplTests.Codeunit.al:30-42); everything else
    /// is refused.
    fn convert_to(&self, target: &Ty, src: &Ty, value: ir::Expr) -> L<ir::Expr> {
        let slot = self.rt(target)?;
        // A DotNet result has no static type: it must fit the target exactly (ADR-014).
        if matches!(src, Ty::DotNet(a) if a.is_empty()) && !matches!(slot, RtType::DotNet(_)) {
            return Ok(ir::Expr::DotNetCoerce(slot, Box::new(value)));
        }
        let RtType::Interface(iface) = slot else {
            return Ok(ir::Expr::Convert(slot, Box::new(value)));
        };
        let iface = iface as usize;
        let iname = &self.ws.objects[iface].name;
        match src {
            // The same interface, or one it extends: the value is the same instance
            // (codeunit/TestInterfaceRuntimeContracts.al:Interface_ExtendedInterface_IsAndAs).
            Ty::Object(ObjKind::Interface, n) => {
                let from = self.resolve_obj(ObjKind::Interface, n)?;
                if from != iface && !self.ws.interface_bases(from).contains(&iface) {
                    return Err(format!(
                        "interface \"{}\" into interface \"{iname}\", which it does not extend",
                        crate::types::display_name(n)
                    ));
                }
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
        self.embeds.borrow_mut().insert(e);
        self.embeds.borrow_mut().insert(iface);
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

    /// `x is I` and `x as I` on an interface value: whether the codeunit instance the variable
    /// holds implements `I` (corpus codeunit/TestInterfaceRuntimeContracts.al:
    /// Interface_ExtendedInterface_IsAndAs pins both for an implementing instance). The
    /// implementing codeunits are known here, so the test is a set of object indexes.
    fn interface_test(&mut self, n: &SyntaxNode, kids: &[SyntaxNode]) -> L<ir::Expr> {
        let is = sig_tokens(n)[0].text().eq_ignore_ascii_case("is");
        if !matches!(self.ty_of(&kids[0]), Ty::Object(ObjKind::Interface, _)) {
            return Err("`is` and `as` on a value that is not an interface variable are not lowered yet".into());
        }
        let name = crate::check::type_name_text(&kids[1]);
        let iface = self.resolve_obj(ObjKind::Interface, &name)?;
        let objects: Vec<u32> = self.implementers(iface).iter().map(|&cu| cu as u32).collect();
        let value = Box::new(self.expr(&kids[0])?);
        Ok(ir::Expr::InterfaceTest {
            value,
            objects,
            cast: !is,
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
        for &cu in self.implementers(iface).iter() {
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
            let routine = self.routine_at.get(&(cu, imp.offset)).ok_or("procedure not found")?;
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
        #[cfg(test)]
        if r.name() == "InjectedLowerPanic" {
            panic!("injected lowering panic");
        }
        // A routine the compiler made up (a page expression) has no source.
        self.emit_pos = self.lines.is_some() && !crate::pages::is_synthetic(&r.name());
        let pos = match r.keyword_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
        let end_pos = match r.end_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
        let begin_pos = match r.begin_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
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
        self.ret_decl = ret_ty.clone();
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
        self.ret_ty = ret.clone();
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
        let mut body = match r.body() {
            Some(b) => self.stmts_of(&b),
            None => Vec::new(),
        };
        // The service tier implements a `[Native]` procedure; its AL body is empty. Running that body
        // would answer the default value of the return type, which is not what the platform answers.
        // A few have an implementation here (`platform_native`, pinned by the corpus); the others are refused.
        let native = (body.is_empty() && r.attributes().any(|a| a.name().eq_ignore_ascii_case("native")))
            .then(|| {
                let types: Vec<String> = r
                    .params()
                    .map(|p| p.type_ref().map(|t| t.type_name().to_lowercase()).unwrap_or_default())
                    .collect();
                al_builtins::platform_native::find(&self.ws.objects[self.oi].name, &r.name(), &types)
            })
            .flatten();
        if let Some(entry) = native {
            let call = ir::Expr::Builtin {
                id: entry.id.to_string(),
                receiver: None,
                args: (0..params)
                    .map(|i| ir::Arg::Value(ir::Expr::Load(ir::Place::Local(i))))
                    .collect(),
                ret_used: ret_ty.is_some(),
            };
            body = vec![if ret_ty.is_some() {
                ir::Stmt::Exit(Some(call))
            } else {
                ir::Stmt::Expr(call)
            }];
        } else if body.is_empty() && r.attributes().any(|a| a.name().eq_ignore_ascii_case("native")) {
            self.reasons.push(format!(
                "[Native] procedure `{}` is implemented by the platform and has no AL body (what it answers is not pinned)",
                r.name()
            ));
        }
        let field = r
            .syntax()
            .ancestors()
            .filter_map(ast::Section::cast)
            .find(|s| s.name().eq_ignore_ascii_case("field"))
            .and_then(|s| {
                let id: u32 = s.args().first()?.parse().ok()?;
                self.table_of[self.oi].and_then(|t| self.field_pos(t, id))
            });
        let container = r.syntax().ancestors().find_map(ast::Section::cast).map(|sec| {
            let arg = sec.args().first().cloned().unwrap_or_default();
            (
                sec.name().to_ascii_lowercase(),
                arg.trim().trim_matches('"').to_string(),
            )
        });
        ir::Routine {
            name: r.name(),
            field,
            container,
            is_trigger: r.is_trigger(),
            attributes: self.attributes_of(r),
            params,
            by_var,
            locals,
            ret,
            ret_local,
            body,
            pos,
            end_pos,
            begin_pos,
        }
    }

    /// The first argument of `Format`. An Option field with an `OptionCaption` formats as the
    /// caption, not the member name (corpus fieldref/TestPrecompiledFieldOptionCaption.al:47-55,
    /// a Base Application table whose caption differs from its member). The value keeps its
    /// ordinal and takes the caption list. Other Format forms on such a field are not pinned.
    fn format_option_field(&mut self, plain: bool, arg: &SyntaxNode) -> L<ir::Expr> {
        let value = self.expr(arg)?;
        let Some(Res::Field { table, id: field }) = self.res(arg).cloned() else {
            return Ok(value);
        };
        let Some(f) = self.members[table].fields.iter().find(|f| f.id == field) else {
            return Ok(value);
        };
        let (Ty::Option(names), Some((_, text))) = (
            &f.ty,
            f.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("OptionCaption")),
        ) else {
            return Ok(value);
        };
        let captions: Vec<String> = match crate::members::first_string_literal(text) {
            Some(list) => list.split(',').map(str::to_string).collect(),
            None => return Err(format!("Format of option field {} with OptionCaption `{text}`", f.name)),
        };
        if &captions == names {
            return Ok(value);
        }
        // Every form of Format reads the caption list (text/TestProbeTextDateFormatContracts.al:
        // Format_OptionFieldAndOptionVariable): standard formats 2 and 9 the ordinal, a length cuts or pads the caption.
        let _ = plain;
        if captions.len() != names.len() {
            return Err(format!(
                "Format of option field {} whose OptionCaption has another number of entries than its members is not pinned",
                f.name
            ));
        }
        Ok(ir::Expr::Convert(RtType::Option(captions), Box::new(value)))
    }

    fn attributes_of(&self, r: &ast::Routine) -> Vec<ir::Attribute> {
        r.attributes()
            .map(|a| ir::Attribute {
                text: a.syntax().text().to_string(),
                name: a.name(),
                args: a
                    .args()
                    .map(|l| l.children().filter_map(|e| self.constant(&e).ok()).collect())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// A routine of a dependency app that nothing reached: its name and attributes (so event
    /// subscribers stay registered), no body, and a reason that refuses anything that runs it.
    fn stub(&mut self, r: &ast::Routine) -> ir::Routine {
        self.refused(
            r,
            "not compiled: no code reached it while compiling the workspace's dependencies (ADR-013)".into(),
        )
    }

    /// A routine without a body, and `reason` that refuses anything that runs it.
    fn refused(&mut self, r: &ast::Routine, reason: String) -> ir::Routine {
        self.reasons.push(reason);
        ir::Routine {
            name: r.name(),
            field: None,
            container: None,
            is_trigger: r.is_trigger(),
            attributes: self.attributes_of(r),
            params: 0,
            by_var: Vec::new(),
            locals: Vec::new(),
            ret: None,
            ret_local: None,
            body: Vec::new(),
            pos: ir::NO_POS,
            end_pos: ir::NO_POS,
            begin_pos: ir::NO_POS,
        }
    }

    /// Records the position of the code at `start..end` of the file being lowered and returns its id.
    fn add_pos(&mut self, start: usize, end: usize) -> Option<u32> {
        let lines = self.lines.as_ref()?;
        let (line, col) = lines.locate(start);
        let (end_line, _) = lines.locate(end.saturating_sub(1).max(start));
        let p = ir::SrcPos {
            file: lines.file,
            line,
            col,
            end_line,
            object: self.oi as u32,
            routine: self.routine_idx as u32,
        };
        self.positions.push(p);
        Some(self.positions.len() as u32 - 1)
    }

    /// The marker for statement `n`, when the routine being lowered has positions.
    fn mark(&mut self, n: &SyntaxNode) -> Option<ir::Stmt> {
        if !self.emit_pos {
            return None;
        }
        let (start, end) = significant_span(n)?;
        self.add_pos(start, end).map(ir::Stmt::Pos)
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

    /// Notes that the lowering of the statement being lowered rests on assumption `a`: the statement is
    /// preceded by `Stmt::Assume(a)`, so a test that runs it is reported as flagged.
    fn assume(&mut self, a: al_builtins::assumptions::Assumption) {
        if let Some(frame) = self.assumed.last_mut() {
            if !frame.contains(&a) {
                frame.push(a);
            }
        }
    }

    fn stmt(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        self.assumed.push(Vec::new());
        let mut out = self.stmt_plain(n);
        let marks = self.assumed.pop().unwrap_or_default();
        if !marks.is_empty() && !out.is_empty() {
            // After the position marker, which belongs to the statement that follows it.
            let at = usize::from(matches!(out.first(), Some(ir::Stmt::Pos(_))));
            for (i, a) in marks.into_iter().enumerate() {
                out.insert(at + i, ir::Stmt::Assume(a));
            }
        }
        out
    }

    fn stmt_plain(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        let r = match n.kind() {
            Block => return self.stmts_of(n),
            // A try function called as a statement behaves like a normal method
            // (doc: TryFunction attribute), so its errors propagate.
            // `continue;` (a statement of the AL language that Base Application's Inventory Adjustment uses): the
            // checker sees a bare name that is no variable, procedure or field.
            ExprStmt
                if kids[0].kind() == NameRef
                    && kids[0].text().to_string().trim().eq_ignore_ascii_case("continue")
                    && self.res(&kids[0]).is_none() =>
            {
                Ok(ir::Stmt::Continue)
            }
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
                let cond = self.condition(&kids[0]);
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
                let cond = self.condition(&kids[0]);
                let body = self.stmt_or_empty(kids.get(1));
                cond.map(|c| ir::Stmt::While(c, body))
            }
            RepeatStmt => {
                let (until, body_nodes) = kids.split_last().expect("repeat has until");
                let mut body = Vec::new();
                for b in body_nodes {
                    body.extend(self.stmt(b));
                }
                self.condition(until).map(|u| ir::Stmt::Repeat(body, u))
            }
            ForStmt => {
                let down = sig_tokens(n).iter().any(|t| t.text().eq_ignore_ascii_case("downto"));
                let var = self.place(&kids[0]);
                let from = self.operand(&kids[1], ir::OperandUse::Value);
                let to = self.operand(&kids[2], ir::OperandUse::Value);
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
                Some(e) => self.expr(e).and_then(|v| {
                    // A DotNet result must fit the return type exactly (ADR-014). An interface
                    // result takes what an interface variable takes: a codeunit that implements
                    // it, an enum, or an interface it extends (codeunit/TestInterfaceRuntimeContracts.al:
                    // Interface_VarParameterAndReturnValue_CarryTheReference, `exit(Impl)`).
                    if let Some(decl @ Ty::Object(ObjKind::Interface, _)) = self.ret_decl.clone() {
                        return self
                            .convert_to(&decl, &self.ty_of(e), v)
                            .map(|v| ir::Stmt::Exit(Some(v)));
                    }
                    Ok(ir::Stmt::Exit(Some(match (&self.ty_of(e), &self.ret_ty) {
                        (Ty::DotNet(a), Some(rt)) if a.is_empty() && !matches!(rt, RtType::DotNet(_)) => {
                            ir::Expr::DotNetCoerce(rt.clone(), Box::new(v))
                        }
                        _ => v,
                    })))
                }),
            },
            BreakStmt => Ok(ir::Stmt::Break),
            AssertErrorStmt => Ok(ir::Stmt::AssertError(self.stmt_or_empty(kids.first()))),
            other => Err(format!("statement {other:?} is not lowered yet")),
        };
        match r {
            Ok(s) => match self.mark(n) {
                Some(m) => vec![m, s],
                None => vec![s],
            },
            Err(e) => {
                self.reasons.push(e);
                Vec::new()
            }
        }
    }

    fn assign(&mut self, n: &SyntaxNode, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        // `CurrFieldNo := FieldNo(X)` in table code sets the field number of the implicit Rec. Base Application's
        // `SelltoContactLookup` does it before it validates a field. The corpus pins reading CurrFieldNo
        // (handlers/TestPageCurrFieldNo_Tests.al); the assignment is not pinned (docs/probes-needed.md).
        if kids[0].kind() == SyntaxKind::NameRef
            && matches!(self.res(&kids[0]), Some(Res::Global(g)) if g == "currfieldno")
            && !self.globals.contains_key("currfieldno")
        {
            if !sig_tokens(n).iter().any(|t| t.kind() == SyntaxKind::Assign) {
                return Err("a compound assignment to CurrFieldNo is not lowered (not pinned)".into());
            }
            let rec = self
                .rec_slot
                .ok_or("CurrFieldNo outside code that has an implicit Rec")?;
            let value = self.expr(&kids[1])?;
            let value = self.convert_to(&Ty::Integer, &self.ty_of(&kids[1]), value)?;
            return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                id: "Record.SetCurrFieldNo#1".into(),
                receiver: Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec)))),
                args: vec![ir::Arg::Value(value)],
                ret_used: false,
            }));
        }
        // `X.Property := value` on a DotNet value (ADR-014).
        if let Some(Res::DotNetMember(name)) = self.res(&kids[0]).cloned() {
            if kids[0].kind() == SyntaxKind::MemberExpr {
                if sig_tokens(n).iter().any(|t| {
                    matches!(
                        t.kind(),
                        SyntaxKind::PlusAssign
                            | SyntaxKind::MinusAssign
                            | SyntaxKind::StarAssign
                            | SyntaxKind::SlashAssign
                    )
                }) {
                    return Err("compound assignment to a DotNet property is not lowered yet".into());
                }
                let value = self.dotnet_arg(&kids[1])?;
                return self
                    .dotnet_member(&kids[0], &kids[0], &name, ir::DotNetOp::Set(name.clone()), vec![value])
                    .map(ir::Stmt::Expr);
            }
        }
        // `FieldRef.Value := x` is the setter form of the `Value` method
        // (corpus recordref/TestRecordRefCRUD.al:20-23): not a place, a builtin call.
        if let Some(Res::Builtin(id)) = self.res(&kids[0]).cloned() {
            let sig = self
                .catalog
                .by_id(&id)
                .filter(|s| is_property_setter(&s.receiver, &s.name))
                .cloned();
            let member = kids[0].kind() == SyntaxKind::MemberExpr;
            // Without a receiver the member is a global function (`WorkDate := d`) or a Record
            // member of the implicit Rec of table, page and dataitem code (`FilterGroup := 2`).
            let implicit = !member
                && sig
                    .as_ref()
                    .is_some_and(|s| !s.instance || (s.receiver == "Record" && self.rec_slot.is_some()));
            if let Some(sig) = sig.filter(|_| member || implicit) {
                // A test page control's setter form is run by the test client, not a native builtin.
                if al_builtins::native::lookup(&id).is_none() && !id.starts_with("TestField.") {
                    return Err(match al_builtins::native::refusal(&id) {
                        Some(why) => format!("builtin {id}: {why} (docs/probes-needed.md)"),
                        None => format!("builtin {id} is not implemented"),
                    });
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
                    return Err("compound assignment to a setter-form builtin is not lowered yet".into());
                }
                let receiver = if member {
                    let recv = kids[0].children().next().ok_or("member base")?;
                    // `CurrReport` is the running report: the VM takes it when a report builtin has
                    // no receiver (as for the call form). A static member (`System.WorkDate`)
                    // has none.
                    let current_report = sig.receiver == "Report"
                        && matches!(
                            self.ws.objects[self.oi].kind,
                            ObjKind::Report | ObjKind::ReportExtension
                        )
                        && recv.text().to_string().trim().eq_ignore_ascii_case("CurrReport");
                    if !sig.instance || current_report {
                        None
                    } else {
                        Some(Box::new(match self.place(&recv) {
                            Ok(p) => ir::Arg::Ref(p),
                            Err(_) => ir::Arg::Value(self.expr(&recv)?),
                        }))
                    }
                } else if sig.instance {
                    let rec = self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?;
                    Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec))))
                } else {
                    None
                };
                let rhs = self.expr(&kids[1])?;
                return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                    id,
                    receiver,
                    args: vec![ir::Arg::Value(rhs)],
                    ret_used: false,
                }));
            }
        }
        // `GlobalLanguage := Id` is the other spelling of `GlobalLanguage(Id)` (System Application's
        // "Reten. Pol. Telemetry Impl." writes it so; the setter is pinned by
        // session/TestSessionSettings.al:534): a builtin call, not a place.
        if let Some(Res::Builtin(id)) = self.res(&kids[0]).cloned() {
            if id == "System.GlobalLanguage#1" && kids[0].kind() != SyntaxKind::MemberExpr {
                if sig_tokens(n).iter().any(|t| {
                    t.kind() != SyntaxKind::Assign
                        && matches!(
                            t.kind(),
                            SyntaxKind::PlusAssign
                                | SyntaxKind::MinusAssign
                                | SyntaxKind::StarAssign
                                | SyntaxKind::SlashAssign
                        )
                }) {
                    return Err("compound assignment to GlobalLanguage is not lowered yet".into());
                }
                let rhs = self.expr(&kids[1])?;
                return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                    id,
                    receiver: None,
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
        let scrutinee = self.operand(&kids[0], ir::OperandUse::Value)?;
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

    /// The argument for a `var` parameter: the variable it names. Base Application passes the implicit
    /// `CurrFieldNo` to event publishers that declare `var CallingFieldNo: Integer` (`Sales Header`,
    /// `OnBeforeShouldSearchForCustomerByName`). `CurrFieldNo` is no variable, so the callee gets a copy of its
    /// value and what it assigns to its parameter stays there. Whether BC writes the assignment back to
    /// `CurrFieldNo` is not pinned (docs/probes-needed.md).
    fn by_var_arg(&mut self, a: &SyntaxNode) -> L<ir::Arg> {
        match self.place(a) {
            Ok(p) => Ok(ir::Arg::Ref(p)),
            Err(_)
                if a.kind() == SyntaxKind::NameRef
                    && a.text().to_string().trim().eq_ignore_ascii_case("CurrFieldNo") =>
            {
                Ok(ir::Arg::Value(self.expr(a)?))
            }
            Err(e) => Err(e),
        }
    }

    fn place(&mut self, n: &SyntaxNode) -> L<ir::Place> {
        match n.kind() {
            SyntaxKind::ParenExpr => self.place(&n.children().next().ok_or("empty parentheses")?),
            SyntaxKind::IndexExpr => {
                let kids: Vec<SyntaxNode> = n.children().collect();
                // `A[x] [y]` on a multi-dimensional array is `A[x, y]`: the chain becomes one index.
                if kids[0].kind() == SyntaxKind::IndexExpr && matches!(self.ty_of(&kids[0]), Ty::Array(..)) {
                    if let ir::Place::Index(root, mut idx) = self.place(&kids[0])? {
                        self.assume(al_builtins::assumptions::Assumption::ChainedArrayIndex);
                        for k in &kids[1..] {
                            idx.push(self.expr(k)?);
                        }
                        return Ok(ir::Place::Index(root, idx));
                    }
                }
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
                // An extension variable named like a base variable is its own (corpus
                // handlers/TestPageExtensionBaseAccessContracts.al:20, Shadow).
                Some(Res::Global(name)) => self
                    .globals
                    .get(name)
                    .map(|i| ir::Place::Global(*i))
                    .ok_or_else(|| format!("implicit variable `{name}` is not lowered yet")),
                Some(Res::BaseGlobal(name)) => {
                    let (slot, _, _) = self.base_var(name)?;
                    Ok(ir::Place::BaseGlobal(slot))
                }
                Some(Res::Field { table, id }) => {
                    let (table, id) = (*table, *id);
                    // A temporary record of a platform table is an ordinary temporary table: the
                    // columns the platform provider pins do not matter (record/
                    // TestPlatformTablesWritableContracts.al:27-48).
                    let temporary = n.kind() == SyntaxKind::MemberExpr
                        && n.children()
                            .next()
                            .is_some_and(|b| matches!(self.ty_of(&b), Ty::Record { temporary: true, .. }));
                    let pos = if temporary {
                        self.field_pos(table, id).ok_or_else(|| "field position".to_string())?
                    } else {
                        self.readable_field_pos(table, id)?
                    };
                    let base = match n.kind() {
                        SyntaxKind::MemberExpr => self.place(&n.children().next().ok_or("member base")?)?,
                        _ => ir::Place::Global(self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?),
                    };
                    Ok(ir::Place::Field(Box::new(base), pos))
                }
                _ if n.kind() == SyntaxKind::NameRef && n.text().to_string().trim().eq_ignore_ascii_case("this") => Err(
                    "`this` passed by reference (`BindSubscription(this)`) is not lowered: no corpus test passes the running instance as an argument"
                        .into(),
                ),
                _ => Err(self
                    .base_member(n)
                    .or_else(|| self.control_add_in(n))
                    .unwrap_or_else(|| format!("`{}` is not assignable", n.text()))),
            },
        }
    }

    /// The page or report that the page or report extension being lowered extends, with the noun
    /// for messages and whether it is a report (ADR-023).
    fn base_object(&self) -> Option<(usize, &'static str, bool)> {
        let (noun, report) = match self.ws.objects[self.oi].kind {
            ObjKind::PageExtension => ("page", false),
            ObjKind::ReportExtension => ("report", true),
            _ => return None,
        };
        Some((self.ws.extension_target(self.oi)?, noun, report))
    }

    /// Variable `name` (lower case) of the base object, seen from the extension being lowered
    /// (ADR-023): its slot among the base object's globals, and its type. Refused until the probes
    /// of ADR-023 pin the access.
    fn base_var(&self, name: &str) -> L<(usize, Ty, Option<String>)> {
        let (base, noun, report) = self.base_object().ok_or("a base variable outside an extension")?;
        let probe = if report { "R1" } else { "P1/P2" };
        if !(if report {
            crate::base_access::pinned("R1")
        } else {
            crate::base_access::pinned("P1") && crate::base_access::pinned("P2")
        }) {
            return Err(crate::base_access::unpinned_reason(&format!("`{name}`"), probe, noun));
        }
        let ast = self.ws.object_ast(base);
        let (_, decl) = crate::base_access::visible_vars(&ast)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("`{name}` is no protected variable of the {noun} it extends"))?;
        let ty = decl.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
        // Reference-like values (a record, a list, a dialog) are probe P5 (page) or R3 (report).
        let simple = matches!(
            ty,
            Ty::Integer
                | Ty::BigInteger
                | Ty::Decimal
                | Ty::Boolean
                | Ty::Char
                | Ty::Byte
                | Ty::Text(_)
                | Ty::Code(_)
                | Ty::Date
                | Ty::Time
                | Ty::DateTime
                | Ty::Duration
                | Ty::Guid
                | Ty::Option(_)
                | Ty::Enum(_)
                | Ty::Label
        );
        let kind_probe = if report { "R3" } else { "P5" };
        if !simple && !crate::base_access::pinned(kind_probe) {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` (a {ty} variable)"),
                kind_probe,
                noun,
            ));
        }
        // A control's source expression is probe P9.
        if !report && !crate::base_access::pinned("P9") && self.in_synthetic_routine() {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` in a control's source expression"),
                "P9",
                noun,
            ));
        }
        let items = if report {
            let l = self.layouts.report(base).ok_or("the base report has no layout")?;
            let mut seen = std::collections::HashSet::new();
            Some(l.items.iter().filter(|i| seen.insert(i.name.to_lowercase())).count())
        } else {
            None
        };
        let slot = crate::base_access::slot_of(&ast, items, name)
            .ok_or_else(|| format!("`{name}` has no slot in the {noun} it extends"))?;
        let label = decl.type_ref().as_ref().and_then(label_text);
        Ok((slot, ty, label))
    }

    /// A procedure of the extension with the name of a callable procedure of its base is compile
    /// error AL0440 in BC (corpus handlers/TestPageExtensionBaseAccessContracts.al:20), so it is
    /// refused: BC never runs such code.
    fn declared_in_both_proc(&self, object: usize, offset: usize) -> L<()> {
        if !crate::base_access::scope_enabled() {
            return Ok(());
        }
        let Some((base, noun, _)) = self.base_object() else {
            return Ok(());
        };
        let Some(name) = self.members[object]
            .procedures
            .iter()
            .find(|p| p.offset == offset)
            .map(|p| p.name.clone())
        else {
            return Ok(());
        };
        // Only the same parameter types are AL0440; a different signature is an overload (corpus
        // pageext/POPageExtOverload.al on the staging branch).
        let mine = ast::routines_in(self.ws.object_ast(object).syntax())
            .into_iter()
            .find(|r| usize::from(r.syntax().text_range().start()) == offset);
        let same_signature = |b: &ast::Routine| {
            mine.as_ref()
                .is_none_or(|m| crate::alcodes::signature(m) == crate::alcodes::signature(b))
        };
        if crate::base_access::visible_procs(&self.ws.object_ast(base), &name).any(|b| same_signature(&b)) {
            return Err(format!(
                "AL0440: the {noun} extension declares a method `{name}` that the {noun} it extends already defines (BC does not compile it)"
            ));
        }
        Ok(())
    }

    /// Whether extension code may call routine `routine` of its base object (ADR-023).
    fn base_call_allowed(&self, object: usize, routine: usize) -> L<()> {
        let (_, noun, report) = self.base_object().ok_or("a base call outside an extension")?;
        let (probe, ok) = if report {
            ("R1", crate::base_access::pinned("R1"))
        } else {
            ("P4", crate::base_access::pinned("P4"))
        };
        let node = self
            .routine_nodes
            .get(object)
            .get(routine)
            .ok_or("base procedure not found")?;
        let name = node.name();
        if !ok {
            return Err(crate::base_access::unpinned_reason(&format!("`{name}`"), probe, noun));
        }
        if !report && !crate::base_access::pinned("P10") && self.in_synthetic_routine() {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` in a control's source expression"),
                "P9/P10",
                noun,
            ));
        }
        for a in node.attributes() {
            let attr = a.name();
            if attr.eq_ignore_ascii_case("tryfunction") || attr.to_ascii_lowercase().ends_with("event") {
                return Err(format!(
                    "`{name}` is a [{attr}] procedure of the {noun} it extends: calling it from {noun} extension code is not lowered (ADR-023, refusals)"
                ));
            }
        }
        Ok(())
    }

    /// Whether the routine being lowered was made from a property expression (a control's source).
    fn in_synthetic_routine(&self) -> bool {
        self.routine_nodes
            .get(self.oi)
            .get(self.routine_idx)
            .is_some_and(|r| crate::pages::is_synthetic(&r.name()))
    }

    /// `CurrPage.Chart` where `Chart` is a `usercontrol` of this page: a control add-in, which the runtime
    /// does not model (no corpus test uses one; Base Application's chart pages pass theirs to a `var`
    /// parameter).
    fn control_add_in(&self, n: &SyntaxNode) -> Option<String> {
        let text = n.text().to_string();
        let name = text
            .trim()
            .strip_prefix("CurrPage.")?
            .trim()
            .trim_matches('"')
            .to_string();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ') {
            return None;
        }
        let object = self
            .ws
            .object_ast(self.oi)
            .syntax()
            .text()
            .to_string()
            .to_ascii_lowercase();
        let lower = name.to_ascii_lowercase();
        (object.contains(&format!("usercontrol({lower};")) || object.contains(&format!("usercontrol(\"{lower}\";")))
            .then(|| format!("`CurrPage.{name}` is a control add-in (usercontrol): control add-ins are not modelled"))
    }

    /// Why a name cannot be lowered in a page or report extension: it is a variable or procedure of
    /// the object the extension extends. Extension code runs on its own object, which holds none of
    /// the base object's variables or procedures (a bare control name binds to a base variable
    /// through `Binding::BaseVar`, corpus handlers/TestRequestPageControlErrorName.al). A report
    /// extension reaches the `protected` ones of its base report (Microsoft's "Mfg. Adjust Add. Rep.
    /// Currency" calls `Window.Open(..)` and `ExchangeAmtLCYToFCY(..)`).
    fn base_member(&self, n: &SyntaxNode) -> Option<String> {
        let noun = match self.ws.objects[self.oi].kind {
            ObjKind::PageExtension => "page",
            ObjKind::ReportExtension => "report",
            _ => return None,
        };
        let base = self.ws.extension_target(self.oi)?;
        let root = n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))?;
        let name = ast::ident_text(&root);
        let obj = self.ws.object_ast(base);
        let declared = obj
            .var_decls()
            .any(|v| v.names().iter().any(|x| x.eq_ignore_ascii_case(&name)))
            || obj.routines().any(|r| r.name().eq_ignore_ascii_case(&name));
        declared.then(|| {
            if crate::base_access::scope_enabled() {
                // The checker already resolves what extension code may name (ADR-023), so this one
                // is `local`, or a variable that is not `protected`.
                // AL0161 in BC (corpus handlers/TestPageExtensionBaseAccessContracts.al, header; the text
                // is the compiler's): BC never runs such code.
                let is_proc = obj.routines().any(|r| r.name().eq_ignore_ascii_case(&name))
                    && !obj.var_decls().any(|v| v.names().iter().any(|x| x.eq_ignore_ascii_case(&name)));
                let shown = if is_proc { format!("{name}()") } else { name.clone() };
                return format!(
                    "'{shown}' is inaccessible due to its protection level (a variable of the {noun} it extends that is not `protected`, or a `local` procedure; AL0161)"
                );
            }
            format!("`{name}` is a variable or procedure of the {noun} it extends: {noun} extension code on base {noun} members is not lowered yet (ADR-023: built, off until its probes ran)")
        })
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
                // `0T` is the blank time, `000000T` is midnight
                // (types/TestFormatEvaluateTextContracts.al:Format_Time_StandardFormats).
                if d.chars().all(|c| c == '0') && d.len() < 6 {
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
            // `this` in a codeunit is the running instance (corpus handlers/ProbeEventSubscriberOrder.al:
            // `BindSubscription(this)`).
            NameRef
                if crate::check::is_this_keyword(n)
                    && self.ws.objects[self.oi].kind == ObjKind::Codeunit
                    && !self.globals.contains_key("this") =>
            {
                Ok(ir::Expr::ThisInstance)
            }
            ParenExpr => self.expr(&kids[0]),
            PrefixExpr => {
                let op = match sig_tokens(n)[0].text().to_ascii_lowercase().as_str() {
                    "not" => ir::UnOp::Not,
                    "-" => ir::UnOp::Neg,
                    _ => ir::UnOp::Plus,
                };
                Ok(ir::Expr::Unary(
                    op,
                    Box::new(self.operand(&kids[0], ir::OperandUse::Value)?),
                ))
            }
            BinExpr if matches!(sig_tokens(n)[0].text().to_ascii_lowercase().as_str(), "is" | "as") => {
                self.interface_test(n, &kids)
            }
            // The parser recovered from text it did not accept and left one operand.
            BinExpr if kids.len() < 2 => {
                Err("binary expression with a missing operand (the parser did not accept it)".into())
            }
            BinExpr => {
                let op = sig_tokens(n)[0].text().to_ascii_lowercase();
                // How a settled DotNet operand may be used depends on the operator (ADR-024).
                let operand_use = match op.as_str() {
                    "+" | "=" | "<>" => ir::OperandUse::Enum,
                    "<" | "<=" | ">" | ">=" => ir::OperandUse::NoText,
                    _ => ir::OperandUse::Value,
                };
                if op == "in" {
                    self.no_dotnet_operand(&kids[0])?;
                }
                let l = if op == "in" {
                    self.expr(&kids[0])?
                } else {
                    self.operand(&kids[0], operand_use)?
                };
                if op == "in" {
                    let set = &kids[1];
                    if set.kind() != SetExpr {
                        return Err("`in` with a non-literal set is not lowered yet".into());
                    }
                    let elems = set.children().map(|e| self.set_elem(&e)).collect::<L<Vec<_>>>()?;
                    return Ok(ir::Expr::In(Box::new(l), elems));
                }
                let r = self.operand(&kids[1], operand_use)?;
                let dotnet =
                    matches!(self.ty_of(&kids[0]), Ty::DotNet(_)) || matches!(self.ty_of(&kids[1]), Ty::DotNet(_));
                let op = match op.as_str() {
                    "=" if dotnet => ir::BinOp::DotNetEq,
                    "<>" if dotnet => ir::BinOp::DotNetNe,
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
            TernaryExpr if kids.iter().any(|k| matches!(self.ty_of(k), Ty::DotNet(_))) => {
                Err("a DotNet value in a conditional expression: its type is only known at run time".into())
            }
            // `Text[i]` read: the character at i. The base is any text, a call result included
            // (corpus json/TestJsonXmlDeepContracts.al:71 `Format(10, 0, '<Integer>')[1]`).
            IndexExpr
                if matches!(kids.first().map(|k| self.ty_of(k)), Some(Ty::Text(_) | Ty::Code(_)))
                    && kids.len() == 2 =>
            {
                Ok(ir::Expr::CharAt(
                    Box::new(self.expr(&kids[0])?),
                    Box::new(self.operand(&kids[1], ir::OperandUse::Value)?),
                ))
            }
            IndexExpr if matches!(kids.first().map(|k| self.ty_of(k)), Some(Ty::DotNet(_))) => {
                Err("indexing a DotNet value is not lowered yet (call get_Item / GetValue instead)".into())
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
                // `Mode::Decompress` with `Mode: DotNet CompressionMode`: the type's static member.
                Some(Res::DotNetMember(name)) => {
                    let Ty::DotNet(alias) = self.ty_of(n) else {
                        return Err(format!("`{name}` is not a member of a DotNet value"));
                    };
                    let ty = self.ws.resolve_dotnet(self.file, &alias).ok_or_else(|| {
                        format!("DotNet '{alias}' has no dotnet {{ }} declaration in this app or its dependencies")
                    })?;
                    Ok(ir::Expr::DotNet {
                        ty: Some(ty),
                        receiver: Some(Box::new(self.expr(&kids[0])?)),
                        op: ir::DotNetOp::Get(name),
                        args: Vec::new(),
                    })
                }
                Some(Res::EnumValue { object, value }) => Ok(ir::Expr::Const(ir::Const::Enum(object as u32, value))),
                Some(Res::OptionValue(i)) => match self.ty_of(n) {
                    Ty::Option(m) => Ok(ir::Expr::Const(ir::Const::Option(i, m))),
                    Ty::Builtin(b) if al_builtins::sysenum::members(&b).is_some() => Ok(ir::Expr::Const(
                        ir::Const::Option(i, al_builtins::sysenum::option_members(&b).expect("checked")),
                    )),
                    _ => Err("option value without its option type".into()),
                },
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                _ => {
                    // `Enum::Member` of a system enum whose member list has no such member.
                    let text = n.text().to_string();
                    match text.split_once("::") {
                        Some((e, m)) if al_builtins::sysenum::members(e.trim()).is_some() => Err(format!(
                            "`{}::{}` is not a member of the system enum {} in the member list this runtime holds, which only holds the members a corpus probe lists: its ordinal is not pinned (docs/probes-needed.md)",
                            e.trim(),
                            m.trim().trim_matches('"'),
                            e.trim()
                        )),
                        _ => Err(format!("`{text}` is not lowered yet")),
                    }
                }
            },
            MemberExpr if self.is_control_access(n) => self.control(n),
            // `CurrPage.<Part>.Page`: the page the part shows.
            MemberExpr if self.is_curr_page_part_page(n) => {
                let part = n.children().next().ok_or("part page without a part")?;
                self.control(&part)
            }
            NameRef | MemberExpr | IndexExpr => match self.res(n).cloned() {
                Some(Res::Proc { .. }) | Some(Res::Builtin(_)) | Some(Res::DotNetMember(_)) => self.call(n, n, &[]),
                // `Q.Amount`: the current row's value of a query column.
                Some(Res::QueryColumn { query, column })
                    if n.kind() == MemberExpr
                        && query == self.oi
                        && kids[0].kind() == NameRef
                        && kids[0].text().to_string().trim().eq_ignore_ascii_case("this") =>
                {
                    // `this.Id` (corpus query/PRQueryOpenColumn.al `PeekThis`).
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    let slot = *self.globals.get("currquery").ok_or("the running query")?;
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(ir::Expr::Load(ir::Place::Global(slot))),
                        column,
                    })
                }
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
                // A bare column name in the query's own code reads the running query's current row
                // (Base Application's "Summarized Stock By Item Trkg." does `X := Serial_No_;`; no corpus test
                // does, docs/probes-needed.md).
                Some(Res::QueryColumn { query, column }) if n.kind() == NameRef && query == self.oi => {
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    let slot = *self.globals.get("currquery").ok_or("the running query")?;
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(ir::Expr::Load(ir::Place::Global(slot))),
                        column,
                    })
                }
                // `Database::A.B."Name"`: a namespace-qualified object id.
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                // `this.BaseVar` in a reportextension compiles and raises when it runs (types/PYThisExtensions.al:
                // ReportExtension_ThisBaseVariable_ReadInATrigger, BC 27.5 and 29.0). For a page extension BC aborts the
                // page with a text this corpus does not pin.
                Some(Res::ThisBaseGlobal(_)) => match (self.ws.objects[self.oi].kind, self.ws.objects[self.oi].id) {
                    (ObjKind::ReportExtension, Some(id)) => Ok(ir::Expr::Fail {
                        message: format!(
                            "The another object refers to a global variable on ReportExtension with ID {id} that no longer seems to exists. Try to recompile dependent extensions."
                        ),
                        code: "GlobalVariableNotFound".into(),
                    }),
                    _ => Err("`this.<variable of the base>` in this extension (what BC raises is not pinned)".into()),
                },
                Some(Res::Local(name)) if self.local_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.local_labels[&name].clone())))
                }
                Some(Res::Global(name)) if self.global_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.global_labels[&name].clone())))
                }
                // A Label of the base object is its text.
                Some(Res::BaseGlobal(name)) if self.base_var(&name)?.2.is_some() => Ok(ir::Expr::Const(
                    ir::Const::Text(self.base_var(&name)?.2.unwrap_or_default()),
                )),
                // `CurrFieldNo` in table code: a property of the implicit `Rec` the VM reads.
                Some(Res::Global(name)) if name == "currfieldno" && !self.globals.contains_key(&name) => {
                    let rec = self
                        .rec_slot
                        .ok_or("CurrFieldNo outside code that has an implicit Rec")?;
                    Ok(ir::Expr::Builtin {
                        id: "Record.CurrFieldNo#1".into(),
                        receiver: Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec)))),
                        args: Vec::new(),
                        ret_used: true,
                    })
                }
                // `GetLocation(Code)."Bin Mandatory"`: a field of what a procedure returns. A call result is not a
                // place, so the base is evaluated and the field read off the record it gives. No corpus test
                // reads a field of a returned record (docs/probes-needed.md).
                Some(Res::Field { table, id })
                    if n.kind() == MemberExpr && kids.first().is_some_and(|b| b.kind() == SyntaxKind::CallExpr) =>
                {
                    let pos = self.readable_field_pos(table, id)?;
                    Ok(ir::Expr::FieldOf(Box::new(self.expr(&kids[0])?), pos))
                }
                _ => Ok(ir::Expr::Load(self.place(n)?)),
            },
            other => Err(format!("expression {other:?} `{}` is not lowered yet", n.text())),
        }
    }

    /// `TP.Ctl`: a member access on a test page or part that names a control.
    fn is_control_access(&self, n: &SyntaxNode) -> bool {
        let Some(recv) = n.children().next() else { return false };
        let request = matches!(self.ty_of(&recv), Ty::TestRequestPage(_));
        (matches!(self.ty_of(&recv), Ty::TestPage(_) | Ty::TestPart(_)) || request)
            && (matches!(self.ty_of(n), Ty::TestPart(_))
                || matches!(self.ty_of(n), Ty::Builtin(b) if b == "TestField" || b == "TestAction")
                // A data item of a request page.
                || (request && matches!(self.ty_of(n), Ty::TestFilter(_))))
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
        self.control_named(&recv, &name)
    }

    /// The control `name` of the test page (or part, or request page) that `recv` evaluates to.
    fn control_named(&mut self, recv: &SyntaxNode, name: &str) -> L<ir::Expr> {
        let (Ty::TestPage(page) | Ty::TestPart(page) | Ty::Object(ObjKind::Page, page) | Ty::TestRequestPage(page)) =
            self.ty_of(recv)
        else {
            return Err("control of a non-page".into());
        };
        let kind = if matches!(self.ty_of(recv), Ty::TestRequestPage(_)) {
            ObjKind::Report
        } else {
            ObjKind::Page
        };
        let page = self.resolve_obj(kind, &page)?;
        self.embeds.borrow_mut().insert(page);
        let layout = self.layouts.page_layout(page).ok_or("page layout")?;
        let control = layout
            .controls
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("control `{name}` is not in the page layout"))?;
        Ok(ir::Expr::Control {
            page: Box::new(self.expr(recv)?),
            control,
        })
    }

    /// An expression used as an operand. A `DotNet` value has no static type: it is settled when
    /// the code runs, from the kind the host returned (ADR-024).
    fn operand(&mut self, n: &SyntaxNode, how: ir::OperandUse) -> L<ir::Expr> {
        let e = self.expr(n)?;
        Ok(match self.ty_of(n) {
            Ty::DotNet(_) => ir::Expr::DotNetOperand(Box::new(e), how),
            _ => e,
        })
    }

    /// The left side of `in` keeps the static refusal: a membership test against a set of
    /// literals is not pinned for a settled value (ADR-024, refusals that stay).
    fn no_dotnet_operand(&self, n: &SyntaxNode) -> L<()> {
        match self.ty_of(n) {
            Ty::DotNet(_) => Err(format!(
                "`{}` is a DotNet value used as an operand: its type is only known at run time; assign it to a typed variable first",
                n.text().to_string().trim()
            )),
            _ => Ok(()),
        }
    }

    /// A condition: a DotNet result must be a Boolean.
    fn condition(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        let e = self.expr(n)?;
        Ok(match self.ty_of(n) {
            Ty::DotNet(a) if a.is_empty() => ir::Expr::DotNetCoerce(RtType::Boolean, Box::new(e)),
            _ => e,
        })
    }

    /// An argument of a DotNet member: a plain variable goes by place, so a by-reference .NET
    /// parameter can write it back (ADR-014 section 4); anything else by value.
    fn dotnet_arg(&mut self, a: &SyntaxNode) -> L<ir::Arg> {
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
    fn dotnet_member(
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

    /// A call resolved on `node` (the CallExpr, or the callee for a call without parentheses).
    fn call(&mut self, node: &SyntaxNode, callee: &SyntaxNode, args: &[SyntaxNode]) -> L<ir::Expr> {
        if let Some(Res::DotNetMember(name)) = self.res(node).cloned() {
            let args = args.iter().map(|a| self.dotnet_arg(a)).collect::<L<Vec<_>>>()?;
            let op = if node.kind() == SyntaxKind::CallExpr {
                ir::DotNetOp::Call(name.clone())
            } else {
                ir::DotNetOp::Get(name.clone())
            };
            return self.dotnet_member(node, callee, &name, op, args);
        }
        // `this.Proc(...)` calls on the running instance, as the bare call does.
        let receiver_node = (callee.kind() == SyntaxKind::MemberExpr)
            .then(|| callee.children().next())
            .flatten()
            .filter(|r| !crate::check::is_this_keyword(r));
        match self.res(node).cloned() {
            Some(Res::Proc { object, offset }) if self.ws.objects[object].kind == ObjKind::Interface => {
                self.interface_call(object, offset, receiver_node.as_ref(), args)
            }
            Some(Res::Proc { object, offset }) => {
                let routine = self.routine_at.get(&(object, offset)).ok_or("procedure not found")?;
                let sig = self.members[object]
                    .procedures
                    .iter()
                    .find(|p| p.offset == offset)
                    .ok_or("procedure signature")?
                    .clone();
                let instance = match &receiver_node {
                    // `this.Proc()` runs on the current instance, like a bare call.
                    Some(r)
                        if r.kind() == SyntaxKind::NameRef
                            && r.text().to_string().trim().eq_ignore_ascii_case("this") =>
                    {
                        if object != self.oi {
                            return Err("`this` calls a procedure of another object".into());
                        }
                        None
                    }
                    Some(r)
                        if matches!(
                            self.ty_of(r),
                            Ty::Object(
                                ObjKind::Codeunit | ObjKind::Page | ObjKind::XmlPort | ObjKind::Report | ObjKind::Query,
                                _
                            ) | Ty::Record { .. }
                        ) =>
                    {
                        Some(Box::new(self.expr(r)?))
                    }
                    Some(_) => return Err("procedure calls on this receiver are not lowered yet".into()),
                    None if object == self.oi => {
                        self.declared_in_both_proc(object, offset)?;
                        None
                    }
                    // A page or report extension calling a procedure of the page or report it extends.
                    None if self.base_object().is_some_and(|(b, _, _)| b == object) => {
                        self.base_call_allowed(object, routine)?;
                        Some(Box::new(ir::Expr::BaseInstance))
                    }
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
                        out.push(self.by_var_arg(a)?);
                    } else {
                        let value = self.expr(a)?;
                        out.push(ir::Arg::Value(self.convert_to(&p.ty, &self.ty_of(a), value)?));
                    }
                }
                self.calls.push((object, routine));
                // The routine node is the one `routine_at` numbered, so no search by offset.
                let try_call = self
                    .routine_nodes
                    .get(object)
                    .get(routine)
                    .is_some_and(|r| r.attributes().any(|a| a.name().eq_ignore_ascii_case("tryfunction")));
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
                    // Without a dimension the total number of elements
                    // (streams/TestStreamsRuntimeContracts.al:ArrayLen_MultiDimensional_WithAndWithoutDimension).
                    None => {
                        let total: u32 = dims.iter().product();
                        return Ok(ir::Expr::Const(ir::Const::Integer(total as i32)));
                    }
                };
                match dim.and_then(|k| usize::try_from(k - 1).ok()).and_then(|k| dims.get(k)) {
                    Some(n) => Ok(ir::Expr::Const(ir::Const::Integer(*n as i32))),
                    None => Err("ArrayLen with a dimension outside the array (error text not pinned)".into()),
                }
            }
            Some(Res::Builtin(id)) => {
                if al_builtins::native::lookup(&id).is_none() {
                    return Err(match al_builtins::native::refusal(&id) {
                        Some(why) => format!("builtin {id}: {why} (docs/probes-needed.md)"),
                        None => format!("builtin {id} is not implemented"),
                    });
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
                            && matches!(
                                self.ws.objects[self.oi].kind,
                                ObjKind::Report | ObjKind::ReportExtension
                            )
                            && r.text().to_string().trim().eq_ignore_ascii_case("CurrReport") =>
                    {
                        None
                    }
                    // `RequestOptionsPage` is the request page of the report whose code is running; it has
                    // no variable to take as a receiver.
                    Some(r)
                        if sig.instance
                            && sig.receiver == "RequestPage"
                            && matches!(
                                self.ws.objects[self.oi].kind,
                                ObjKind::Report | ObjKind::ReportExtension
                            )
                            && r.text().to_string().trim().eq_ignore_ascii_case("RequestOptionsPage") =>
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
                    // A Label is its text: the variable holds none (reads lower to the constant).
                    Some(r) if sig.instance && sig.receiver == "Label" => Some(Box::new(ir::Arg::Value(self.expr(r)?))),
                    Some(r) if sig.instance => Some(Box::new(match self.place(r) {
                        Ok(p) => ir::Arg::Ref(p),
                        Err(_) => ir::Arg::Value(self.expr(r)?),
                    })),
                    // An unqualified record method in table code runs on the implicit Rec.
                    None if sig.instance && matches!(sig.receiver.as_str(), "Record" | "Query") => {
                        Some(Box::new(ir::Arg::Ref(ir::Place::Global(
                            self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?,
                        ))))
                    }
                    _ => None,
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig
                        .params
                        .get(i)
                        .or_else(|| sig.params.last().filter(|p| p.variadic))
                        .ok_or("too many arguments")?;
                    // `Clear(X)` of a declared DotNet variable goes by place like any other variable.
                    let clears_dotnet =
                        id == "System.Clear#1" && matches!(self.ty_of(a), Ty::DotNet(ref al) if !al.is_empty());
                    if let (false, Ty::DotNet(alias)) = (clears_dotnet, self.ty_of(a)) {
                        // `IsNull(DotNet)` takes the value as it is. A `Variant` parameter takes a
                        // primitive, an object or an enum as it is (`Format` of an object is its
                        // `ToString()`). A scalar parameter takes a DotNet result (no static type)
                        // when its run-time kind fits the parameter, as in an assignment (ADR-024
                        // section 2). Anything else is refused.
                        let scalar = match p.ty.as_str() {
                            "Text" | "Code" => Some(RtType::Text(None)),
                            "Integer" => Some(RtType::Integer),
                            "BigInteger" => Some(RtType::BigInteger),
                            "Decimal" => Some(RtType::Decimal),
                            "Boolean" => Some(RtType::Boolean),
                            "Char" => Some(RtType::Char),
                            "Byte" => Some(RtType::Byte),
                            "Date" => Some(RtType::Date),
                            "Time" => Some(RtType::Time),
                            "DateTime" => Some(RtType::DateTime),
                            "Duration" => Some(RtType::Duration),
                            "Guid" => Some(RtType::Guid),
                            _ => None,
                        };
                        let value = match p.ty.as_str() {
                            "DotNet" => self.expr(a)?,
                            // CopyStream takes a DotNet stream as it is; the VM copies the bytes
                            // through the host (ADR-024 section 3).
                            "InStream" | "OutStream" if !p.by_var && id.starts_with("System.CopyStream#") => {
                                self.expr(a)?
                            }
                            "Any" | "Variant" if !p.by_var => {
                                ir::Expr::DotNetOperand(Box::new(self.expr(a)?), ir::OperandUse::Variant)
                            }
                            _ => match scalar {
                                Some(rt) if !p.by_var && alias.is_empty() => ir::Expr::DotNetCoerce(
                                    rt,
                                    Box::new(ir::Expr::DotNetOperand(Box::new(self.expr(a)?), ir::OperandUse::Strict)),
                                ),
                                _ => {
                                    return Err(format!(
                                        "a DotNet value passed to builtin {id}: its type is only known at run time; assign it to a typed variable first"
                                    ));
                                }
                            },
                        };
                        out.push(ir::Arg::Value(value));
                        continue;
                    }
                    // A bare control name in a field position of a test page method.
                    if a.kind() == SyntaxKind::NameRef
                        && self.res(a).is_none()
                        && matches!(self.ty_of(a), Ty::Builtin(ref b) if b == "TestField")
                        && receiver_node.as_ref().is_some_and(|r| {
                            matches!(
                                self.ty_of(r),
                                Ty::TestPage(_) | Ty::TestRequestPage(_) | Ty::TestPart(_)
                            )
                        })
                    {
                        let name = a
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
                            .map(|t| ast::ident_text(&t))
                            .ok_or("control name")?;
                        let recv = receiver_node.clone().expect("checked");
                        out.push(ir::Arg::Value(self.control_named(&recv, &name)?));
                        continue;
                    }
                    // `Source.CopyFilter(FromField, Target."ToField")`: the target is a field of another
                    // record variable, passed as that record and the field's position.
                    if id == "Record.CopyFilter#1" && i == 1 {
                        let Some(Res::Field { table, id: fid }) = self.res(a).cloned() else {
                            return Err("CopyFilter to something that is not a field of a record".into());
                        };
                        let pos = self.readable_field_pos(table, fid)?;
                        let base = match a.kind() {
                            SyntaxKind::MemberExpr => self.place(&a.children().next().ok_or("member base")?)?,
                            _ => ir::Place::Global(
                                self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?,
                            ),
                        };
                        out.push(ir::Arg::Ref(base));
                        out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(pos as u32))));
                        continue;
                    }
                    // A parameter named Field... that is an Integer takes the number as a VALUE (`RecRef.Field(Buf."Field No.")`,
                    // `RecRef.FieldExist(R.Number)`); a field reference is passed for a parameter that names a field.
                    let field_param = (p.name.to_ascii_lowercase().starts_with("field") && p.ty != "Integer")
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
                    // `Page.FindFirstField(Name, v)` names the control bare. The corpus only
                    // qualifies it (`ListPage."Integer Field"`, handlers/TestBCReportHandlerContracts.al:247);
                    // what a bare name means (the receiver's controls first?) is not pinned.
                    if p.ty == "TestField"
                        && a.kind() != SyntaxKind::MemberExpr
                        && !matches!(self.res(a), Some(Res::Local(_) | Res::Global(_)))
                    {
                        return Err(format!(
                            "the TestField argument `{}` is a bare control name: only the qualified form is pinned by a corpus test",
                            a.text()
                        ));
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
                    } else if id.starts_with("System.Format#") && i == 0 {
                        ir::Arg::Value(self.format_option_field(args.len() == 1, a)?)
                    // The substituted values of StrSubstNo, Message and Error are `Format`ted: an Option
                    // field shows its caption (corpus xmlport/PWXmlPortFormatEvaluate.al:Import_Xml_Option,
                    // `%3` of a field with OptionCaption).
                    } else if i > 0
                        && (id.starts_with("Text.StrSubstNo#")
                            || id.starts_with("Dialog.Message#")
                            || id.starts_with("Dialog.Error#"))
                    {
                        ir::Arg::Value(self.format_option_field(true, a)?)
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
            // The checker refused a builtin of a newer runtime (ADR-015 3.3): say why.
            None if self.runtime_refusal(node).is_some() => Err(self.runtime_refusal(node).unwrap_or_default()),
            // `Window.Open(..)` in a report extension, where `Window` is a variable of the base report.
            None if self.base_member(callee).is_some() => Err(self.base_member(callee).unwrap_or_default()),
            // `CurrPage.<AddIn>.Method()`: refused when it runs, so code that never reaches it still runs
            // (Base Application's "Item Tracking Lines" calls its barcode add-in only after the client said it
            // is supported). Control add-ins are not modelled; no corpus test calls one.
            None if callee.kind() == SyntaxKind::MemberExpr
                && callee.children().next().and_then(|b| self.control_add_in(&b)).is_some() =>
            {
                let why = callee
                    .children()
                    .next()
                    .and_then(|b| self.control_add_in(&b))
                    .unwrap_or_default();
                Ok(ir::Expr::Refused(format!("call `{}`: {why}", node.text())))
            }
            None => Err(format!("call `{}` has no callee the checker resolved", node.text())),
            Some(r) => Err(format!(
                "call `{}` to {} is not lowered yet",
                node.text(),
                match r {
                    Res::Local(_) | Res::Global(_) => "a variable",
                    Res::Field { .. } | Res::QueryColumn { .. } => "a field",
                    Res::EnumValue { .. } | Res::OptionValue(_) => "an enum or option value",
                    Res::ObjectId(_) => "an object id",
                    _ => "this kind of callee",
                }
            )),
        }
    }

    /// The checker's AR1005 on this call, if any.
    fn runtime_refusal(&self, node: &SyntaxNode) -> Option<String> {
        let path = &self.ws.files[self.file].path;
        let at: usize = node.text_range().start().into();
        self.checked
            .diagnostics
            .iter()
            .find(|d| d.code == "AR1005" && d.offset == at && &d.file == path)
            .map(|d| format!("AR1005 {}", d.message))
    }
}

/// Load, check and lower in one go, against the newest catalog (nothing hidden), with source
/// positions.
pub fn compile(
    apps: Vec<crate::workspace::AppSource>,
    symbols: &std::collections::BTreeSet<String>,
) -> (Workspace, Lowered) {
    let kinds = vec![crate::workspace::AppKind::Workspace; apps.len()];
    compile_apps(apps, kinds, symbols)
}

/// Like [`compile`], with the kind of each app (dependency apps, ADR-013).
pub fn compile_apps(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
) -> (Workspace, Lowered) {
    compile_with(apps, kinds, symbols, &al_builtins::catalog::Catalog::load())
}

/// Like [`compile`], for a run target: builtins newer than the target's runtime are
/// refused (ADR-015 3.3).
pub fn compile_for(
    apps: Vec<crate::workspace::AppSource>,
    symbols: &std::collections::BTreeSet<String>,
    target: &al_version::Target,
) -> (Workspace, Lowered) {
    let kinds = vec![crate::workspace::AppKind::Workspace; apps.len()];
    compile_apps_for(apps, kinds, symbols, target)
}

/// [`compile_for`] with the kind of each app.
pub fn compile_apps_for(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
    target: &al_version::Target,
) -> (Workspace, Lowered) {
    compile_with(apps, kinds, symbols, &al_builtins::catalog::Catalog::for_target(target))
}

/// [`compile_apps_for`] (or the newest catalog without a target) with extra demand roots: the
/// install baseline (ADR-013 decision 5).
pub fn compile_apps_with(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    roots: crate::check::Roots,
) -> (Workspace, Lowered) {
    let catalog = match target {
        Some(t) => al_builtins::catalog::Catalog::for_target(t),
        None => al_builtins::catalog::Catalog::load(),
    };
    compile_with_roots(apps, kinds, symbols, &catalog, roots)
}

/// The lowered form of the dependency apps `apps`, built by a complete compile of them alone,
/// for a compile cache (ADR-022). The apps are in the order a run puts them: dependencies first.
pub fn lower_for_cache(
    apps: Vec<crate::workspace::AppSource>,
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
) -> crate::provided::DepSet {
    lower_for_cache_with(apps, &[], symbols, target)
}

/// [`lower_for_cache`] where app `i` with `app_symbols[i]` set is preprocessed with those symbols.
pub fn lower_for_cache_with(
    apps: Vec<crate::workspace::AppSource>,
    app_symbols: &[Option<std::collections::BTreeSet<String>>],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
) -> crate::provided::DepSet {
    let catalog = match target {
        Some(t) => al_builtins::catalog::Catalog::for_target(t),
        None => al_builtins::catalog::Catalog::load(),
    };
    let kinds = vec![crate::workspace::AppKind::Dependency; apps.len()];
    let ws = Workspace::load_apps_symbols(apps, kinds, symbols, app_symbols, Vec::new());
    let members = crate::members::build(&ws);
    let roots = crate::check::Roots {
        eager: true,
        ..Default::default()
    };
    let checked = crate::check::check_with(&ws, &members, &catalog, roots);
    let mut set = lower_core(&ws, &members, &checked, true, lower_workers(), None, true)
        .expect("no cache, no cache error")
        .exported
        .expect("exported");
    set.parse_problems = ws.lazy_parse_problems();
    set
}

/// The blocks a run takes from `src` for the first `n_prov` objects of `ws`: the identity of the
/// objects is checked, and a block the apps above them change (`relower_set`) is left out, to be
/// lowered again from source.
fn provided_blocks(
    ws: &Workspace,
    n_prov: usize,
    src: &dyn crate::provided::DepSource,
    decode: bool,
) -> Result<Vec<Option<crate::provided::DepBlock>>, String> {
    use rayon::prelude::*;
    crate::provided::check_identity(ws, n_prov, src)?;
    let relower = crate::provided::relower_set(ws, n_prov, src);
    (0..ws.objects.len())
        .into_par_iter()
        .map(|oi| {
            if oi < n_prov && !relower[oi] {
                if decode {
                    src.block(oi).map(Some)
                } else {
                    Ok(Some(crate::provided::DepBlock::placeholder()))
                }
            } else {
                Ok(None)
            }
        })
        .collect()
}

/// One layer of the dependency set (ADR-022, owner decision 2026-10-15). `apps` are the apps of
/// the layers below followed by the layer's own, in program order; `n_below_apps` is how many
/// belong to the layers below; `below` describes their objects, which come first in the program.
/// The layer's objects are compiled completely, the objects below are taken from `below` except
/// those the layer changes, which are lowered again. The result holds the layer's own objects and
/// those lowered again, so that the layers below and this one give, object for object, what one
/// complete compile of all the apps gives.
pub fn lower_layer(
    apps: Vec<crate::workspace::AppSource>,
    n_below_apps: usize,
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    below: &dyn crate::provided::DepSource,
) -> Result<crate::provided::DepLayer, String> {
    lower_layer_with(apps, n_below_apps, &[], symbols, target, below)
}

/// [`lower_layer`] where app `i` with `app_symbols[i]` set is preprocessed with those symbols.
pub fn lower_layer_with(
    apps: Vec<crate::workspace::AppSource>,
    n_below_apps: usize,
    app_symbols: &[Option<std::collections::BTreeSet<String>>],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    below: &dyn crate::provided::DepSource,
) -> Result<crate::provided::DepLayer, String> {
    let catalog = match target {
        Some(t) => al_builtins::catalog::Catalog::for_target(t),
        None => al_builtins::catalog::Catalog::load(),
    };
    let kinds = vec![crate::workspace::AppKind::Dependency; apps.len()];
    let ws = Workspace::load_apps_symbols(apps, kinds, symbols, app_symbols, Vec::new());
    let n_prov = below.len();
    if n_prov > ws.objects.len() {
        return Err(format!(
            "the layers below hold {n_prov} objects, the apps have {}",
            ws.objects.len()
        ));
    }
    // A layer's result holds only the objects lowered here, so the blocks below are not decoded.
    let blocks = provided_blocks(&ws, n_prov, below, false)?;
    let provided = Provided { src: below, blocks };
    let members = crate::members::build(&ws);
    let mut roots = crate::check::Roots {
        eager: true,
        ..Default::default()
    };
    roots.provided = (0..ws.objects.len()).map(|oi| provided.has(oi)).collect();
    roots.provided_queries = (0..ws.objects.len())
        .map(|oi| provided.has(oi).then(|| below.facts(oi).query.clone()).flatten())
        .collect();
    let checked = crate::check::check_with(&ws, &members, &catalog, roots);
    let lowered = lower_core(&ws, &members, &checked, true, lower_workers(), Some(provided), true)?;
    let all = lowered.exported.expect("exported");
    let mut layer = crate::provided::DepLayer {
        base_len: n_prov,
        ..Default::default()
    };
    // Facts and direct calls of an object taken from `below` are not complete in the export, and
    // are not needed: only the objects lowered here are kept.
    for (oi, (facts, block)) in all.facts.into_iter().zip(all.blocks).enumerate() {
        if !lowered.provided[oi] {
            layer.objects.push(oi as u32);
            layer.set.facts.push(facts);
            layer.set.blocks.push(block);
        }
    }
    layer.set.parse_problems = ws.lazy_parse_problems_from(n_below_apps);
    Ok(layer)
}

/// [`compile_apps_with`] for a run whose dependency apps (first in `apps`) are described by `src`:
/// their objects come from it, except the ones the workspace changes (see `provided.rs`). The
/// result is the complete (eager) compile. An error says the source does not fit the apps or is
/// damaged; nothing was compiled then.
pub fn compile_apps_cached(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    roots: crate::check::Roots,
    src: &dyn crate::provided::DepSource,
) -> Result<(Workspace, Lowered), Box<(Workspace, String)>> {
    let catalog = match target {
        Some(t) => al_builtins::catalog::Catalog::for_target(t),
        None => al_builtins::catalog::Catalog::load(),
    };
    let started = std::time::Instant::now();
    let ws = Workspace::load_apps(apps, kinds, symbols);
    let load = started.elapsed();
    compile_loaded_cached(ws, load, &catalog, roots, src)
}

/// [`compile_apps_cached`] on a workspace that is loaded. An error gives the workspace back, so
/// the caller can compile it without the cache.
pub fn compile_loaded_cached(
    ws: Workspace,
    load: std::time::Duration,
    catalog: &al_builtins::catalog::Catalog,
    mut roots: crate::check::Roots,
    src: &dyn crate::provided::DepSource,
) -> Result<(Workspace, Lowered), Box<(Workspace, String)>> {
    let blocks_started = std::time::Instant::now();
    let prepared = (|| {
        let n_dep = crate::provided::dependency_prefix(&ws).ok_or("a dependency app follows a workspace app")?;
        provided_blocks(&ws, n_dep, src, true)
    })();
    let blocks = match prepared {
        Ok(b) => b,
        Err(why) => return Err(Box::new((ws, why))),
    };
    let provided = Provided { src, blocks };
    let cache_blocks = blocks_started.elapsed();
    let started = std::time::Instant::now();
    let members = crate::members::build(&ws);
    let members_time = started.elapsed();
    roots.eager = true;
    roots.provided = (0..ws.objects.len()).map(|oi| provided.has(oi)).collect();
    roots.provided_queries = (0..ws.objects.len())
        .map(|oi| provided.has(oi).then(|| src.facts(oi).query.clone()).flatten())
        .collect();
    let checked = crate::check::check_with(&ws, &members, catalog, roots);
    let started = std::time::Instant::now();
    let mut lowered = lower_core(&ws, &members, &checked, true, lower_workers(), Some(provided), false)
        .expect("a cache that passed its checks lowers");
    lowered.timings = CompileTimings {
        load,
        members: members_time,
        check: checked.timings,
        lower: started.elapsed(),
        cache_blocks,
    };
    drop(members);
    Ok((ws, lowered))
}

fn compile_with(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
    catalog: &al_builtins::catalog::Catalog,
) -> (Workspace, Lowered) {
    compile_with_roots(apps, kinds, symbols, catalog, Default::default())
}

fn compile_with_roots(
    apps: Vec<crate::workspace::AppSource>,
    kinds: Vec<crate::workspace::AppKind>,
    symbols: &std::collections::BTreeSet<String>,
    catalog: &al_builtins::catalog::Catalog,
    roots: crate::check::Roots,
) -> (Workspace, Lowered) {
    let started = std::time::Instant::now();
    let ws = Workspace::load_apps(apps, kinds, symbols);
    let load = started.elapsed();
    compile_loaded(ws, load, catalog, roots)
}

/// The compile of a workspace that is loaded (`load` is how long that took, for the timings).
pub fn compile_loaded(
    ws: Workspace,
    load: std::time::Duration,
    catalog: &al_builtins::catalog::Catalog,
    roots: crate::check::Roots,
) -> (Workspace, Lowered) {
    let started = std::time::Instant::now();
    let members = crate::members::build(&ws);
    let members_time = started.elapsed();
    let checked = crate::check::check_with(&ws, &members, catalog, roots);
    let started = std::time::Instant::now();
    let mut lowered = lower(&ws, &members, &checked);
    lowered.timings = CompileTimings {
        load,
        members: members_time,
        check: checked.timings,
        lower: started.elapsed(),
        cache_blocks: Default::default(),
    };
    drop(checked);
    drop(members);
    (ws, lowered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;

    fn lower_src(src: &str) -> (Workspace, Lowered) {
        lower_files(&[("f.al", src)], false)
    }

    /// Like `lower_src`, with source positions (the way the runner compiles).
    fn lower_pos(src: &str) -> (Workspace, Lowered) {
        lower_files(&[("f.al", src)], true)
    }

    fn lower_files(files: &[(&str, &str)], positions: bool) -> (Workspace, Lowered) {
        let ws = Workspace::load(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
            }],
            &Default::default(),
        );
        let members = crate::members::build(&ws);
        let checked = crate::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
        let lowered = lower_with_positions(&ws, &members, &checked, positions);
        (ws, lowered)
    }

    /// Tests-ERM (Microsoft's BaseApp bucket): a page field whose source is a lone quoted name with
    /// a hyphen used to reach the synthetic source routine unquoted (`exit(Assign-to No.)`), which
    /// parses as `Assign-` with no right operand. Lowering indexed `kids[1]` and panicked, which
    /// ended the whole run. The quotes now stay (the next test); an expression with a missing
    /// operand is still refused, not a panic.
    #[test]
    fn a_binary_expression_with_a_missing_operand_is_refused_not_a_panic() {
        let (_, l) = lower_files(
            &[(
                "f.al",
                "table 50100 T { fields { field(1; \"Assign-to No.\"; Code[20]) { } } }
page 50100 P { SourceTable = T; layout { area(Content) { field(X; Assign-to No.) { } } } }",
            )],
            true,
        );
        let reasons: Vec<&String> = l.unsupported.iter().flatten().flatten().collect();
        assert!(reasons.iter().any(|r| r.contains("missing operand")), "{reasons:?}");
    }

    #[test]
    fn a_page_field_with_a_lone_quoted_source_name_lowers() {
        let (_, l) = lower_files(
            &[(
                "f.al",
                "table 50100 T { fields { field(1; \"Assign-to No.\"; Code[20]) { } } }
page 50100 P { SourceTable = T; layout { area(Content) { field(\"Assign-to No.\"; \"Assign-to No.\") { } } } }",
            )],
            true,
        );
        let reasons: Vec<&String> = l.unsupported.iter().flatten().flatten().collect();
        assert!(reasons.is_empty(), "{reasons:?}");
    }

    /// ADR-023: the slot of a base variable is computed from the base object's syntax, so the
    /// extension lowers without the lowered base. It must equal the slot the base's own lowering
    /// gives the variable (a page lays out its variables in declaration order; a report puts its
    /// data items and the request-page flag first).
    #[test]
    fn the_slot_of_a_base_variable_computed_from_syntax_is_the_one_the_base_lowers_to() {
        let (ws, l) = lower_files(
            &[(
                "f.al",
                "table 50100 T { fields { field(1; A; Integer) { } } keys { key(PK; A) { } } }
page 50100 P { SourceTable = T; var First: Integer; protected var Second: Text; Lbl: Label 'x'; var Third: Record T; }
report 50101 R { dataset { dataitem(I1; T) { dataitem(I2; T) { } } dataitem(I3; T) { } } var RFirst: Integer; protected var RSecond: Text; }",
            )],
            false,
        );
        let find = |name: &str| ws.objects.iter().position(|o| o.name == name).unwrap();
        let (page, report) = (find("P"), find("R"));
        for (oi, items, names) in [
            (page, None, &["First", "Second", "Lbl", "Third"][..]),
            (report, Some(3), &["RFirst", "RSecond"][..]),
        ] {
            let ast = ws.object_ast(oi);
            for n in names {
                let want = l.program.objects[oi].globals.iter().position(|g| g.name == *n);
                assert_eq!(crate::base_access::slot_of(&ast, items, n), want, "{n}");
            }
        }
    }

    /// Base Application's chart pages pass their control add-in to a `var` parameter
    /// (`BusinessChartBuffer.UpdateChart(CurrPage.BusinessChart)`). Control add-ins are not modelled, so the
    /// routine is refused, and the reason names the control add-in instead of "is not assignable".
    #[test]
    fn a_control_add_in_passed_by_reference_is_refused_as_a_control_add_in() {
        let (_, l) = lower_files(
            &[(
                "f.al",
                "controladdin MyChart { }
codeunit 50100 Buf { procedure UpdateChart(var Chart: ControlAddIn \"MyChart\") begin end; }
page 50100 P { PageType = Card; layout { area(Content) { usercontrol(BusinessChart; MyChart) { } } }
  trigger OnOpenPage() var B: Codeunit Buf; begin B.UpdateChart(CurrPage.BusinessChart); end; }",
            )],
            true,
        );
        let reasons: Vec<&String> = l.unsupported.iter().flatten().flatten().collect();
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("control add-in") && !r.contains("not assignable")),
            "{reasons:?}"
        );
    }

    /// Tests-SCM: a page extension's field `field(X; Item."Qty. on Component Lines")` names a
    /// `protected var` of the page it extends. It lowers (ADR-023; corpus
    /// handlers/TestPageExtensionBaseAccessContracts.al:326), and a name the base does not make
    /// visible is refused with the compiler's text (AL0161).
    #[test]
    fn an_extension_naming_a_variable_of_its_base_page_lowers_and_a_private_one_is_refused() {
        let (_, l) = lower_files(
            &[(
                "f.al",
                "table 50100 T { fields { field(1; \"Qty. A\"; Integer) { } } }
page 50100 Base { SourceTable = T; layout { area(Content) { field(A; Rec.\"Qty. A\") { } } } protected var Item: Record T; var Hidden: Integer; }
pageextension 50101 Ext extends Base { layout { addlast(Content) { field(\"X\"; Item.\"Qty. A\") { } } } }
pageextension 50102 Ext2 extends Base { trigger OnOpenPage() begin Hidden := 1; end; }",
            )],
            true,
        );
        let reasons: Vec<&String> = l.unsupported.iter().flatten().flatten().collect();
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("'Hidden' is inaccessible due to its protection level"),
            "{reasons:?}"
        );
    }

    /// Tests-ERM: the Manufacturing report extensions call `Window.Open(..)` and
    /// `ExchangeAmtLCYToFCY(..)`, a `protected var` and a `protected procedure` of the report they
    /// extend (Microsoft's "Adjust Add. Reporting Currency"). They lower (corpus
    /// reportextensiontrigger/TestReportExtensionBaseMembersContracts.al:256); a `local` procedure
    /// is refused with the compiler's text.
    #[test]
    fn a_report_extension_naming_a_member_of_its_base_report_lowers_and_a_local_one_is_refused() {
        let (_, l) = lower_files(
            &[(
                "f.al",
                "table 50100 T { fields { field(1; Id; Integer) { } } }
report 50100 Base { dataset { dataitem(Item; T) { } } protected var Window: Dialog; protected procedure Conv(A: Decimal): Decimal begin exit(A); end; local procedure Priv() begin end; }
reportextension 50101 Ext extends Base
{
    dataset { modify(Item) { trigger OnPreDataItem() begin Window.Open('x'); end; } }
    trigger OnPreReport() begin Message(Format(Conv(1))); Priv(); end;
}",
            )],
            true,
        );
        let reasons: Vec<&String> = l.unsupported.iter().flatten().flatten().collect();
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("'Priv()' is inaccessible due to its protection level"),
            "{reasons:?}"
        );
    }

    const TWO_TESTS: &str = "codeunit 50100 \"Wanted\"
{
    Subtype = Test;
    [Test]
    procedure T1()
    var
        x: Integer;
    begin
        x := 1;
    end;
}
codeunit 50101 \"Left Out\"
{
    Subtype = Test;
    [Test]
    procedure T2()
    var
        x: Integer;
    begin
        x := 1;
    end;
}
codeunit 50102 \"Helper\"
{
    procedure H()
    var
        x: Integer;
    begin
        x := 1;
    end;
}
codeunit 50103 \"Listening Test\"
{
    Subtype = Test;
    [Test]
    procedure T3()
    var
        x: Integer;
    begin
        x := 1;
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Helper, 'OnX', '', false, false)]
    local procedure OnX()
    begin
    end;
}";

    /// A test selection (`Roots::only_tests`) roots the check at the selected test codeunits:
    /// the others are stubs that refuse, and helpers and event listeners stay compiled.
    #[test]
    fn a_selection_compiles_only_the_selected_test_codeunits() {
        let app = || AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".to_string(), TWO_TESTS.to_string())],
        };
        let reasons = |only: Option<Vec<(usize, u32)>>| {
            let (ws, l) = compile_apps_with(
                vec![app()],
                vec![crate::workspace::AppKind::Workspace],
                &Default::default(),
                None,
                crate::check::Roots {
                    baseline: false,
                    only_tests: only,
                    ..Default::default()
                },
            );
            let of = |id: u32| {
                let oi = ws.objects.iter().position(|o| o.id == Some(id)).expect("object");
                l.unsupported[oi].iter().flatten().count()
            };
            (of(50100), of(50101), of(50102), of(50103))
        };
        assert_eq!(
            reasons(None),
            (0, 0, 0, 0),
            "without a selection every codeunit compiles"
        );
        assert_eq!(
            reasons(Some(vec![(0, 50100)])),
            (0, 1, 0, 0),
            "only the codeunit that is not selected, not a helper and not a listener, is left out"
        );
    }

    /// With `Roots::reach_only` a normal codeunit's procedure that no selected test (and no root that
    /// runs by itself: a trigger, a subscriber, a codeunit with a Subtype) names is a stub that
    /// refuses, so a pure test is not compiled together with the data-touching code beside it.
    #[test]
    fn reach_only_compiles_the_procedures_the_selected_tests_name() {
        const SRC: &str = "codeunit 50100 \"Helper\" {
            procedure Used(): Integer begin exit(Deeper()); end;
            local procedure Deeper(): Integer begin exit(1); end;
            procedure Unused() begin Message('x'); end;
            local procedure AlsoUnused() begin Message('y'); end;
            trigger OnRun() begin Message('z'); end;
            [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Other\", 'OnX', '', false, false)]
            local procedure Listen() begin Message('w'); end;
        }
        codeunit 50101 \"Other\" { [IntegrationEvent(false, false)] procedure OnX() begin end; }
        codeunit 50102 \"T\" { Subtype = Test; var H: Codeunit \"Helper\";
            [Test] procedure P() begin if H.Used() <> 1 then Error('x'); end; }
        codeunit 50103 \"Other Test\" { Subtype = Test; var H: Codeunit \"Helper\";
            [Test] procedure Q() begin H.Unused(); end; }";
        let compile = |reach_only: bool| {
            let (ws, l) = compile_apps_with(
                vec![AppSource {
                    id: "a".into(),
                    name: "a".into(),
                    dependencies: vec![],
                    files: vec![("f.al".to_string(), SRC.to_string())],
                }],
                vec![crate::workspace::AppKind::Workspace],
                &Default::default(),
                None,
                crate::check::Roots {
                    baseline: false,
                    only_tests: Some(vec![(0, 50102)]),
                    reach_only,
                    ..Default::default()
                },
            );
            let oi = ws.objects.iter().position(|o| o.id == Some(50100)).expect("helper");
            let refused: Vec<String> = l.program.objects[oi]
                .routines
                .iter()
                .zip(&l.unsupported[oi])
                .filter(|(_, why)| !why.is_empty())
                .map(|(r, _)| r.name.clone())
                .collect();
            refused
        };
        assert_eq!(
            compile(false),
            Vec::<String>::new(),
            "without reach_only the whole codeunit compiles"
        );
        assert_eq!(
            compile(true),
            vec!["Unused".to_string(), "AlsoUnused".to_string()],
            "Used and Deeper are named from the test, the trigger and the subscriber run by themselves"
        );
    }

    /// (line, column) of the position a `Stmt::Pos` marker names.
    fn at(l: &Lowered, s: &ir::Stmt) -> (u32, u32) {
        let ir::Stmt::Pos(id) = s else {
            panic!("not a position marker: {s:?}")
        };
        let p = l.program.pos(*id).expect("a position in the table");
        (p.line, p.col)
    }

    fn routine<'a>(l: &'a Lowered, name: &str) -> (usize, &'a ir::Routine) {
        l.program.objects[0]
            .routines
            .iter()
            .enumerate()
            .find(|(_, r)| r.name == name)
            .expect("routine")
    }

    const POS_SRC: &str = "codeunit 50100 C
{
    [Scope('OnPrem')]
    procedure P()
    var
        x: Integer;
    begin
        // a comment before the statement
        x := 1;
        if x = 1 then
            x := 2;
        x :=
            3;
    end;

    trigger OnRun()
    begin
        P();
    end;
}
";

    #[test]
    fn statements_carry_their_line_and_column() {
        let (_, l) = lower_pos(POS_SRC);
        let (_, p) = routine(&l, "P");
        let b = &p.body;
        assert_eq!(b.len(), 6, "{b:?}");
        assert_eq!(at(&l, &b[0]), (9, 9));
        assert!(matches!(b[1], ir::Stmt::Assign(..)));
        assert_eq!(at(&l, &b[2]), (10, 9));
        let ir::Stmt::If(_, then_s, _) = &b[3] else {
            panic!("{:?}", b[3])
        };
        assert_eq!(at(&l, &then_s[0]), (11, 13));
        // A statement that wraps starts on its first line and ends on its last.
        assert_eq!(at(&l, &b[4]), (12, 9));
        let ir::Stmt::Pos(id) = b[4] else { panic!() };
        assert_eq!(l.program.pos(id).unwrap().end_line, 13);
    }

    #[test]
    fn a_routine_position_is_its_keyword_not_its_attribute() {
        let (_, l) = lower_pos(POS_SRC);
        let (oi, p) = routine(&l, "P");
        let pos = l.program.pos(p.pos).expect("routine position");
        assert_eq!(
            (pos.line, pos.col),
            (4, 5),
            "the `procedure` line, not the [Scope] line"
        );
        let (_, t) = routine(&l, "OnRun");
        assert_eq!(l.program.pos(t.pos).map(|p| p.line), Some(16));
        // A statement's position names its object and routine.
        let ir::Stmt::Pos(id) = p.body[0] else { panic!() };
        let sp = l.program.pos(id).unwrap();
        assert_eq!((sp.object as usize, sp.routine), (oi, 0));
        assert_eq!(l.program.sources[sp.file as usize].path, "f.al");
        assert_eq!(l.program.sources[sp.file as usize].app, 0);
    }

    #[test]
    fn without_positions_the_bodies_have_no_markers() {
        let (_, l) = lower_src(POS_SRC);
        let (_, p) = routine(&l, "P");
        assert!(p.body.iter().all(|s| !matches!(s, ir::Stmt::Pos(_))));
        assert!(l.program.positions.is_empty());
        assert_eq!(p.pos, ir::NO_POS);
    }

    // The page expansion appends routines at the end of the page, which moves the lines of
    // every object that follows it in the file. Positions use the author's lines.
    #[test]
    fn expanding_a_page_does_not_move_the_lines_of_the_next_object() {
        let src = "page 50100 P
{
    layout { area(Content) { field(A; Rec.X) { Visible = not Hide; } } }
    var Hide: Boolean;
}

codeunit 50101 C2
{
    procedure Q()
    begin
        Hide();
    end;
    local procedure Hide() begin end;
}
";
        let (_, l) = lower_pos(src);
        let c2 = l.program.objects.iter().find(|o| o.name == "C2").unwrap();
        let q = c2.routines.iter().find(|r| r.name == "Q").unwrap();
        assert_eq!(l.program.pos(q.pos).map(|p| p.line), Some(9));
        let ir::Stmt::Pos(id) = q.body[0] else {
            panic!("{:?}", q.body)
        };
        assert_eq!(l.program.pos(id).map(|p| p.line), Some(11));
        // The made-up routine has no source position.
        let page = l.program.objects.iter().find(|o| o.name == "P").unwrap();
        let synthetic = page
            .routines
            .iter()
            .find(|r| crate::pages::is_synthetic(&r.name))
            .unwrap();
        assert_eq!(synthetic.pos, ir::NO_POS);
        assert!(synthetic.body.iter().all(|s| !matches!(s, ir::Stmt::Pos(_))));
    }

    const DOTNET: &str = "dotnet { assembly(\"netstandard\") { Culture = 'neutral'; PublicKeyToken = 'cc7b13ffcd2ddd51'; type(\"System.Convert\"; \"Convert\") { } type(\"System.IO.MemoryStream\"; \"MemoryStream\") { } } }\n";

    #[test]
    fn dotnet_calls_lower_to_dynamic_member_calls_on_the_declared_type() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P(): Integer var Convert: DotNet Convert; I: Integer; begin I := Convert.ToInt32('12'); exit(I); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        assert_eq!(l.program.dotnet_types.len(), 2);
        let d = &l.program.dotnet_types[0];
        assert_eq!(
            (d.alias.as_str(), d.type_name.as_str(), d.assembly.as_str()),
            ("Convert", "System.Convert", "netstandard")
        );
        assert_eq!(d.public_key_token.as_deref(), Some("cc7b13ffcd2ddd51"));
        let (oi, r) = routine(&l, "P");
        assert_eq!(l.unsupported[oi][0], Vec::<String>::new());
        assert_eq!(r.locals[0].ty, RtType::DotNet(0));
        assert_eq!(
            r.body[0],
            ir::Stmt::Assign(
                ir::Place::Local(1),
                ir::Expr::DotNetCoerce(
                    RtType::Integer,
                    Box::new(ir::Expr::DotNet {
                        ty: Some(0),
                        receiver: Some(Box::new(ir::Expr::Load(ir::Place::Local(0)))),
                        op: ir::DotNetOp::Call("ToInt32".into()),
                        args: vec![ir::Arg::Value(ir::Expr::Const(ir::Const::Text("12".into())))],
                    })
                )
            )
        );
    }

    #[test]
    fn dotnet_constructors_properties_and_by_reference_arguments_lower() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P() var M: DotNet MemoryStream; N: Integer; begin M := M.MemoryStream(16); M.Position := 3; N := M.Length; M.Read(N); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let (oi, r) = routine(&l, "P");
        assert_eq!(l.unsupported[oi][0], Vec::<String>::new());
        let ir::Stmt::Assign(_, ir::Expr::Convert(RtType::DotNet(1), ctor)) = &r.body[0] else {
            panic!("{:?}", r.body[0])
        };
        assert!(matches!(
            **ctor,
            ir::Expr::DotNet {
                ty: Some(1),
                op: ir::DotNetOp::Construct,
                ..
            }
        ));
        assert!(matches!(
            &r.body[1],
            ir::Stmt::Expr(ir::Expr::DotNet { op: ir::DotNetOp::Set(n), args, .. }) if n == "Position" && args.len() == 1
        ));
        assert!(matches!(
            &r.body[2],
            ir::Stmt::Assign(_, ir::Expr::DotNetCoerce(RtType::Integer, e))
                if matches!(**e, ir::Expr::DotNet { op: ir::DotNetOp::Get(ref n), .. } if n == "Length")
        ));
        // A plain variable argument is passed by place, so a by-reference parameter can write it back.
        assert!(matches!(
            &r.body[3],
            ir::Stmt::Expr(ir::Expr::DotNet { op: ir::DotNetOp::Call(n), args, .. })
                if n == "Read" && matches!(args[0], ir::Arg::Ref(ir::Place::Local(1)))
        ));
    }

    #[test]
    fn a_dotnet_alias_without_a_declaration_is_a_lowering_reason() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var X: DotNet Missing; begin end; }");
        let (oi, _) = routine(&l, "P");
        assert!(
            l.unsupported[oi][0]
                .iter()
                .any(|r| r.contains("DotNet 'Missing'") && r.contains("no dotnet")),
            "{:?}",
            l.unsupported[oi][0]
        );
    }

    // session/TestEnvironmentUserTenantContracts.al:204-216 reaches `CopyStr(TenantInfo.PreferredLanguage(), 1, 2)`
    // and `IsNull(TenantInfo)` in Microsoft's Azure AD Tenant Impl.: a DotNet result passed to a Text
    // parameter must fit it exactly (ADR-014 section 4), and IsNull takes a DotNet value.
    #[test]
    fn a_dotnet_result_passed_to_a_text_parameter_is_coerced_and_isnull_takes_a_dotnet_value() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P(): Text var Convert: DotNet Convert; begin if IsNull(Convert) then exit(''); exit(CopyStr(Convert.ToBase64String('x'), 1, 2)); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let (oi, r) = routine(&l, "P");
        assert_eq!(l.unsupported[oi][0], Vec::<String>::new());
        let dump = format!("{:?}", r.body);
        assert!(dump.contains("DotNetCoerce(Text(None)"), "{dump}");
    }

    // Tests-ERM Document Service Management.CheckError: `Error(Service.LastError.Message)`. A DotNet result
    // fits the Text overload of Error, not the ErrorInfo one (ADR-024).
    #[test]
    fn a_dotnet_result_passed_to_error_picks_the_text_overload() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P() var Convert: DotNet Convert; begin Error(Convert.ToBase64String('x')); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let (oi, r) = routine(&l, "P");
        assert_eq!(l.unsupported[oi][0], Vec::<String>::new());
        assert!(format!("{:?}", r.body).contains("DotNetCoerce(Text"), "{:?}", r.body);
    }

    // NPR install code and Base Application's Job Queue Entry compare with `ReportLayoutType::Rdlc` and `::Word`.
    // The members are the documentation's (a documentation-only assumption, docs/probes-needed.md).
    #[test]
    fn report_layout_type_members_lower() {
        let (ws, l) = lower_src(
            "codeunit 50100 C { procedure P(): Boolean begin exit(ReportLayoutType::Rdlc <> ReportLayoutType::Word); end; }",
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let (oi, _) = routine(&l, "P");
        assert_eq!(l.unsupported[oi][0], Vec::<String>::new());
    }

    // A `[Native]` procedure of a platform app has an empty AL body: the service tier implements it. Running
    // the empty body would answer the default value (the corpus run showed `Agent Utilities.IsAgentSession`
    // deciding every session is an agent session, because the native `GetCurrentSessionAgentMetadataProviderType`
    // "returned" 0 instead of -1). Such a routine is refused when it runs.
    #[test]
    fn a_native_procedure_with_an_empty_body_is_refused() {
        let (_, l) = lower_src(
            "codeunit 50100 C { [Native] procedure Kind(): Integer begin end; procedure Other(): Integer begin exit(1); end; }",
        );
        let (ki, _) = routine(&l, "Kind");
        assert!(
            l.unsupported[0][ki]
                .iter()
                .any(|r| r.contains("Native") && r.contains("Kind")),
            "{:?}",
            l.unsupported[0][ki]
        );
        let (oi, _) = routine(&l, "Other");
        assert!(l.unsupported[0][oi].is_empty());
    }

    // ADR-024: a DotNet result used as an operand is settled when the code runs. The comparison with
    // a Text literal is a CLR string comparison, so it has its own operator.
    #[test]
    fn a_dotnet_result_used_as_an_operand_is_settled_at_run_time() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P(): Boolean var Convert: DotNet Convert; begin exit(Convert.ToInt32('1') > 0); end; procedure Q(): Boolean var Convert: DotNet Convert; begin exit(Convert.ToBase64String('x') = 'WA=='); end; procedure R(): Integer var Convert: DotNet Convert; I: Integer; begin for I := 1 to Convert.ToInt32('2') - 1 do exit(-Convert.ToInt32('3')); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        for name in ["P", "Q", "R"] {
            let (ri, _) = routine(&l, name);
            assert!(l.unsupported[0][ri].is_empty(), "{name}: {:?}", l.unsupported[0][ri]);
        }
        let (_, p) = routine(&l, "P");
        let dump = format!("{:?}", p.body);
        assert!(dump.contains("DotNetOperand(") && dump.contains("NoText)"), "{dump}");
        let (_, q) = routine(&l, "Q");
        assert!(format!("{:?}", q.body).contains("DotNetEq"), "{:?}", q.body);
        // The bound of a for loop is settled too.
        let (_, r) = routine(&l, "R");
        assert!(format!("{:?}", r.body).contains("DotNetOperand("), "{:?}", r.body);
    }

    // ADR-024: Format takes an object as it is (its ToString()); a scalar parameter takes a settled
    // primitive; a `var` parameter and an AL procedure's DotNet-typed object stay refused.
    #[test]
    fn a_dotnet_value_passed_to_format_is_a_variant_operand() {
        let (ws, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P(): Text var Convert: DotNet Convert; begin exit(Format(Convert.ToBase64String('x')) + Format(Convert)); end; procedure Q(): Integer var Convert: DotNet Convert; begin exit(StrLen(Convert.ToBase64String('x'))); end; }}"
        ));
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        for name in ["P", "Q"] {
            let (ri, _) = routine(&l, name);
            assert!(l.unsupported[0][ri].is_empty(), "{name}: {:?}", l.unsupported[0][ri]);
        }
        let (_, p) = routine(&l, "P");
        assert!(format!("{:?}", p.body).contains("Variant)"), "{:?}", p.body);
        let (_, q) = routine(&l, "Q");
        assert!(format!("{:?}", q.body).contains("Strict)"), "{:?}", q.body);
    }

    const DOTNET_REGISTRY: &str = "dotnet { assembly(\"Microsoft.Dynamics.Nav.NavUserAccount\") { Culture = 'neutral'; PublicKeyToken = '31bf3856ad364e35'; type(\"Microsoft.Dynamics.Nav.NavUserAccount.NavUserAccountHelper\"; \"NavUserAccountHelper\") { } type(\"Microsoft.Dynamics.Nav.NavUserAccount.NavTenantSettingsHelper\"; \"NavTenantSettingsHelper\") { } } assembly(\"Microsoft.Dynamics.Nav.Types\") { Culture = 'neutral'; PublicKeyToken = '31bf3856ad364e35'; type(\"Microsoft.Dynamics.Nav.Types.ALConfigSettings\"; \"ALConfigSettings\") { } } }\n";

    /// A member the registry provides declares its AL return type (ADR-014), so its result is an
    /// operand like any typed value. Base App: `exit(IsSaaS() and UserAccountHelper.IsAzure())`.
    #[test]
    fn a_registry_member_with_a_declared_type_is_usable_as_an_operand() {
        let (_, l) = lower_src(&format!(
            "{DOTNET_REGISTRY}codeunit 50100 C {{ procedure P(B: Boolean): Boolean var U: DotNet NavUserAccountHelper; begin if U.IsAzure() then exit(true); exit(B and U.IsAzure()); end; procedure Q(): Boolean var T: DotNet NavTenantSettingsHelper; begin exit(T.GetEnvironmentName() <> ''); end; procedure R(): Boolean var S: DotNet ALConfigSettings; begin S := S.Instance(); exit(S.ApiSubscriptionDelayTime() > 0); end; }}"
        ));
        for name in ["P", "Q", "R"] {
            let (ri, _) = routine(&l, name);
            assert!(l.unsupported[0][ri].is_empty(), "{name}: {:?}", l.unsupported[0][ri]);
        }
    }

    /// The same member names on a type the registry does not know are untyped, so they are settled
    /// when the code runs (ADR-024) instead of refused when it is lowered.
    #[test]
    fn a_member_of_an_unregistered_type_is_untyped_and_settled_at_run_time() {
        let (_, l) = lower_src(&format!(
            "{DOTNET}codeunit 50100 C {{ procedure P(): Boolean var Convert: DotNet Convert; begin exit(Convert.IsAzure() and true); end; }}"
        ));
        let (oi, r) = routine(&l, "P");
        assert!(l.unsupported[oi][0].is_empty(), "{:?}", l.unsupported[oi][0]);
        assert!(format!("{:?}", r.body).contains("DotNetOperand("), "{:?}", r.body);
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
    fn format_of_an_option_field_with_captions_formats_the_caption() {
        // corpus fieldref/TestPrecompiledFieldOptionCaption.al:47-55
        let (_, l) = lower_src(
            "table 1 T { fields { field(1; K; Code[10]) { } field(2; St; Option) { OptionMembers = Draft,\"Validaton in Process\"; OptionCaption = 'Draft,Validation in Process'; } field(3; Plain; Option) { OptionMembers = A,B; } } }\n\
             codeunit 2 C { procedure P(): Text var R: Record T; begin exit(Format(R.St)); end; procedure Q(): Text var R: Record T; begin exit(Format(R.Plain)); end; procedure W(): Text var R: Record T; begin exit(Format(R.St, 0, 9)); end; }",
        );
        let arg0 = |name: &str| -> Option<ir::Expr> {
            let c = l.program.objects.iter().position(|o| o.name == "C")?;
            let r = l.program.objects[c].routines.iter().find(|r| r.name == name)?;
            let ir::Stmt::Exit(Some(ir::Expr::Builtin { args, .. })) = &r.body[0] else {
                return None;
            };
            let ir::Arg::Value(v) = &args[0] else { return None };
            Some(v.clone())
        };
        let want = RtType::Option(vec!["Draft".into(), "Validation in Process".into()]);
        assert!(
            matches!(arg0("P"), Some(ir::Expr::Convert(t, _)) if t == want),
            "{:?}",
            arg0("P")
        );
        // No caption list: untouched.
        assert!(
            !matches!(arg0("Q"), Some(ir::Expr::Convert(RtType::Option(_), _))),
            "{:?}",
            arg0("Q")
        );
        // Every Format form reads the caption list (text/TestProbeTextDateFormatContracts.al:
        // Format_OptionFieldAndOptionVariable), standard format 9 included.
        let c = l.program.objects.iter().position(|o| o.name == "C").unwrap();
        let w = l.program.objects[c]
            .routines
            .iter()
            .position(|r| r.name == "W")
            .unwrap();
        assert!(l.unsupported[c][w].is_empty(), "{:?}", l.unsupported[c][w]);
    }

    #[test]
    fn a_builtin_newer_than_the_target_runtime_makes_the_routine_unsupported() {
        let src =
            "codeunit 50100 C { procedure P() begin Codeunit.Run('Some Codeunit'); end; procedure Ok() begin end; }";
        let apps = || {
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }]
        };
        let target = |bc: &str| al_version::Target::for_bc(al_version::BcVersion::parse(bc).unwrap());
        let (_, l) = compile_for(apps(), &Default::default(), &target("27.5"));
        let (pi, _) = routine(&l, "P");
        let (oi, _) = routine(&l, "Ok");
        assert_eq!(
            l.unsupported[0][pi],
            vec!["AR1005 `Run` is not available in runtime 16.0 (introduced in 17.0)".to_string()],
            "refused at runtime 16.0"
        );
        assert!(l.unsupported[0][oi].is_empty());
        // At runtime 17.0 the by-name overload exists; the call lowers (the VM refuses it later).
        let (_, l) = compile_for(apps(), &Default::default(), &target("28"));
        assert!(
            !l.unsupported[0][pi].iter().any(|r| r.contains("AR1005")),
            "{:?}",
            l.unsupported[0][pi]
        );
    }

    #[test]
    fn unimplemented_builtin_makes_the_routine_unsupported() {
        let (_, l) = lower_src(
            "codeunit 50100 C { procedure P() begin MinimumActiveRowVersion(); end; procedure Ok() begin end; }",
        );
        let (pi, _) = routine(&l, "P");
        let (oi, _) = routine(&l, "Ok");
        assert_eq!(
            l.unsupported[0][pi],
            vec!["builtin Database.MinimumActiveRowVersion#1 is not implemented".to_string()]
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

    /// An enum field's InitValue by name when the enum is in another namespace, named by a
    /// qualified path: Base App's `Item Journal Line.Type` is
    /// `Enum Microsoft.Manufacturing.Capacity."Capacity Type Journal"` with `InitValue = " "`
    /// (corpus record/TestRenamePropagationBaseApp.al, Codeunit 60975).
    #[test]
    fn enum_init_value_with_namespaced_enum() {
        let (_, l) = lower_files(
            &[
                (
                    "e.al",
                    "namespace A.B; enum 50920 \"ALT Cap\" { value(0; \" \") { } value(1; \"Machine Center\") { } value(2; Work) { } }",
                ),
                (
                    "t.al",
                    "namespace C.D; table 50921 \"ALT Tab\" { fields { field(1; K; Code[10]) { } \
                     field(2; T; Enum A.B.\"ALT Cap\") { InitValue = \" \"; } \
                     field(3; U; Enum A.B.\"ALT Cap\") { InitValue = Work; } } keys { key(PK; K) { } } }",
                ),
            ],
            false,
        );
        let t = l
            .program
            .objects
            .iter()
            .find_map(|o| o.table.as_ref().filter(|t| t.fields.iter().any(|f| f.name == "T")))
            .expect("table schema");
        let f = |n: &str| t.fields.iter().find(|f| f.name == n).unwrap();
        assert!(matches!(f("T").init, Some(ir::Const::Enum(_, 0))), "{:?}", f("T").init);
        assert!(matches!(f("U").init, Some(ir::Const::Enum(_, 2))), "{:?}", f("U").init);
    }

    /// A dependency app's TableRelation is fixed when that app compiled, against the apps it saw:
    /// a same-named table of a dependent app does not take it over (corpus
    /// fieldref/TestRelationTargetNameCollision.al: Customer."Shipping Agent Code" relates to the
    /// Base Application table 291, not to the dependent app's table 60990).
    #[test]
    fn a_dependency_apps_relation_resolves_among_the_apps_it_saw() {
        use crate::workspace::AppKind;
        let src = |id: &str, deps: &[&str], files: &[(&str, &str)]| AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files: files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
        };
        let dep = src(
            "dep",
            &[],
            &[
                (
                    "cust.al",
                    "namespace D.Sales; using D.Ship; table 18 Cust { fields { field(1; K; Code[10]) { } \
                     field(2; Agent; Code[10]) { TableRelation = \"Ship Agent\"; } } keys { key(PK; K) { } } }",
                ),
                (
                    "ship.al",
                    "namespace D.Ship; table 291 \"Ship Agent\" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }",
                ),
            ],
        );
        let main = src(
            "main",
            &["dep"],
            &[
                (
                    "m.al",
                    "namespace M; using D.Sales; codeunit 1 C { var X: Record Cust; }",
                ),
                (
                    "local.al",
                    "namespace M; table 60990 \"Ship Agent\" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }",
                ),
            ],
        );
        let ws = Workspace::load_apps(
            vec![main, dep],
            vec![AppKind::Workspace, AppKind::Dependency],
            &Default::default(),
        );
        let members = crate::members::build(&ws);
        let checked = crate::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
        let l = lower_with_positions(&ws, &members, &checked, false);
        let cust = l
            .program
            .objects
            .iter()
            .find(|o| o.name == "Cust")
            .expect("Cust is compiled");
        let agent = cust
            .table
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|f| f.name == "Agent")
            .unwrap();
        let target = agent.relation_arms[0].table.map(|t| l.program.objects[t].id);
        assert_eq!(target, Some(Some(291)), "{:?}", agent.relation_arms);
    }

    // Base Application's `AdjustAddReportingCurrency.Report.al` declares
    // `dataitem(Integer; System.Utilities.Integer)`: a data item whose table is written with its namespace.
    #[test]
    fn a_dataitem_table_written_with_its_namespace_resolves() {
        use crate::workspace::AppKind;
        let src = |id: &str, deps: &[&str], files: &[(&str, &str)]| AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files: files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
        };
        let dep = src(
            "dep",
            &[],
            &[(
                "ship.al",
                "namespace D.Ship; table 291 \"Ship Agent\" { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }",
            )],
        );
        let main = src(
            "main",
            &["dep"],
            &[(
                "r.al",
                "report 50000 R { ProcessingOnly = true; UseRequestPage = false; \
                 dataset { dataitem(Agent; D.Ship.\"Ship Agent\") { } } \
                 trigger OnPreReport() begin end; } \
                 codeunit 50001 C { var Rep: Report R; procedure P() begin end; }",
            )],
        );
        let ws = Workspace::load_apps(
            vec![main, dep],
            vec![AppKind::Workspace, AppKind::Dependency],
            &Default::default(),
        );
        let members = crate::members::build(&ws);
        let checked = crate::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
        let l = lower_with_positions(&ws, &members, &checked, false);
        let why: Vec<_> = l
            .unsupported
            .iter()
            .enumerate()
            .flat_map(|(o, rs)| {
                let name = &l.program.objects[o].name;
                rs.iter().flatten().map(move |r| format!("{name}: {r}"))
            })
            .collect();
        assert!(why.is_empty(), "{why:?}");
    }

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
    fn table_relation_where_object_constants_resolve_to_the_object_id() {
        // corpus record/CdcOwner.Table.al:67,79 (`Database::"CDC Owner"`, `Report::"ALT Simple Report"`).
        let (_, l) = lower_src(
            "table 50911 \"ALT Line\" { fields { field(1; Code; Code[20]) { } field(7; \"Table ID\"; Integer) { } field(8; \"Report ID\"; Integer) { } } }
             report 50950 \"ALT Rep\" { }
             table 50912 \"ALT Head\" { fields {
                field(1; \"No.\"; Code[20]) { }
                field(2; D; Code[20]) { TableRelation = \"ALT Line\".Code where(\"Table ID\" = const(Database::\"ALT Line\")); }
                field(3; R; Code[20]) { TableRelation = \"ALT Line\".Code where(\"Report ID\" = const(Report::\"ALT Rep\")); }
             } }",
        );
        let text = |name: &str| {
            let f = flow(&l, "ALT Head", name);
            match &f.relation_arms[0].wheres.as_ref().expect("wheres understood")[0] {
                ir::RelationWhere::Value { text, .. } => text.clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(text("D"), "50911");
        assert_eq!(text("R"), "50950");
    }

    #[test]
    fn table_relation_condition_on_the_blank_enum_member_resolves() {
        // Base Application "Standard Service Line"."No.": `if (Type = const(" ")) "Standard Text" else
        // if (Type = const(Item)) Item ...`. The blank member is the enum value named `" "` (the corpus
        // declares such members: _fixtures/enums/ALTStatus.al:5); trimming the name lost it.
        let (_, l) = lower_src(
            "enum 50913 \"ALT Line Kind\" { value(0; \" \") { } value(1; Item) { } }
             table 50914 \"ALT Text\" { fields { field(1; Code; Code[20]) { } } }
             table 50915 \"ALT Item\" { fields { field(1; \"No.\"; Code[20]) { } } }
             table 50916 \"ALT Std Line\" { fields {
                field(1; Kind; Enum \"ALT Line Kind\") { }
                field(2; \"No.\"; Code[20]) { TableRelation = if (Kind = const(\" \")) \"ALT Text\" else if (Kind = const(Item)) \"ALT Item\"; }
             } }",
        );
        let f = flow(&l, "ALT Std Line", "No.");
        let conds: Vec<_> = f.relation_arms.iter().map(|a| a.conditions[0].value.clone()).collect();
        assert_eq!(conds.len(), 2, "{:?}", f.relation_arms);
        assert!(matches!(conds[0], ir::Const::Enum(_, 0)), "{conds:?}");
        assert!(matches!(conds[1], ir::Const::Enum(_, 1)), "{conds:?}");
    }

    #[test]
    fn table_relation_where_filter_rewrites_double_quoted_names() {
        // Base Application's `TableRelation = Item where(Type = filter(Inventory | "Non-Inventory"))`
        // (Prod. Order Component): the filter grammar quotes with `'`, so the AL quoting is rewritten
        // the way a CalcFormula filter's is (corpus record/TestCalcFormulaQuotedFilterTests.Codeunit.al:9).
        let (_, l) = lower_src(
            "enum 50920 \"ALT Kind\" { value(0; Inventory) { } value(1; \"Non-Inventory\") { } }
             table 50921 \"ALT Item\" { fields { field(1; \"No.\"; Code[20]) { } field(2; Type; Enum \"ALT Kind\") { } } }
             table 50922 \"ALT Comp\" { fields {
                field(1; \"No.\"; Code[20]) { }
                field(2; Item; Code[20]) { TableRelation = \"ALT Item\" where(Type = filter(Inventory | \"Non-Inventory\")); }
             } }",
        );
        let f = flow(&l, "ALT Comp", "Item");
        match &f.relation_arms[0].wheres.as_ref().expect("wheres understood")[0] {
            ir::RelationWhere::Value { text, .. } => assert_eq!(text, "Inventory | 'Non-Inventory'"),
            other => panic!("{other:?}"),
        }
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
    fn a_record_of_a_table_that_is_not_normal_or_temporary_is_refused() {
        // A table served by a table connection the runtime does not model (ExternalSQL here) is refused.
        // A CRM table is lowered: its first use fails at run time while no connection is registered
        // (session/TestCrmTableNoConnectionContracts.al), and with one it is refused there.
        let (_, l) = lower_src(
            "table 50960 Crm { TableType = ExternalSQL; fields { field(1; Id; Guid) { } } }
             table 50961 Plain { TableType = Normal; fields { field(1; Id; Integer) { } } }
             codeunit 50962 C {
                procedure A() var R: Record Crm; begin R.Init(); end;
                procedure B() var R: Record Plain; begin R.Init(); end;
             }",
        );
        let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
        let reasons = |n: &str| {
            let ri = l.program.objects[oi].routines.iter().position(|r| r.name == n).unwrap();
            l.unsupported[oi][ri].clone()
        };
        assert!(
            reasons("A").iter().any(|r| r.contains("TableType = ExternalSQL")),
            "{:?}",
            reasons("A")
        );
        assert!(reasons("B").is_empty(), "{:?}", reasons("B"));
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

    // The runtime stamps SystemId, SystemRowVersion and the created/modified pairs on writes
    // (al-vm hostvalues.rs), so a formula may read any of them
    // (record/TestCalcFormulaSystemFieldsTests.Codeunit.al:120-169).
    #[test]
    fn formulas_over_system_fields_are_lowered() {
        let src = "table 50920 L { fields { field(1; K; Integer) { } field(2; Ref; Guid) { } } }
table 50921 H { fields { field(1; K; Integer) { }
    field(2; A; DateTime) { FieldClass = FlowField; CalcFormula = max(L.SystemCreatedAt where(K = field(K))); }
    field(3; B; Guid) { FieldClass = FlowField; CalcFormula = lookup(L.SystemModifiedBy where(K = field(K))); }
    field(4; C; Integer) { FieldClass = FlowField; CalcFormula = count(L where(SystemCreatedBy = field(SystemId))); }
    field(5; D; Integer) { FieldClass = FlowField; CalcFormula = count(L where(Ref = field(SystemId))); }
    field(6; E; BigInteger) { FieldClass = FlowField; CalcFormula = max(L.SystemRowVersion where(K = field(K))); } } }";
        let (_, l) = lower_src(src);
        for name in ["A", "B", "C", "D", "E"] {
            let f = flow(&l, "H", name);
            assert!(f.calc.is_some(), "{name}: {:?}", f.calc_refusal);
        }
    }

    #[test]
    fn routines_reading_system_fields_are_lowered() {
        let src = format!(
            "codeunit 50100 C {{ procedure Stamped() var R: Record \"ALT T\"; D: DateTime; begin D := R.SystemModifiedAt; end;
             procedure Id() var R: Record \"ALT T\"; G: Guid; begin G := R.SystemId; end; }}\n{TABLE}"
        );
        let (_, l) = lower_src(&src);
        let (stamped, _) = routine(&l, "Stamped");
        let (id, _) = routine(&l, "Id");
        let oi = 0;
        assert!(
            l.unsupported[oi][stamped].is_empty(),
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
    fn audit_system_fields_are_lowered_now_that_the_runtime_stamps_them() {
        // record/TestBCSystemFieldContracts.al reads SystemCreatedAt/By and SystemModifiedAt/By;
        // the session stamps them from the host's clock and identity (al-vm hostvalues.rs).
        let src = format!(
            "{TABLE}\ncodeunit 50100 C {{
                procedure Read() var R: Record \"ALT T\"; D: DateTime; begin D := R.SystemModifiedAt; end;
                procedure Filter() var R: Record \"ALT T\"; begin R.SetRange(SystemCreatedBy); end;
                procedure Ok() var R: Record \"ALT T\"; G: Guid; begin G := R.SystemId; end;
            }}"
        );
        let (_, l) = lower_src(&src);
        let find = |n: &str| l.program.objects[1].routines.iter().position(|r| r.name == n).unwrap();
        for n in ["Read", "Filter", "Ok"] {
            assert!(
                l.unsupported[1][find(n)].is_empty(),
                "{n}: {:?}",
                l.unsupported[1][find(n)]
            );
        }
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

    const SAVE_VALUES_PAGE: &str = r#"
table 50010 "SV T" { fields { field(1; "No."; Code[20]) { } field(2; Qty; Decimal) { } } }
page 50011 "SV P" {
    SourceTable = "SV T"; SaveValues = true; PageType = StandardDialog;
    layout { area(Content) { field(Glob; GlobN) { } } }
    var GlobN: Text[30];
}
"#;

    // Handlers/TestPageSaveValues.al pins SaveValues for a page without extensions. An extension
    // that adds only controls bound to `Rec` fields adds no variable-bound control, so what the page
    // stores is the same set (docs/probes-needed.md, SaveValues with page extensions).
    #[test]
    fn save_values_with_an_extension_that_adds_only_record_fields_is_lowered() {
        let src = format!(
            "{SAVE_VALUES_PAGE} pageextension 50012 \"SV X\" extends \"SV P\" {{ layout {{ addlast(Content) {{ field(Q; Rec.Qty) {{ }} }} }} }}"
        );
        let (_, l) = lower_src(&src);
        let pg = l
            .program
            .objects
            .iter()
            .find(|o| o.name == "SV P")
            .unwrap()
            .page
            .clone()
            .unwrap();
        assert!(pg.unsupported.is_empty(), "{:?}", pg.unsupported);
    }

    // What BC stores for a control an extension binds to its own variable is not pinned: refused.
    #[test]
    fn save_values_with_an_extension_variable_control_is_refused() {
        let src = format!(
            "{SAVE_VALUES_PAGE} pageextension 50012 \"SV X\" extends \"SV P\" {{ layout {{ addlast(Content) {{ field(V; ExtVar) {{ }} }} }} var ExtVar: Integer; }}"
        );
        let (_, l) = lower_src(&src);
        let pg = l
            .program
            .objects
            .iter()
            .find(|o| o.name == "SV P")
            .unwrap()
            .page
            .clone()
            .unwrap();
        assert!(
            pg.unsupported
                .iter()
                .any(|w| w.contains("SaveValues") && w.contains("variable")),
            "{:?}",
            pg.unsupported
        );
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
