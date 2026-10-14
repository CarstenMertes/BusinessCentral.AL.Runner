//! Dependency objects whose lowered form is supplied instead of compiled (ADR-022).
//!
//! A compile cache holds the lowered IR of a set of dependency apps, built by a complete (eager)
//! compile of those apps alone. A run that has the cache compiles only the workspace's own apps
//! and the few dependency objects the workspace changes, and takes every other dependency object
//! from the cache. The dependency apps come first in the program, so an object index or a
//! base-field position means the same in the cache and in the run.
//!
//! Some lowered dependency code embeds facts about the whole program: the extensions of a table
//! or page, the implementers of an interface. A block lists the objects whose such facts it
//! embeds (`DepFacts::embeds`). When the workspace extends or implements one of them, the block
//! is not used: the object is lowered again from source, with the group of objects that extend
//! each other (`relower_set`).

use crate::workspace::{ObjKind, Workspace};
use al_ir as ir;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

/// What the lowering of other objects reads about a dependency object, besides its IR. Stored
/// beside the block, so a run does not parse the object to learn it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DepFacts {
    /// Identity, checked against the object the run loaded at the same index.
    pub kind: Option<ObjKind>,
    pub id: Option<u32>,
    pub name: String,
    pub app: u32,
    /// The objects whose extensions or implementers this object's lowered form embeds, itself
    /// included (see the module comment).
    pub embeds: Vec<u32>,
    /// The table whose record is the implicit `Rec` of the object.
    pub table_of: Option<u32>,
    pub page_temporary: bool,
    /// Why the test client cannot drive the page yet (empty: it can).
    pub page_reasons: Vec<String>,
    /// The page objects a page shows as parts, and those its actions run.
    pub page_parts: Vec<u32>,
    pub page_runs: Vec<u32>,
    /// The page extensions of a page or report, in the order they apply.
    pub page_extensions: Vec<u32>,
    /// The global slot of each variable of a page extension, by lower-case name.
    pub ext_slots: Vec<(String, u32)>,
    /// The source offset of every routine, in the order the object numbers them.
    pub routine_offsets: Vec<u32>,
    /// Why a variable of the xmlport or report is refused (the first reason), if it is.
    pub xml_reason: Option<String>,
    pub report_reason: Option<String>,
    /// The report extension that changes the run of a report, if exactly one does.
    pub report_extension: Option<u32>,
    /// The layout of a query, which the lowering of code that uses its columns reads.
    pub query: Option<crate::queries::QueryLayout>,
}

/// The lowered form of one dependency object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepBlock {
    pub object: ir::Object,
    /// Per routine: why it cannot run (empty: it can).
    pub unsupported: Vec<Vec<String>>,
    /// Per routine: the routines it calls directly, as (object, routine). What a variable of a
    /// page, report or xmlport can drive is added when asked (`Lowered::calls_of`).
    pub calls: Vec<Vec<(u32, u32)>>,
}

/// A set of lowered dependency objects, indexed like the objects of the dependency apps.
pub trait DepSource: Sync {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn facts(&self, object: usize) -> &DepFacts;
    /// The block of `object`, read and checked now. An error means the source is damaged.
    fn block(&self, object: usize) -> Result<DepBlock, String>;
    /// Dependency files that did not parse when the set was built, as (path, why). A run that
    /// uses the set parses none of them, so it has to be told.
    fn parse_problems(&self) -> &[(String, String)] {
        &[]
    }
}

/// A complete set, in memory: what a build of the dependency apps produces.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DepSet {
    pub facts: Vec<DepFacts>,
    pub blocks: Vec<DepBlock>,
    pub parse_problems: Vec<(String, String)>,
}

impl DepSource for DepSet {
    fn len(&self) -> usize {
        self.facts.len()
    }
    fn facts(&self, object: usize) -> &DepFacts {
        &self.facts[object]
    }
    fn block(&self, object: usize) -> Result<DepBlock, String> {
        Ok(self.blocks[object].clone())
    }
    fn parse_problems(&self) -> &[(String, String)] {
        &self.parse_problems
    }
}

/// How many objects, from the first, belong to dependency apps. The program has the dependency
/// apps first, so these are the objects a cache can supply. `None` when a dependency object
/// follows a workspace one.
pub fn dependency_prefix(ws: &Workspace) -> Option<usize> {
    let n = (0..ws.objects.len()).take_while(|&i| ws.is_dependency(i)).count();
    (n..ws.objects.len()).all(|i| !ws.is_dependency(i)).then_some(n)
}

