//! Resolves every name, member access and call in routine bodies, inferring
//! expression types. The corpus compiles on BC, so a diagnostic here means
//! the model is missing something, not that the AL is wrong.

use crate::members::{MemberTable, ProcSig};
use crate::types::Ty;
use crate::workspace::{Diagnostic, ObjKind, Resolution, Workspace};
use al_builtins::catalog::Catalog;
use al_builtins::signature::Signature;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use posmap::{FxMap as HashMap, FxSet as HashSet};

pub mod demand;

/// Wall time of the parts of a check (`--timings`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckTimings {
    /// The workspace's own objects. With dependency apps they are the first round of the demand
    /// and counted there.
    pub workspace: std::time::Duration,
    /// The install baseline's roots.
    pub baseline: std::time::Duration,
    /// The demand fixpoint over the dependency apps.
    pub demand: std::time::Duration,
    /// Sorting the diagnostics and laying out the queries.
    pub rest: std::time::Duration,
}

pub struct CheckResult {
    pub timings: CheckTimings,
    pub diagnostics: Vec<Diagnostic>,
    /// Inferred type of every expression, keyed by (file index, start offset, end offset).
    pub types: PosMap<Ty>,
    /// What each name, member and call resolved to, same keys.
    pub resolutions: PosMap<Res>,
    /// The layout of every query object, by object index.
    pub queries: Vec<Option<crate::queries::QueryLayout>>,
    /// With dependency apps: which of their objects and routines the workspace reaches.
    /// `None` when every object is checked.
    pub demand: Option<demand::Demand>,
    /// Per object: a test codeunit a selection left out (`Roots::only_tests`). Empty without one.
    pub skipped: Vec<bool>,
    /// Checks that panicked, by (object, routine offset) -- `ANY_ROUTINE` for the whole object --
    /// with the panic message. A panic is a bug in the compiler; the routine (or object) is lowered
    /// as a refusal, so the run goes on and the tests that reach it are not run.
    pub panicked: HashMap<(usize, usize), String>,
    /// The diagnostics the check of the objects raised, in the order they were found, and the ones
    /// `alcodes::check_workspace` raised; `diagnostics` is both, sorted.
    pub from_checker: Vec<Diagnostic>,
    pub from_codes: Vec<Diagnostic>,
}

/// The routine offset of a [`CheckResult::panicked`] entry that covers every routine of the object.
pub const ANY_ROUTINE: usize = usize::MAX;

/// The text of a panic payload.
pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic without a message".into())
}

impl CheckResult {
    /// Why checking routine `offset` of object `oi` panicked, if it did (the routine's own panic,
    /// or one that took the whole object).
    pub fn panic_of(&self, oi: usize, offset: usize) -> Option<&str> {
        self.panicked
            .get(&(oi, offset))
            .or_else(|| self.panicked.get(&(oi, ANY_ROUTINE)))
            .map(String::as_str)
    }

    /// Whether object `oi` was left out of the check by a test selection.
    pub fn is_skipped(&self, oi: usize) -> bool {
        self.skipped.get(oi).copied().unwrap_or(false)
    }
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
    /// A `protected var` variable of the page or report that the page or report extension extends
    /// (ADR-023), by lower-case name.
    BaseGlobal(String),
    /// `this.<protected variable of the base>` in an extension, by lower-case name: BC compiles it and raises when it
    /// runs (types/PYThisExtensions.al:ReportExtension_ThisBaseVariable_ReadInATrigger).
    ThisBaseGlobal(String),
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
    /// A member of a `DotNet` value: only its name is known at compile time (ADR-014).
    DotNetMember(String),
}