/// Whether the dependency objects the run loaded are the ones the source describes.
pub fn check_identity(ws: &Workspace, n_dep: usize, src: &dyn DepSource) -> Result<(), String> {
    if src.len() != n_dep {
        return Err(format!(
            "the cache holds {} dependency objects, the dependency apps have {n_dep}",
            src.len()
        ));
    }
    for oi in 0..n_dep {
        let (o, f) = (&ws.objects[oi], src.facts(oi));
        if f.kind != Some(o.kind) || f.id != o.id || f.name != o.name || f.app as usize != o.app {
            return Err(format!(
                "dependency object {oi} is `{} {}` in the run and `{} {}` in the cache",
                o.kind.label(),
                o.name,
                f.kind.map_or("unknown", |k| k.label()),
                f.name
            ));
        }
    }
    Ok(())
}

/// The dependency objects the workspace's own objects extend or implement: what changes the
/// extension and implementer sets the blocks embed.
pub fn touched(ws: &Workspace, n_dep: usize) -> HashSet<usize> {
    let mut out = HashSet::new();
    for w in n_dep..ws.objects.len() {
        let o = &ws.objects[w];
        if let Some(t) = ws.extension_target(w) {
            out.insert(t);
        }
        let mut interfaces: Vec<usize> = Vec::new();
        for p in &o.implements {
            if let crate::workspace::Resolution::Found(i) = ws.resolve_ref(o.file, ObjKind::Interface, p) {
                interfaces.push(i);
            }
        }
        if o.kind == ObjKind::Interface {
            interfaces.push(w);
        }
        for i in interfaces {
            out.insert(i);
            out.extend(ws.interface_bases(i));
        }
    }
    out.retain(|&i| i < n_dep);
    out
}

/// The dependency objects that cannot come from the source because the workspace changes
/// something their lowered form embeds, and the objects that extend each other with them.
pub fn relower_set(ws: &Workspace, n_dep: usize, src: &dyn DepSource) -> Vec<bool> {
    let touched = touched(ws, n_dep);
    let mut relower = vec![false; n_dep];
    if touched.is_empty() {
        return relower;
    }
    for (oi, r) in relower.iter_mut().enumerate() {
        *r = src.facts(oi).embeds.iter().any(|e| touched.contains(&(*e as usize)));
    }
    // An object and the objects that extend it are lowered together: the extension's lowering
    // reads the base's layout, and the base's lists the extensions.
    let mut queue: Vec<usize> = (0..n_dep).filter(|&i| relower[i]).collect();
    while let Some(i) = queue.pop() {
        let mut group: Vec<usize> = ws.extensions_of(i).to_vec();
        group.extend(ws.extension_target(i));
        for g in group {
            if g < n_dep && !relower[g] {
                relower[g] = true;
                queue.push(g);
            }
        }
    }
    relower
}

/// `embeds` of an object: itself, what it recorded, and the pages a recorded page shows as
/// parts (their reasons are part of the page's).
pub fn embeds_of(oi: usize, recorded: &BTreeSet<usize>, page_parts: &dyn Fn(usize) -> Vec<usize>) -> Vec<u32> {
    let mut out: BTreeSet<usize> = recorded.clone();
    out.insert(oi);
    let mut queue: Vec<usize> = out.iter().copied().collect();
    while let Some(p) = queue.pop() {
        for q in page_parts(p) {
            if out.insert(q) {
                queue.push(q);
            }
        }
    }
    out.into_iter().map(|i| i as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Roots;
    use crate::lower::{Lowered, compile_apps_cached, compile_apps_with, lower_for_cache};
    use crate::workspace::{AppKind, AppSource};
    use std::collections::BTreeSet;

    fn app(id: &str, deps: &[&str], src: &str) -> AppSource {
        AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            files: vec![("f.al".into(), src.into())],
        }
    }

    const BASE: &str = r#"
table 50000 T { fields { field(1; A; Integer) { } field(2; B; Code[20]) { } } keys { key(PK; A) { } } }
tableextension 50001 DT extends T { fields { field(10; DX; Integer) { } } }
page 50002 P { PageType = Card; SourceTable = T; layout { area(Content) { field(AC; Rec.A) { } } } }
page 50003 P2 { PageType = Card; SourceTable = T; layout { area(Content) { part(Sub; P) { } } } }
interface IFoo { procedure Bump(): Integer; }
codeunit 50004 DImpl implements IFoo { procedure Bump(): Integer begin exit(1); end; }
codeunit 50005 DUse
{
    procedure CallIt(F: Interface IFoo): Integer begin exit(F.Bump()); end;
    procedure Fields(var R: Record T) begin R.DX := 3; R.A := 2; end;
    procedure Open() var TP: TestPage P2; begin end;
    procedure Is(F: Interface IFoo): Boolean begin exit(F is IFoo); end;
}
enum 50006 E implements IFoo { value(0; X) { Implementation = IFoo = DImpl; } }
codeunit 50007 DEnum { procedure G(En: Enum E): Integer var F: Interface IFoo; begin F := En; exit(F.Bump()); end; }
codeunit 50008 DPlain { procedure Plain(): Integer begin exit(7); end; procedure Twice(): Integer begin exit(Plain() * 2); end; }
"#;

    const MORE: &str = r#"
query 50009 DQ { elements { dataitem(T; T) { column(A; A) { } column(B; B) { } } } }
codeunit 50010 DQUse { procedure Sum(): Integer var Q: Query DQ; begin Q.Open(); if Q.Read() then exit(Q.A); end; }
page 50011 P3 { PageType = List; SourceTable = T; layout { area(Content) { part(Deep; P2) { } } } }
codeunit 50012 DPage { procedure Open() var TP: TestPage P3; begin TP.OpenView(); end; }
report 50013 R { dataset { dataitem(TD; T) { column(A; A) { } } } }
reportextension 50014 DRE extends R { dataset { add(TD) { column(B; B) { } } } }
codeunit 50015 DRun { procedure Go() begin Report.Run(50013); end; }
xmlport 50016 X { schema { textelement(Root) { tableelement(TE; T) { fieldelement(A; TE.A) { } } } } }
codeunit 50017 DX { procedure Imp() var XP: XmlPort X; begin end; }
interface IBar extends IFoo { procedure Name(): Text; }
codeunit 50018 DBar implements IBar { procedure Bump(): Integer begin exit(1); end; procedure Name(): Text begin exit('b'); end; }
codeunit 50019 DUseBar { procedure Use(F: Interface IFoo): Boolean begin exit(F is IFoo); end; }
"#;

    const OTHER: &str = r#"
table 51000 U { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 51001 OUse { procedure R(var X: Record U) begin X.K := 1; end; }
"#;

    const TABLE_EXT: &str = r#"
tableextension 60000 WT extends T { fields { field(20; WX; Integer) { } } }
codeunit 60004 WUse { procedure W() var R: Record T; begin R.WX := 1; R.DX := 2; end; }
"#;

    const EVERYTHING: &str = r#"
tableextension 60000 WT extends T { fields { field(20; WX; Integer) { } } }
pageextension 60001 WP extends P { layout { addlast(Content) { field(WXC; Rec.WX) { } } } }
codeunit 60002 WImpl implements IFoo { procedure Bump(): Integer begin exit(2); end; }
enumextension 60003 WE extends E { value(1; Y) { Implementation = IFoo = WImpl; } }
codeunit 60004 WUse { procedure W() var R: Record T; begin R.WX := 1; R.DX := 2; end; }
"#;

    fn calls(l: &Lowered) -> Vec<Vec<Vec<(usize, usize)>>> {
        (0..l.program.objects.len())
            .map(|o| {
                (0..l.program.objects[o].routines.len())
                    .map(|r| {
                        let mut v = l.calls_of(o, r);
                        v.sort_unstable();
                        v.dedup();
                        v
                    })
                    .collect()
            })
            .collect()
    }

    /// The complete compile of `deps` + `workspace` and the cached compile of them must agree on
    /// everything the runtime reads.
    pub(crate) fn assert_cached_equals_eager(deps: Vec<AppSource>, workspace: Vec<AppSource>) -> usize {
        let symbols = BTreeSet::new();
        let set = lower_for_cache(deps.clone(), &symbols, None);
        let mut apps = deps.clone();
        apps.extend(workspace);
        let kinds: Vec<AppKind> = (0..apps.len())
            .map(|i| {
                if i < deps.len() {
                    AppKind::Dependency
                } else {
                    AppKind::Workspace
                }
            })
            .collect();
        let roots = Roots {
            eager: true,
            ..Default::default()
        };
        let (_, eager) = compile_apps_with(apps.clone(), kinds.clone(), &symbols, None, roots.clone());
        let (ws, cached) = compile_apps_cached(apps, kinds, &symbols, None, roots, &set)
            .map_err(|e| e.1)
            .expect("the cache fits");
        let n_dep = dependency_prefix(&ws).unwrap();
        let relowered = (0..n_dep).filter(|&i| !cached.provided[i]).count();
        assert_eq!(eager.program.objects.len(), cached.program.objects.len());
        for (i, (a, b)) in eager.program.objects.iter().zip(&cached.program.objects).enumerate() {
            assert!(a == b, "object {i} `{}` differs", a.name);
        }
        assert!(*eager.program == *cached.program, "the programs differ");
        assert!(eager.unsupported == cached.unsupported, "the refusals differ");
        assert!(calls(&eager) == calls(&cached), "the call graphs differ");
        assert_eq!(eager.dependency, cached.dependency);
        relowered
    }

    fn deps() -> Vec<AppSource> {
        vec![
            app("d1", &[], BASE),
            app("d2", &["d1"], OTHER),
            app("d3", &["d1"], MORE),
        ]
    }

    const USES_DEPENDENCY_OBJECTS: &str = r#"
codeunit 60010 WQ { procedure R() var Q: Query DQ; begin Q.Open(); if Q.Read() then exit(Q.A); end; }
codeunit 60011 WP { procedure P() var TP: TestPage P3; begin TP.OpenView(); end; }
"#;

    const EXTENDS_A_PART_PAGE: &str = r#"
pageextension 60020 WPartExt extends P { layout { addlast(Content) { field(WAC; Rec.B) { } } } }
codeunit 60021 WUse { procedure P() var TP: TestPage P3; begin TP.OpenView(); end; }
"#;

    const EXTENDS_REPORT_AND_INTERFACE: &str = r#"
reportextension 60030 WRE extends R { dataset { add(TD) { column(WB; B) { } } } }
interface IWork extends IBar { procedure More(); }
codeunit 60031 WBar implements IWork { procedure Bump(): Integer begin exit(3); end; procedure Name(): Text begin exit('w'); end; procedure More() begin end; }
"#;

    #[test]
    fn a_workspace_that_changes_nothing_takes_every_dependency_object_from_the_cache() {
        let relowered = assert_cached_equals_eager(
            deps(),
            vec![app("w", &["d1"], "codeunit 60000 C { procedure P() begin end; }")],
        );
        assert_eq!(relowered, 0);
    }

    #[test]
    fn a_table_extension_in_the_workspace_relowers_the_table_and_what_embeds_it() {
        let relowered = assert_cached_equals_eager(deps(), vec![app("w", &["d1"], TABLE_EXT)]);
        assert!(relowered >= 1, "the table itself is lowered again");
        assert!(relowered < 8, "not the whole set ({relowered})");
    }

    #[test]
    fn extensions_and_implementers_in_the_workspace_give_the_complete_compile() {
        let relowered = assert_cached_equals_eager(deps(), vec![app("w", &["d1"], EVERYTHING)]);
        assert!(
            relowered >= 4,
            "table, page, interface users and the enum ({relowered})"
        );
    }

    fn workspace(id: &str, src: &str) -> Vec<AppSource> {
        vec![app(id, &["d1", "d2", "d3"], src)]
    }

    #[test]
    fn workspace_code_that_uses_dependency_queries_and_pages_gives_the_complete_compile() {
        assert_cached_equals_eager(deps(), workspace("w", USES_DEPENDENCY_OBJECTS));
    }

    #[test]
    fn extending_a_page_that_other_pages_show_as_parts_relowers_those_pages() {
        let relowered = assert_cached_equals_eager(deps(), workspace("w", EXTENDS_A_PART_PAGE));
        assert!(
            relowered >= 3,
            "the page, and the pages that show it as a part ({relowered})"
        );
    }

    #[test]
    fn a_report_extension_and_an_interface_implementation_in_the_workspace() {
        assert_cached_equals_eager(deps(), workspace("w", EXTENDS_REPORT_AND_INTERFACE));
    }

    /// The complete check runs on several threads (the eager compile and the cache build). What a
    /// routine checks to does not depend on the thread, so the program is the one a single thread
    /// makes.
    #[test]
    fn the_complete_check_gives_the_same_program_on_one_thread_and_on_several() {
        let mut src = String::from(BASE);
        for i in 0..150 {
            src.push_str(&format!(
                "codeunit {} \"Gen {i}\" {{ procedure P{i}(var R: Record T): Integer begin R.A := {i}; R.DX := 1; exit(R.A); end; \
                 procedure Q{i}(): Integer begin exit({i}); end; }}\n",
                80000 + i
            ));
        }
        let apps = vec![app("d1", &[], &src)];
        let kinds = vec![AppKind::Dependency];
        let make = |workers| {
            compile_apps_with(
                apps.clone(),
                kinds.clone(),
                &BTreeSet::new(),
                None,
                Roots {
                    eager: true,
                    workers: Some(workers),
                    ..Default::default()
                },
            )
            .1
        };
        let (one, many) = (make(1), make(6));
        assert!(*one.program == *many.program, "the programs differ");
        assert!(one.unsupported == many.unsupported, "the refusals differ");
        assert!(one.program.objects.len() > 150);
    }
}