/// Extra roots of the demand-driven check, besides the workspace's own code (ADR-013 decision 5).
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// The install baseline: the install codeunits of the dependency apps and Base Application's
    /// `Company-Initialize` (codeunit 2).
    pub baseline: bool,
    /// A test selection (`al-test-cli --codeunit ...`): the test codeunits to run, as (app index,
    /// object id). Every other test codeunit of a workspace app is neither checked nor compiled
    /// (its routines are stubs that refuse), so the demand starts from the selected tests and the
    /// workspace's non-test code only. `None`: every test codeunit is a root.
    pub only_tests: Option<Vec<(usize, u32)>>,
    /// With `only_tests`: a procedure of a workspace codeunit (one with no Subtype) that no selected
    /// test names, directly or through the procedures it names, is neither checked nor compiled (a
    /// stub that refuses). Names are matched by spelling, so the set is a superset of what can be
    /// called. Triggers, event subscribers and codeunits with a Subtype always stay. For the compile
    /// that weighs whether a pure test needs the install baseline: code beside the test that touches
    /// tables must not drag the dependency apps in.
    pub reach_only: bool,
    /// Check and lower every dependency routine instead of what the workspace reaches. The
    /// environment variable `AL_EAGER_DEPENDENCIES` does the same for a whole process. Used to
    /// compare the demand-driven compile with the complete one (outcomes must not differ).
    pub eager: bool,
    /// How many threads check the routines the demand reaches in dependency apps. `None`: the
    /// environment variable `AL_CHECK_THREADS`, else the number of cores. The result does not
    /// depend on it.
    pub workers: Option<usize>,
    /// Per object: its lowered form comes from a compile cache (`provided.rs`), so it is neither
    /// checked nor given a query layout. Only with `eager`; empty: none.
    pub provided: Vec<bool>,
    /// With `provided`: the query layouts of the objects that come from the cache (the lowering of
    /// the workspace's code reads them), by object index.
    pub provided_queries: Vec<Option<crate::queries::QueryLayout>>,
    /// Check only these objects (the incremental compile: the objects of the files that changed). The
    /// result then holds the types, resolutions and diagnostics of those objects only; everything else
    /// is as for the complete check. Only without a demand state (`eager`).
    pub only_objects: Option<Vec<usize>>,
    /// With `only_objects`: `CheckResult::skipped` of the check before the change. Of the objects that
    /// are not checked again it is still right (a test codeunit is left out or not by its own text).
    pub skipped_before: Option<Vec<bool>>,
}

/// Members that platform enums do not have, as the AL compiler says (corpus must-not-compile 0010
/// to 0013 and 0019, BC 27.5: ExecutionMode has Standard and Debug). Only members a case pins are listed;
/// the member lists in `al_builtins::sysenum` are not complete enough to judge a name absent.
const ABSENT_ENUM_MEMBERS: &[(&str, &[&str])] = &[(
    "ExecutionMode",
    &["Install", "Uninstall", "Upgrade", "Report", "Profile"],
)];

pub fn check(ws: &Workspace, members: &MemberTable<'_>, catalog: &Catalog) -> CheckResult {
    check_with(ws, members, catalog, Roots::default())
}

pub fn check_with(ws: &Workspace, members: &MemberTable<'_>, catalog: &Catalog, roots: Roots) -> CheckResult {
    let t_start = std::time::Instant::now();
    let narrate = roots.only_objects.is_some() && std::env::var_os("ALRUNNER_INCR_TIMINGS").is_some();
    // With dependency apps, only what the workspace's code reaches in them is checked.
    let demand_mode = ws.kinds.contains(&crate::workspace::AppKind::Dependency)
        && !roots.eager
        && std::env::var_os("AL_EAGER_DEPENDENCIES").is_none();
    // A test codeunit the selection leaves out is not a root; nothing else in the workspace is.
    let skipped: Vec<bool> = match &roots.only_tests {
        None => Vec::new(),
        Some(keep) => (0..ws.objects.len())
            .map(|oi| match (&roots.only_objects, &roots.skipped_before) {
                (Some(only), Some(before)) if before.len() == ws.objects.len() && !only.contains(&oi) => before[oi],
                _ => unselected_test(ws, oi, keep),
            })
            .collect(),
    };
    let skip = |oi: usize| skipped.get(oi).copied().unwrap_or(false);
    if narrate {
        eprintln!("incremental: check: skipped {:.3}", t_start.elapsed().as_secs_f64());
    }
    if roots.reach_only && roots.only_tests.is_some() {
        ws.set_unreached(unreached_routines(ws, &skip));
    }
    let provided = |oi: usize| roots.provided.get(oi).copied().unwrap_or(false);
    let mut timings = CheckTimings::default();
    let mut c = Checker::new(ws, members, catalog, system_enums(catalog), demand_mode);
    // The files of the dependency objects the checker reaches are parsed ahead on other
    // threads; parsing is a third of the cost and the checker only waits for what is not ready.
    let demand = if demand_mode {
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        let rx = std::sync::Mutex::new(rx);
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let next = rx.lock().expect("prefetch queue").recv();
                        match next {
                            Ok(fi) => {
                                ws.green(fi);
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            c.prefetch = Some(tx);
            // The workspace's own objects are the first round of the demand: they are checked
            // with everything else, on the worker threads.
            let objects: Vec<usize> = (0..ws.objects.len())
                .filter(|&oi| !ws.is_dependency(oi) && !skip(oi))
                .collect();
            let started = std::time::Instant::now();
            if roots.baseline {
                c.reach_baseline();
            }
            timings.baseline = started.elapsed();
            let started = std::time::Instant::now();
            let demand = c.run_demand(roots.workers.map_or_else(check_workers, |w| w.clamp(1, 32)), objects);
            timings.demand = started.elapsed();
            c.prefetch = None;
            demand
        })
    } else {
        let started = std::time::Instant::now();
        let objects: Vec<usize> = (0..ws.objects.len())
            .filter(|&oi| !skip(oi) && !provided(oi))
            .filter(|oi| roots.only_objects.as_ref().is_none_or(|only| only.contains(oi)))
            .collect();
        let workers = roots.workers.map_or_else(check_workers, |w| w.clamp(1, 32));
        if workers > 1 && objects.len() >= PARALLEL_OBJECTS {
            c.check_objects_parallel(workers, &objects);
        } else {
            for oi in objects {
                c.guard((oi, ANY_ROUTINE), |c| c.check_object(oi));
                c.take_calls();
            }
        }
        timings.workspace = started.elapsed();
        None
    };
    if narrate {
        eprintln!("incremental: check: objects {:.3}", t_start.elapsed().as_secs_f64());
    }
    let rest_started = std::time::Instant::now();
    let positions: HashMap<&str, usize> = ws.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
    let from_checker = c.diags.clone();
    let from_codes = crate::alcodes::check_workspace(ws);
    c.diags.extend(from_codes.iter().cloned());
    c.diags
        .sort_by_key(|d| (positions.get(d.file.as_str()).copied(), d.offset));
    // Without a demand state (the eager compile) every dependency object is lowered, so every one
    // needs its layout.
    if narrate {
        eprintln!("incremental: check: codes {:.3}", t_start.elapsed().as_secs_f64());
    }
    let wanted = |oi: usize| !provided(oi) && (!ws.is_dependency(oi) || demand.as_ref().is_none_or(|d| d.active(oi)));
    for oi in 0..ws.objects.len() {
        if wanted(oi) {
            c.query(oi);
        }
    }
    let queries = c
        .queries
        .into_iter()
        .enumerate()
        .map(|(oi, cell)| {
            if provided(oi) {
                roots.provided_queries.get(oi).cloned().flatten()
            } else {
                cell.into_inner().flatten().filter(|_| wanted(oi))
            }
        })
        .collect();
    timings.rest = rest_started.elapsed();
    CheckResult {
        timings,
        diagnostics: c.diags,
        types: c.types,
        resolutions: c.resolutions,
        queries,
        demand,
        skipped,
        panicked: c.panicked,
        from_checker,
        from_codes,
    }
}

/// The complete check of fewer objects than this stays on the calling thread.
const PARALLEL_OBJECTS: usize = 64;

/// How many threads check the reached routines of dependency apps. `AL_CHECK_THREADS` sets it
/// (1 checks on one thread); the default is the number of cores.
fn check_workers() -> usize {
    std::env::var("AL_CHECK_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, 32)
}

/// The procedures of the workspace's normal codeunits that no root names (`Roots::reach_only`). The
/// roots are every other routine of the objects that are checked: test codeunits, triggers, event
/// subscribers, objects that are not normal codeunits. A name in a root's text, or in the text of a
/// procedure so named, makes every procedure of that name reachable.
fn unreached_routines(ws: &Workspace, skip: &dyn Fn(usize) -> bool) -> HashSet<(usize, usize)> {
    let names_in = |r: &ast::Routine| -> HashSet<String> {
        r.syntax()
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            .map(|t| ast::ident_text(&t).to_lowercase())
            .collect()
    };
    // Free procedures by lower-case name; the rest are roots.
    let mut free: HashMap<String, Vec<(usize, usize, HashSet<String>)>> = HashMap::default();
    let mut wanted: HashSet<String> = HashSet::default();
    for oi in 0..ws.objects.len() {
        if ws.is_dependency(oi) || skip(oi) {
            continue;
        }
        let obj = ws.object_ast(oi);
        let normal_codeunit = ws.objects[oi].kind == ObjKind::Codeunit
            && !obj.properties().any(|p| p.name().eq_ignore_ascii_case("Subtype"));
        for r in ast::routines_in(obj.syntax()) {
            let is_root = !normal_codeunit
                || r.is_trigger()
                || r.attributes().any(|a| {
                    let n = a.name();
                    n.eq_ignore_ascii_case("EventSubscriber") || n.to_ascii_lowercase().ends_with("event")
                });
            let names = names_in(&r);
            if is_root {
                wanted.extend(names);
            } else {
                let offset = usize::from(r.syntax().text_range().start());
                free.entry(r.name().to_lowercase())
                    .or_default()
                    .push((oi, offset, names));
            }
        }
    }
    let mut reached: HashSet<(usize, usize)> = HashSet::default();
    let mut queue: Vec<String> = wanted.iter().cloned().collect();
    let mut asked: HashSet<String> = wanted;
    while let Some(name) = queue.pop() {
        if let Some(rs) = free.get(&name) {
            for (oi, offset, names) in rs {
                if reached.insert((*oi, *offset)) {
                    for n in names {
                        if asked.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
            }
        }
    }
    free.into_values()
        .flatten()
        .map(|(oi, offset, _)| (oi, offset))
        .filter(|k| !reached.contains(k))
        .collect()
}

/// Whether object `oi` is a test codeunit of a workspace app that `keep` (app index, id) leaves out.
fn unselected_test(ws: &Workspace, oi: usize, keep: &[(usize, u32)]) -> bool {
    let o = &ws.objects[oi];
    if o.kind != ObjKind::Codeunit || ws.is_dependency(oi) {
        return false;
    }
    let Some(id) = o.id else { return false };
    if keep.contains(&(o.app, id)) {
        return false;
    }
    let ast = ws.object_ast(oi);
    let is_test = ast
        .properties()
        .any(|p| p.name().eq_ignore_ascii_case("Subtype") && p.value_text().eq_ignore_ascii_case("Test"));
    // A test codeunit that subscribes to events listens during every test, so it stays compiled. The text
    // of the object is searched token by token (no word of the search spans two tokens).
    is_test
        && !ast.syntax().descendants_with_tokens().any(|e| {
            e.as_token()
                .is_some_and(|t| contains_ignore_case(t.text(), b"eventsubscriber"))
        })
}

/// Whether `haystack` holds `needle` (lower case ASCII) in any case.
fn contains_ignore_case(haystack: &str, needle: &[u8]) -> bool {
    let h = haystack.as_bytes();
    h.len() >= needle.len()
        && h.windows(needle.len())
            .any(|w| w.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

/// Types that appear in the catalog's signatures but are not receivers or
/// primitives: the platform's option/enum types (`Verbosity`, `ObjectType`, ...),
/// whose values are written `Verbosity::Normal`.
fn system_enums(catalog: &Catalog) -> HashSet<String> {
    let receivers: HashSet<String> = catalog.signatures.iter().map(|s| s.receiver.to_lowercase()).collect();
    let mut out = HashSet::default();
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
pub(crate) enum E {
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
pub(crate) struct Cand {
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
    /// Set when this overload exists only in a newer runtime than the target's
    /// (ADR-015 3.3). Such a candidate is never called; it only explains a refusal.
    hidden: Option<al_version::Runtime>,
}

impl Cand {
    fn is_field_pos(&self, i: usize) -> bool {
        self.fields.get(i).copied().unwrap_or(self.rest_field)
    }
}

pub(crate) struct Scope {
    oi: usize,
    file: usize,
    /// The object's variables, shared by the routines of the object.
    globals: std::rc::Rc<posmap::FxMap<String, Ty>>,
    /// The routine's own frames (parameters and locals), the last first.
    vars: Vec<posmap::FxMap<String, Ty>>,
    /// The implicit record of table/page code (and `Rec` in codeunits with TableNo).
    rec: Option<Ty>,
    /// Whether the implicit record's fields and methods can be used unqualified.
    rec_unqualified: bool,
    /// Object whose procedures are callable unqualified.
    self_obj: usize,
    /// In a page or report extension: the page or report it extends (ADR-023).
    base_obj: Option<usize>,
    /// The names of the base object's variables that this extension sees (they are also in
    /// `globals`).
    base_vars: std::rc::Rc<std::collections::HashSet<String>>,
}

/// A section with its first argument (its name, for controls and data items).
type NamedSection = (Option<String>, ast::Section);

pub(crate) struct Checker<'a> {
    ws: &'a Workspace,
    members: &'a MemberTable<'a>,
    catalog: &'a Catalog,
    diags: Vec<Diagnostic>,
    types: PosMap<Ty>,
    resolutions: PosMap<Res>,
    system_enums: HashSet<String>,
    /// `sections_of` memo by object, for `named_section`.
    sections: std::cell::RefCell<HashMap<usize, Vec<NamedSection>>>,
    /// The layout of each query object, computed when first asked for.
    queries: Vec<std::cell::OnceCell<Option<crate::queries::QueryLayout>>>,
    /// Procedures the routine being checked calls, as (object, routine offset).
    calls: Vec<(usize, usize)>,
    /// Which dependency objects the workspace reaches (`None`: everything is checked).
    demand: std::cell::RefCell<Option<demand::State>>,
    /// Files to parse ahead on other threads: those of the dependency objects just reached.
    prefetch: Option<std::sync::mpsc::Sender<usize>>,
    /// What `resolve_ref` found for a name written in a file, by (file, kind) and then name. While
    /// a check runs the answer does not change (nothing is left out of the compile before it ends),
    /// and the same few names are asked for again and again by the routines of a file.
    resolved: std::cell::RefCell<Resolved>,
    /// Checks that panicked (see [`CheckResult::panicked`]).
    panicked: HashMap<(usize, usize), String>,
}

/// Names resolved so far, by (file, kind) and then name; see [`Checker::resolved`].
type Resolved = HashMap<(usize, ObjKind), HashMap<String, Option<usize>>>;

/// What every routine of an object sees.
pub(crate) struct ObjCtx {
    oi: usize,
    file: usize,
    globals: std::rc::Rc<posmap::FxMap<String, Ty>>,
    rec: Option<Ty>,
    rec_unqualified: bool,
    self_obj: usize,
    base_items: HashMap<String, String>,
    base_obj: Option<usize>,
    base_vars: std::rc::Rc<std::collections::HashSet<String>>,
}

/// The `this` keyword: the object whose code is running (Microsoft Learn, "this keyword").
pub(crate) fn is_this_keyword(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::NameRef && name_of(node).eq_ignore_ascii_case("this")
}

/// Whether a variable of this name is in scope, whatever frame.
fn sc_has_var(sc: &Scope, name: &str) -> bool {
    sc.globals.contains_key(name) || sc.vars.iter().any(|frame| frame.contains_key(name))
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

/// The name written as the right operand of `is` and `as`: `I`, `"A B"` or `Ns.I`.
pub(crate) fn type_name_text(node: &SyntaxNode) -> String {
    match node.kind() {
        SyntaxKind::MemberExpr => node
            .children()
            .map(|k| type_name_text(&k))
            .collect::<Vec<_>>()
            .join("."),
        _ => name_of(node),
    }
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
            // An operand whose type is known only when the code runs gives a result of the same
            // kind (ADR-024); the VM's operators decide from the settled kinds.
            (Ty::DotNet(_), _) | (_, Ty::DotNet(_)) => Ty::DotNet("".into()),
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
            Ty::Builtin(n) => return Some(n.to_string()),
            _ => return None,
        }
        .to_string(),
    )
}

/// For `Rec.ReadIsolation` and `Rec.SecurityFiltering` written before `::`: the system option the
/// property takes and the position of member `member` in it.
fn system_option_of_property(base: &SyntaxNode, member: &str) -> Option<(&'static str, usize)> {
    let text = base.text().to_string();
    let last = text.rsplit('.').next()?.trim().trim_matches('"').to_ascii_lowercase();
    let enum_name = match last.as_str() {
        "readisolation" => "IsolationLevel",
        "securityfiltering" => "SecurityFilter",
        _ => return None,
    };
    let i = al_builtins::sysenum::members(enum_name)?
        .iter()
        .position(|m| m.eq_ignore_ascii_case(member))?;
    Some((enum_name, i))
}

mod calls;
mod fit;
mod infer;
mod member;
mod object;
pub(crate) mod posmap;
#[cfg(test)]
mod tests;

pub(crate) use self::fit::*;
pub use self::posmap::*;
