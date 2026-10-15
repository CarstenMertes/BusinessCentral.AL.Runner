//! Runs test codeunits the way BC's test runner does, as far as the corpus
//! pins it: one instance per test codeunit, `OnRun` once before the tests
//! (codeunit/TestCodeunitOnRun.al), tests in declaration order.

use crate::explain::{self, TestDetail};
use crate::isolation::{Checkpoint, TestIsolation, runner_isolation};
use crate::{CaseId, Outcome};
use al_builtins::assumptions::Assumption;
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_sema::lower::Lowered;
use al_vm::{Engine, Session, UiKind};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Reads an app folder: its `app.json` manifest and every `.al` file under it.
pub fn load_app(dir: &std::path::Path) -> Result<al_sema::workspace::AppSource, String> {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("app.json")).map_err(|e| format!("{}: {e}", dir.display()))?,
    )
    .map_err(|e| format!("{}/app.json: {e}", dir.display()))?;
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().into_owned();
                files.push((
                    rel,
                    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                ));
            }
        }
    }
    files.sort();
    let s = |k: &str| manifest[k].as_str().unwrap_or_default().to_string();
    Ok(al_sema::workspace::AppSource {
        id: s("id"),
        name: s("name"),
        dependencies: manifest["dependencies"]
            .as_array()
            .map(|a| a.iter().filter_map(|d| d["id"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        files,
    })
}

/// What only an app's manifest says that `ModuleInfo` reports (publisher and version).
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    pub publisher: String,
    pub version: String,
    /// The files under the manifest's `resourceFolders`, named relative to their folder.
    pub resources: Vec<(String, Vec<u8>)>,
    /// `resourceExposurePolicy.includeSourceInSymbolFile`: the Code Coverage table shows the source lines.
    pub include_source: bool,
    /// `preprocessorSymbols` of the app's own app.json. A dependency app is compiled with these;
    /// the project's symbols apply to the workspace apps only (the publisher compiled the dependency).
    pub symbols: Vec<String>,
}

/// The manifest fields `NavApp.GetCurrentModuleInfo` reports, from `dir/app.json`.
pub fn load_manifest(dir: &std::path::Path) -> Result<Manifest, String> {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("app.json")).map_err(|e| format!("{}: {e}", dir.display()))?,
    )
    .map_err(|e| format!("{}/app.json: {e}", dir.display()))?;
    let s = |k: &str| manifest[k].as_str().unwrap_or_default().to_string();
    let mut resources = Vec::new();
    for folder in manifest["resourceFolders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_str())
    {
        let root = dir.join(folder);
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries {
                let p = e.map_err(|e| e.to_string())?.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    resources.push((rel, std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
                }
            }
        }
    }
    resources.sort();
    Ok(Manifest {
        publisher: s("publisher"),
        version: s("version"),
        resources,
        include_source: manifest["resourceExposurePolicy"]["includeSourceInSymbolFile"]
            .as_bool()
            .unwrap_or(false),
        symbols: manifest["preprocessorSymbols"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(String::from))
            .collect(),
    })
}

/// Every corpus test's outcome on this runtime, keyed by (codeunit id, procedure).
pub struct CorpusRuntime {
    /// Keyed by (suite, test codeunit id, lower-cased procedure). A suite is one test runner run
    /// over some of the apps; the same codeunit can be in two (the isolation probe).
    outcomes: std::collections::HashMap<(usize, u32, String), Outcome>,
    /// How long each test that ran took, keyed like `outcomes`.
    times: std::collections::HashMap<(usize, u32, String), std::time::Duration>,
    suites: Vec<String>,
    pub panics: usize,
    /// How long the compile, the install baseline and the tests took.
    pub phases: Phases,
    /// With `RunOptions::trace`: where each test of the run codeunits was refused or failed.
    details: std::collections::HashMap<(usize, u32, String), TestDetail>,
    /// Every test the run decided, codeunit by codeunit.
    results: Vec<TestResult>,
    /// The unprobed assumptions each test ran through, same key as `outcomes`; tests with none are absent.
    marks: std::collections::HashMap<(usize, u32, String), Vec<Assumption>>,
    /// With `RunOptions::coverage`: every statement of the workspace's apps.
    coverage_statements: Vec<crate::coverage::CoverageStatement>,
}

/// One test as the run left it, in the order the test codeunits were scheduled.
#[derive(Debug, Clone)]
pub struct TestResult {
    /// Index into the `SuiteRun`s of the run.
    pub suite: usize,
    /// Index of the app (as given to the compiler) that holds the test codeunit.
    pub app: usize,
    pub codeunit: u32,
    pub codeunit_name: String,
    pub test: String,
    pub outcome: Outcome,
    /// For a failure with an AL error: the call stack of the error, innermost frame first, one per
    /// line (`GetLastErrorCallStack` form). Empty otherwise.
    pub call_stack: String,
    /// The unprobed assumptions the test ran through (`al_builtins::assumptions`), in registry order. A test
    /// that passed with a non-empty list is a flagged pass.
    pub assumptions: Vec<Assumption>,
    /// The wall time the test took; zero for a test that was not started.
    pub time: std::time::Duration,
    /// With `RunOptions::coverage`: the statements the test ran, as (position id, runs), sorted by id. Position ids
    /// index `CorpusRuntime::coverage_statements`' `pos` and `Program::positions`.
    pub coverage: Vec<(u32, u32)>,
}

/// Wall time of the phases of a run (`--timings`).
#[derive(Debug, Clone, Default)]
pub struct Phases {
    pub compile: std::time::Duration,
    pub baseline: std::time::Duration,
    pub tests: std::time::Duration,
    /// The tests phase by run, in the order of the `SuiteRun`s (name, wall time).
    pub suites: Vec<(String, std::time::Duration)>,
    /// The parts of the compile.
    pub compile_parts: al_sema::lower::CompileTimings,
    /// What the compile cache did, when the run used it.
    pub cache: Option<crate::compile_cache::CacheReport>,
    /// Building the engine from the lowered program.
    pub engine: std::time::Duration,
    /// The install baseline did not run because no selected test can observe it (`observe.rs`).
    pub baseline_skipped: bool,
    /// Why the baseline ran although the rule was tried: the first construct a selected test
    /// reaches that could observe it. `None`: the rule was not tried (no dependency app,
    /// `AL_FORCE_BASELINE`, a custom test runner) or the baseline was skipped.
    pub baseline_why: Option<String>,
    /// The compile cache was asked for and not used, because the selected tests were compiled
    /// first without the baseline and nothing needed the cache.
    pub compile_cache_skipped: bool,
    /// With such a compile and lazily read dependency apps: where the scan of their sources came from,
    /// `read` (the index in the cache folder) or `built` (scanned now, index saved).
    pub source_index: Option<&'static str>,
    /// The test codeunits (object id, name) that took longest, slowest first (at most
    /// [`SLOWEST`]).
    pub slowest: Vec<CodeunitTime>,
}

impl Phases {
    /// One line for `--timings`: whether the install baseline ran, and if the rule that skips it
    /// was tried and said no, the first construct a selected test reaches that could observe it.
    /// `None` when there was nothing to say (no dependency app).
    pub fn baseline_line(&self) -> Option<String> {
        if self.baseline_skipped {
            Some(format!(
                "install baseline: not run, no selected test can observe it{}",
                match (self.compile_cache_skipped, self.source_index) {
                    (true, Some(how)) => format!(
                        "; compile cache: not used, compiling what the tests reach was enough; source index: {how}"
                    ),
                    (true, None) => "; compile cache: not used, compiling what the tests reach was enough".to_string(),
                    (false, _) => String::new(),
                }
            ))
        } else {
            self.baseline_why
                .as_ref()
                .map(|why| format!("install baseline: ran, a selected test may observe it: {why}"))
        }
    }
}

/// How long one test codeunit took.
#[derive(Debug, Clone)]
pub struct CodeunitTime {
    pub id: u32,
    pub name: String,
    /// Wall time of the whole codeunit.
    pub took: std::time::Duration,
    /// The part of `took` spent starting .NET sidecar processes (ADR-014): start-up of a helper
    /// process, not the runtime. A budget on the runtime subtracts it.
    pub sidecar_start: std::time::Duration,
}

/// How many test codeunits `--timings` names.
pub const SLOWEST: usize = 8;

/// A test codeunit to run: its object, id and the isolation of the runner it belongs to.
struct Unit {
    si: usize,
    oi: usize,
    id: u32,
    isolation: TestIsolation,
    /// Run without putting the install baseline back (`AL_CHECK_BASELINE_RULE`).
    without_baseline: bool,
    /// Count the statements each test runs (`RunOptions::coverage`).
    coverage: bool,
}

/// What a test codeunit left: each test's outcome, the call stack of each AL error that failed one, how long
/// each test that ran took (wall time), and the unprobed assumptions each test ran through.
type UnitOutcomes = (
    Vec<(String, Outcome)>,
    Vec<(String, String)>,
    Vec<(String, std::time::Duration)>,
    Vec<(String, Vec<Assumption>)>,
    Vec<(String, Vec<(u32, u32)>)>,
);

struct UnitResult {
    si: usize,
    oi: usize,
    id: u32,
    /// The outcome of each test, or the message of a Rust panic.
    result: Result<UnitOutcomes, String>,
    took: std::time::Duration,
    sidecar_start: std::time::Duration,
}

/// The `[Test]` routines of a codeunit.
fn test_routines(o: &al_ir::Object) -> impl Iterator<Item = &al_ir::Routine> {
    o.routines
        .iter()
        .filter(|r| r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("Test")))
}

/// Runs one test codeunit. A Rust panic is a runtime bug; it is contained to this codeunit.
fn run_unit(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    baseline: Option<&al_vm::Baseline>,
    u: &Unit,
    observe: Option<&mut Observer>,
    limit: Option<std::time::Duration>,
) -> UnitResult {
    let started = std::time::Instant::now();
    crate::dotnet::take_start_time();
    let baseline = baseline.filter(|_| !u.without_baseline);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut stacks = Vec::new();
        let mut times = Vec::new();
        let mut hits = Vec::new();
        if u.coverage {
            crate::coverage::begin();
        }
        let done = run_codeunit_full(
            engine,
            lowered,
            u.oi,
            baseline,
            u.isolation,
            observe,
            &mut stacks,
            &mut times,
            &mut hits,
            limit,
        );
        (done, stacks, times, hits, crate::coverage::finish())
    }))
    .map_err(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    });
    UnitResult {
        si: u.si,
        oi: u.oi,
        id: u.id,
        result,
        took: started.elapsed(),
        sidecar_start: crate::dotnet::take_start_time(),
    }
}

/// Whether object `oi` is a test codeunit that `suite` runs under the selection `only`.
fn is_selected_test(lowered: &Lowered, oi: usize, suite: &SuiteRun, only: &Option<Vec<(usize, u32)>>) -> bool {
    let o = &lowered.program.objects[oi];
    o.kind == "codeunit"
        && !lowered.dependency[oi]
        && suite.app.is_none_or(|a| o.app as usize == a)
        && only
            .as_ref()
            .is_none_or(|only| o.id.is_some_and(|id| only.contains(&(o.app as usize, id))))
        && o.properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Test"))
}

/// The objects (indices) of every test codeunit the suites run.
fn selected_tests(lowered: &Lowered, suites: &[SuiteRun], only: &Option<Vec<(usize, u32)>>) -> Vec<usize> {
    (0..lowered.program.objects.len())
        .filter(|&oi| suites.iter().any(|s| is_selected_test(lowered, oi, s, only)))
        .collect()
}

/// How many threads run test codeunits. `AL_TEST_THREADS` sets it (1 runs them one after the
/// other on the calling thread); the default is the number of cores.
fn test_workers() -> usize {
    std::env::var("AL_TEST_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, 64)
}

/// Stack of a test thread: the VM recurses on the native stack per AL call (`MAX_CALL_DEPTH`).
const TEST_STACK: usize = 256 << 20;

/// Drops `value` on another thread. The process does not wait for it when it ends.
pub fn retire<T: Send + 'static>(value: T) {
    std::thread::spawn(move || drop(value));
}

/// What a run is asked to do beyond running everything.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Only these test codeunits (app index of the compile, object id) are roots of the compile
    /// and are run. `None`: all of them.
    pub only: Option<Vec<(usize, u32)>>,
    /// Record where tests are refused or fail (`--explain`).
    pub trace: bool,
    /// Rows of a restored company, read when a session first touches a table (ADR-020). `None`:
    /// a run without test data, as every corpus run is.
    pub test_data: Option<al_host::testdata::TestData>,
    /// Use the compile cache of the dependency apps (ADR-022). `None`: the demand-driven compile
    /// of every run, which is the reference path.
    pub compile_cache: Option<crate::compile_cache::CompileCache>,
    /// A test that runs longer than this is stopped and reported as not run. `None`: no limit.
    pub test_time_limit: Option<std::time::Duration>,
    /// Per app (in the order given to the run): a dependency app whose files hold no text yet, and how
    /// to read them (`deps::load_project_with`). The run reads what it needs; a missing entry means
    /// the app has its text.
    pub lazy: Vec<Option<al_sema::LazySource>>,
    /// `alrunner test --isolation`: the isolation of every test codeunit, instead of the runner's property (`None`: the runner's,
    /// else Codeunit).
    pub isolation: Option<TestIsolation>,
    /// Count how often each statement of the workspace's apps runs, per test (`alrunner test --coverage`).
    /// Off, a run pays one branch per statement.
    pub coverage: bool,
}

/// A test runner run over the test codeunits of some apps (the unit of an oracle file).
#[derive(Debug, Clone)]
pub struct SuiteRun {
    /// The name the report uses; empty for the main corpus app.
    pub name: String,
    /// The test codeunits of the app at this index of the apps given to the compiler. `None`:
    /// those of every app that is not a dependency.
    pub app: Option<usize>,
    /// The runner codeunit whose `TestIsolation` the run uses. `None`: Codeunit, which is what
    /// Microsoft's `Test Runner - Isol. Codeunit` the main corpus is recorded under does.
    pub runner: Option<u32>,
}

impl SuiteRun {
    /// Every test codeunit of every non-dependency app, under Codeunit isolation.
    pub fn everything() -> SuiteRun {
        SuiteRun {
            name: String::new(),
            app: None,
            runner: None,
        }
    }
}

impl CorpusRuntime {
    pub fn run(apps: Vec<al_sema::workspace::AppSource>, symbols: &BTreeSet<String>) -> CorpusRuntime {
        Self::run_with_manifests(apps, &[], symbols)
    }

    /// `manifests[i]` belongs to `apps[i]`: the publisher and version `ModuleInfo` reports.
    pub fn run_with_manifests(
        apps: Vec<al_sema::workspace::AppSource>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
    ) -> CorpusRuntime {
        Self::run_with_dotnet(apps, manifests, symbols, al_vm::dotnet::DotNetConfig::default())
    }

    /// [`run_with_manifests`](Self::run_with_manifests) with a `DotNet` host (ADR-014); the
    /// default has none.
    pub fn run_with_dotnet(
        apps: Vec<al_sema::workspace::AppSource>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        dotnet: al_vm::dotnet::DotNetConfig,
    ) -> CorpusRuntime {
        Self::run_for(apps, manifests, symbols, None, dotnet)
    }

    /// Runs for a target: builtins newer than the target's runtime are refused and version gates
    /// follow it (ADR-015). Without a target, the newest catalog and behaviour are used.
    pub fn run_for(
        apps: Vec<al_sema::workspace::AppSource>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
        dotnet: al_vm::dotnet::DotNetConfig,
    ) -> CorpusRuntime {
        let kinds = vec![al_sema::AppKind::Workspace; apps.len()];
        Self::run_for_apps(apps, kinds, manifests, symbols, target, dotnet)
    }

    /// [`run_for`](Self::run_for) with the kind of each app. Dependency apps are compiled but no
    /// test is run in them (ADR-013 decision 4).
    pub fn run_for_apps(
        apps: Vec<al_sema::workspace::AppSource>,
        kinds: Vec<al_sema::AppKind>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
        dotnet: al_vm::dotnet::DotNetConfig,
    ) -> CorpusRuntime {
        Self::run_suites(
            apps,
            kinds,
            manifests,
            symbols,
            target,
            dotnet,
            &[SuiteRun::everything()],
        )
    }

    /// [`run_for_apps`](Self::run_for_apps) for several runs over the same compiled apps: they
    /// share one compile and one install baseline, and each test codeunit of a run is an isolation
    /// unit on a session of its own (`isolation.rs`).
    pub fn run_suites(
        apps: Vec<al_sema::workspace::AppSource>,
        kinds: Vec<al_sema::AppKind>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
        dotnet: al_vm::dotnet::DotNetConfig,
        suites: &[SuiteRun],
    ) -> CorpusRuntime {
        Self::run_suites_with(
            apps,
            kinds,
            manifests,
            symbols,
            target,
            dotnet,
            suites,
            &RunOptions::default(),
        )
    }

    /// [`run_suites`](Self::run_suites) with a test selection and tracing (`RunOptions`).
    #[allow(clippy::too_many_arguments)]
    pub fn run_suites_with(
        apps: Vec<al_sema::workspace::AppSource>,
        kinds: Vec<al_sema::AppKind>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
        dotnet: al_vm::dotnet::DotNetConfig,
        suites: &[SuiteRun],
        opts: &RunOptions,
    ) -> CorpusRuntime {
        Self::try_run_suites_with(apps, kinds, manifests, symbols, target, dotnet, suites, opts)
            .expect("a run without `require` does not fail to start")
    }

    /// [`run_suites_with`](Self::run_suites_with), which says why a run could not start: the
    /// compile cache is required (`CompileCache::require`) and has no usable entry.
    #[allow(clippy::too_many_arguments)]
    pub fn try_run_suites_with(
        apps: Vec<al_sema::workspace::AppSource>,
        kinds: Vec<al_sema::AppKind>,
        manifests: &[Manifest],
        symbols: &BTreeSet<String>,
        target: Option<&al_version::Target>,
        dotnet: al_vm::dotnet::DotNetConfig,
        suites: &[SuiteRun],
        opts: &RunOptions,
    ) -> Result<CorpusRuntime, String> {
        // AL_NO_BASELINE=1 leaves the install baseline out (to measure what it costs).
        let baseline_wanted =
            kinds.contains(&al_sema::AppKind::Dependency) && std::env::var_os("AL_NO_BASELINE").is_none();
        // The baseline runs only when a selected test can observe it (`observe.rs`).
        // AL_FORCE_BASELINE=1 runs it always: the other side of the differential check, which
        // must show no outcome that changes. A run with a custom test runner keeps it too.
        let try_skip = baseline_wanted
            && std::env::var_os("AL_FORCE_BASELINE").is_none()
            && suites.iter().all(|s| s.runner.is_none());
        let compile_started = std::time::Instant::now();
        // The compile cache holds the dependency apps, which come first in the program (ADR-022).
        // The caller's order is kept in `order` (program app -> caller app) for what it gets back.
        let use_cache = opts.compile_cache.is_some()
            && kinds.contains(&al_sema::AppKind::Dependency)
            && std::env::var_os("AL_EAGER_DEPENDENCIES").is_none();
        let n_apps = apps.len();
        let (apps, mut kinds, manifests, suites_in, only, order) = if use_cache {
            // Dependency apps first, Microsoft's before the others (the layers of the compile
            // cache follow this order), then the workspace's.
            let deps: Vec<usize> = (0..n_apps)
                .filter(|&i| kinds[i] == al_sema::AppKind::Dependency)
                .collect();
            let dep_apps: Vec<&al_sema::workspace::AppSource> = deps.iter().map(|&i| &apps[i]).collect();
            let dep_manifests: Vec<Manifest> = deps
                .iter()
                .map(|&i| manifests.get(i).cloned().unwrap_or_default())
                .collect();
            let order: Vec<usize> = crate::compile_cache::dependency_order(&dep_apps, &dep_manifests)
                .into_iter()
                .map(|k| deps[k])
                .chain((0..n_apps).filter(|&i| kinds[i] != al_sema::AppKind::Dependency))
                .collect();
            let mut new_of = vec![0; n_apps];
            for (new, &old) in order.iter().enumerate() {
                new_of[old] = new;
            }
            let mut slots: Vec<Option<al_sema::workspace::AppSource>> = apps.into_iter().map(Some).collect();
            let apps: Vec<_> = order.iter().map(|&i| slots[i].take().expect("each app once")).collect();
            let kinds: Vec<_> = order.iter().map(|&i| kinds[i]).collect();
            let manifests: Vec<Manifest> = order
                .iter()
                .map(|&i| manifests.get(i).cloned().unwrap_or_default())
                .collect();
            let suites: Vec<SuiteRun> = suites
                .iter()
                .map(|s| SuiteRun {
                    app: s.app.map(|a| new_of[a]),
                    ..s.clone()
                })
                .collect();
            let only = opts
                .only
                .as_ref()
                .map(|v| v.iter().map(|&(a, id)| (new_of[a], id)).collect::<Vec<_>>());
            (apps, kinds, manifests, suites, only, order)
        } else {
            (
                apps,
                kinds,
                manifests.to_vec(),
                suites.to_vec(),
                opts.only.clone(),
                (0..n_apps).collect(),
            )
        };
        let suites = &suites_in[..];
        let manifests = &manifests[..];
        // A dependency app is compiled with the symbols of its own app.json, the project's apps with the
        // project's (hard rule 6: a project that defines CLEAN25 does not change Base Application).
        let dep_symbols: Vec<Option<BTreeSet<String>>> = kinds
            .iter()
            .enumerate()
            .map(|(i, k)| {
                (*k == al_sema::AppKind::Dependency).then(|| {
                    manifests
                        .get(i)
                        .map(|m| m.symbols.iter().cloned().collect())
                        .unwrap_or_default()
                })
            })
            .collect();
        // The apps whose files hold no text yet, in program order.
        let lazy: Vec<Option<al_sema::LazySource>> =
            order.iter().map(|&i| opts.lazy.get(i).cloned().flatten()).collect();
        let identities: Vec<Option<[u8; 32]>> = lazy.iter().map(|l| l.as_ref().map(|l| l.identity)).collect();
        let any_lazy = lazy.iter().any(Option::is_some);
        let fill_all = |apps: &mut Vec<al_sema::workspace::AppSource>| {
            for (a, l) in apps.iter_mut().zip(&lazy) {
                if let Some(l) = l {
                    l.fill(a);
                }
            }
        };
        let roots = al_sema::check::Roots {
            baseline: baseline_wanted,
            only_tests: only.clone(),
            ..Default::default()
        };
        // Whether the install baseline runs, and why it does when it was weighed.
        let mut baseline = baseline_wanted;
        let mut baseline_note: Option<String> = None;
        let catalog = match target {
            Some(t) => al_builtins::catalog::Catalog::for_target(t),
            None => al_builtins::catalog::Catalog::load(),
        };
        let mut apps = apps;
        // Selected tests that look free of data are compiled first the way the demand-driven path
        // does it, without the baseline's roots and without the compile cache: that touches only what
        // the tests reach (a few routines), where the cache decodes every dependency object and the
        // demand-driven compile with the baseline reaches 421 routines of Base Application. If the rule
        // (`observe.rs`) then says the tests cannot observe the baseline, that is the compile. If it says
        // they may, the apps go on to the compile they would have had. `--require-compile-cache` (CI:
        // the measured run must be a hit) never takes this path.
        let mut compile_cache_skipped = false;
        let mut weighed = false;
        let mut compiled: Option<(al_sema::Workspace, Lowered)> = None;
        let require_cache = opts.compile_cache.as_ref().is_some_and(|c| c.require);
        let mut source_index: Option<&'static str> = None;
        if try_skip && !require_cache && crate::observe::text_looks_pure(&apps, &kinds, symbols, only.as_deref()) {
            let started = std::time::Instant::now();
            let kinds_again = kinds.clone();
            // The dependency apps that were read lazily are loaded from their index when the cache folder
            // has one: no source is read or scanned. Without one the sources are read, scanned, and the
            // index is saved for the next run.
            let cc = opts.compile_cache.as_ref();
            let (ws, filled) = match (any_lazy, cc) {
                (true, Some(cc)) => match lazy_index(cc, &apps, manifests, &lazy, &identities) {
                    Ok(deps) => {
                        source_index = Some("read");
                        (
                            al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, deps),
                            false,
                        )
                    }
                    Err(key) => {
                        fill_all(&mut apps);
                        let ws = al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, Vec::new());
                        save_index(cc, &ws, &lazy, &key);
                        source_index = Some("built");
                        (ws, true)
                    }
                },
                _ => {
                    fill_all(&mut apps);
                    (
                        al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, Vec::new()),
                        true,
                    )
                }
            };
            let load = started.elapsed();
            // Without the baseline's roots, and without the workspace's procedures no selected test names:
            // a helper beside the tests that touches tables must not pull the dependency apps in.
            let first_roots = al_sema::check::Roots {
                baseline: false,
                reach_only: true,
                ..roots.clone()
            };
            let (mut ws, lowered) = al_sema::lower::compile_loaded(ws, load, &catalog, first_roots);
            let selected = selected_tests(&lowered, suites, &only);
            weighed = true;
            baseline_note = crate::observe::may_observe_baseline(&lowered, &selected);
            baseline = baseline_note.is_some();
            if baseline {
                // Back to the apps as they were; a file not read yet is read when the compile that
                // follows needs it.
                let _ = filled;
                apps = std::mem::take(&mut ws.apps);
                kinds = kinds_again;
                retire((ws, lowered));
            } else {
                compile_cache_skipped = use_cache;
                compiled = Some((ws, lowered));
                apps = Vec::new();
                kinds = Vec::new();
            }
        }
        let (ws, mut lowered, cache_report) = match compiled {
            Some((ws, lowered)) => (ws, lowered, None),
            None => match opts.compile_cache.as_ref().filter(|_| use_cache) {
                Some(cc) => {
                    let (ws, lowered, report) =
                        compile_with_cache(cc, apps, kinds, manifests, symbols, target, &lazy, &identities, roots)?;
                    if try_skip && !weighed {
                        let selected = selected_tests(&lowered, suites, &only);
                        baseline_note = crate::observe::may_observe_baseline(&lowered, &selected);
                        baseline = baseline_note.is_some();
                    }
                    (ws, lowered, report)
                }
                None => {
                    fill_all(&mut apps);
                    let started = std::time::Instant::now();
                    let ws = al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, Vec::new());
                    let load = started.elapsed();
                    // Without a weighed answer, weigh on the compiled program, which has the baseline's
                    // roots: the rule only looks at what the tests reach.
                    let (ws, lowered) = al_sema::lower::compile_loaded(ws, load, &catalog, roots);
                    if try_skip && !weighed {
                        let selected = selected_tests(&lowered, suites, &only);
                        baseline_note = crate::observe::may_observe_baseline(&lowered, &selected);
                        baseline = baseline_note.is_some();
                    }
                    (ws, lowered, None)
                }
            },
        };
        let opts_only = only;
        let source_index = source_index.or(cache_report.as_ref().and_then(|c| c.source_index));
        // A dependency file that does not parse takes every object in it out of the compile, and the
        // tests that need one are refused as "outside the workspace". Say so, once, with the first lines.
        // A file whose second parse gave another result is a defect, not a property of the file (issue 15).
        for flake in ws.parse_flakes() {
            eprintln!("parse: WARNING {flake}");
        }
        let mut unparsed = ws.lazy_parse_problems();
        // The files the cache supplies were parsed when it was built.
        if let Some(c) = &cache_report {
            unparsed.extend(c.parse_problems.iter().cloned());
            unparsed.sort();
            unparsed.dedup();
        }
        if !unparsed.is_empty() {
            eprintln!(
                "parse: {} dependency file(s) did not parse; their objects are unusable and the tests that need them are not run",
                unparsed.len()
            );
            for (_, why) in unparsed.iter().take(10) {
                eprintln!("parse: {why}");
            }
            if unparsed.len() > 10 {
                eprintln!("parse: ... {} more", unparsed.len() - 10);
            }
        }
        // Code the AL compiler rejects (must-not-compile corpus, `alcodes.rs`). Said, not acted on: the
        // run goes on, so an error found here that BC would not give shows in the output, not as a refusal.
        if !lowered.al_diagnostics.is_empty() {
            eprintln!(
                "compile: {} error(s) the AL compiler would give for the workspace's code",
                lowered.al_diagnostics.len()
            );
            for d in lowered.al_diagnostics.iter().take(100) {
                eprintln!("compile: error {} {}: {}", d.code, d.file, d.message);
            }
        }
        if std::env::var_os("AL_REACH_REPORT").is_some() {
            eprintln!("{}", crate::reach::report(&ws, &lowered));
        }
        // Nothing else holds the program yet, so this does not copy it.
        for (app, m) in std::sync::Arc::make_mut(&mut lowered.program)
            .apps
            .iter_mut()
            .zip(manifests)
        {
            app.publisher = m.publisher.clone();
            app.version = m.version.clone();
            app.resources = m.resources.clone();
            app.include_source = m.include_source;
        }
        // The source lines of an app that includes its source in the symbols (the Code Coverage `Line`).
        {
            let program = std::sync::Arc::make_mut(&mut lowered.program);
            for sf in &mut program.sources {
                let Some(app) = program.apps.get(sf.app as usize).filter(|a| a.include_source) else {
                    continue;
                };
                let _ = app;
                if let Some((_, text)) = ws
                    .apps
                    .get(sf.app as usize)
                    .and_then(|a| a.files.iter().find(|(p, _)| *p == sf.path))
                {
                    sf.lines = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
                }
            }
        }
        let blocked: Vec<Vec<Option<String>>> = lowered
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let limit = opts.test_time_limit;
        let workers = if opts.trace { 1 } else { test_workers() };
        let baseline_skipped = baseline_wanted && !baseline;
        let engine_started = std::time::Instant::now();
        let make_engine = || {
            let mut engine = Engine::with_config(
                lowered.program.clone(),
                blocked.clone(),
                target.cloned(),
                dotnet.clone(),
            );
            global_triggers_find_unavailable(&mut engine, &lowered);
            if let Some(e) = Arc::get_mut(&mut engine) {
                e.test_data = opts.test_data.clone();
            }
            engine
        };
        let mut engine = make_engine();
        let engine_time = engine_started.elapsed();
        let compile_time = compile_started.elapsed();
        let started = std::time::Instant::now();
        let baseline_run = if baseline {
            crate::baseline::run(&engine, &lowered)
        } else {
            None
        };
        // The tests run against an engine that knows which tables the install baseline could not
        // seed. The baseline itself ran before it knew; with several test threads, which each run
        // the baseline again (its data is not for sharing between threads), the engine that ran it
        // stays as it was and the tests get another.
        let unseeded: std::collections::HashSet<usize> = baseline_run
            .as_ref()
            .map(|b| b.unseeded.iter().map(|&t| t as usize).collect())
            .unwrap_or_default();
        let baseline_engine = (workers > 1 && baseline_run.is_some()).then(|| engine.clone());
        if let Some(b) = &baseline_run {
            for n in &b.notes {
                eprintln!("baseline: {n}");
            }
            for l in b.restored_lines() {
                eprintln!("baseline: {l}");
            }
            eprintln!(
                "baseline: {} of {} install roots ran to their end; {} tables hold rows, {} are unseeded, {:.2} s",
                b.ran,
                b.total,
                b.filled,
                b.unseeded.len(),
                started.elapsed().as_secs_f64()
            );
            if baseline_engine.is_some() {
                engine = make_engine();
            }
            if let Some(e) = Arc::get_mut(&mut engine) {
                e.unseeded = unseeded;
            }
        }
        let baseline_time = started.elapsed();
        let baseline = baseline_run.map(|b| b.baseline);
        let tests_started = std::time::Instant::now();
        let mut outcomes = std::collections::HashMap::new();
        let mut marks_by_test: std::collections::HashMap<(usize, u32, String), Vec<Assumption>> =
            std::collections::HashMap::new();
        let mut times_by_case: std::collections::HashMap<(usize, u32, String), std::time::Duration> =
            Default::default();
        let mut details = std::collections::HashMap::new();
        let mut panics = 0;
        let mut codeunit_times: Vec<CodeunitTime> = Vec::new();
        // The test codeunits to run, in the order the report lists them.
        let mut units: Vec<Unit> = Vec::new();
        let mut list: Vec<TestResult> = Vec::new();
        for (si, suite) in suites.iter().enumerate() {
            let isolation = match suite.runner {
                None => Ok(TestIsolation::default()),
                Some(r) => runner_isolation(&lowered, r),
            };
            for (oi, o) in lowered.program.objects.iter().enumerate() {
                let is_test = is_selected_test(&lowered, oi, suite, &opts_only);
                let Some(id) = o.id.filter(|_| is_test) else { continue };
                match &isolation {
                    Ok(i) => units.push(Unit {
                        si,
                        oi,
                        id,
                        isolation: opts.isolation.unwrap_or(*i),
                        without_baseline: false,
                        coverage: opts.coverage,
                    }),
                    Err(why) => {
                        for r in test_routines(o) {
                            outcomes.insert((si, id, r.name.to_lowercase()), Outcome::NotRun(why.clone()));
                            list.push(TestResult {
                                suite: si,
                                app: order[o.app as usize],
                                codeunit: id,
                                codeunit_name: o.name.clone(),
                                test: r.name.clone(),
                                outcome: Outcome::NotRun(why.clone()),
                                call_stack: String::new(),
                                assumptions: Vec::new(),
                                time: std::time::Duration::ZERO,
                                coverage: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        // AL_CHECK_BASELINE_RULE=1: the verification of `observe.rs`. The baseline runs as usual, but
        // each test codeunit the rule calls free of it starts on an empty store. A run compared with a
        // normal one must show no outcome that differs (`tools/baseline_rule_check.sh`).
        if std::env::var_os("AL_CHECK_BASELINE_RULE").is_some() && baseline.is_some() {
            let list = std::env::var("AL_CHECK_BASELINE_RULE").is_ok_and(|v| v == "list");
            for u in &mut units {
                let why = crate::observe::may_observe_baseline(&lowered, &[u.oi]);
                if let (true, Some(why)) = (list, &why) {
                    eprintln!(
                        "baseline rule check: {} may observe it: {why}",
                        lowered.program.objects[u.oi].name
                    );
                }
                u.without_baseline = why.is_none();
            }
            eprintln!(
                "baseline rule check: {} of {} test codeunits run without the baseline",
                units.iter().filter(|u| u.without_baseline).count(),
                units.len()
            );
        }
        let results: Vec<UnitResult> = if workers <= 1 || units.len() < 2 {
            units
                .iter()
                .map(|u| {
                    let mut observe = |name: &str, _: &Outcome, seen: &Observed| {
                        let mut d = explain::detail(
                            &lowered.program,
                            seen.session.fault_site(),
                            seen.error.map(|e| explain::ErrorInfo {
                                message: e.message.clone(),
                                code: e.code.clone(),
                                call_stack: e.call_stack.clone(),
                            }),
                        );
                        d.refused_in = seen
                            .refused_in
                            .map(|(o, r)| explain::describe(&lowered.program, o, r, al_ir::NO_POS));
                        details.insert((u.si, u.id, name.to_lowercase()), d);
                    };
                    let trace: Option<&mut Observer> = opts.trace.then_some(&mut observe);
                    run_unit(&engine, &lowered, baseline.as_ref(), u, trace, limit)
                })
                .collect()
        } else {
            // Test codeunits run on a session each and do not share anything (the install
            // baseline is put back for each), so worker threads take them in turn. A worker
            // sets up its own copy of the baseline.
            let next = std::sync::atomic::AtomicUsize::new(0);
            let pull = |baseline: Option<&al_vm::Baseline>| {
                let mut done = Vec::new();
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(u) = units.get(k) else { return done };
                    done.push(run_unit(&engine, &lowered, baseline, u, None, limit));
                }
            };
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for _ in 1..workers.min(units.len()) {
                    let spawned = std::thread::Builder::new()
                        .stack_size(TEST_STACK)
                        .spawn_scoped(scope, || {
                            let own = baseline_engine
                                .as_ref()
                                .and_then(|e| crate::baseline::run(e, &lowered))
                                .map(|b| b.baseline);
                            pull(own.as_ref())
                        });
                    match spawned {
                        Ok(h) => handles.push(h),
                        // No room for another thread (an address space limit): fewer will do.
                        Err(_) => break,
                    }
                }
                let mut done = pull(baseline.as_ref());
                for h in handles {
                    done.extend(h.join().expect("a test worker panicked"));
                }
                done
            })
        };
        let mut suite_work: Vec<(usize, std::time::Duration)> = Vec::new();
        for r in results {
            suite_work.push((r.si, r.took));
            codeunit_times.push(CodeunitTime {
                id: r.id,
                name: lowered.program.objects[r.oi].name.clone(),
                took: r.took,
                sidecar_start: r.sidecar_start,
            });
            let o = &lowered.program.objects[r.oi];
            let push = |list: &mut Vec<TestResult>,
                        test: &str,
                        outcome: &Outcome,
                        call_stack: String,
                        assumptions: Vec<Assumption>,
                        time: std::time::Duration,
                        coverage: Vec<(u32, u32)>| {
                list.push(TestResult {
                    suite: r.si,
                    app: order[o.app as usize],
                    codeunit: r.id,
                    codeunit_name: o.name.clone(),
                    test: test.to_string(),
                    outcome: outcome.clone(),
                    call_stack,
                    assumptions,
                    time,
                    coverage,
                })
            };
            match r.result {
                Ok((done, stacks, times, hits, mut covs)) => {
                    for (name, outcome) in done {
                        let stack = stacks.iter().find(|(n, _)| *n == name).map(|(_, s)| s.clone());
                        let marks = hits.iter().find(|(n, _)| *n == name).map(|(_, h)| h.clone());
                        let marks = marks.unwrap_or_default();
                        if !marks.is_empty() {
                            marks_by_test.insert((r.si, r.id, name.to_lowercase()), marks.clone());
                        }
                        let time = times
                            .iter()
                            .find(|(n, _)| *n == name)
                            .map_or(std::time::Duration::ZERO, |(_, d)| *d);
                        let cov = covs
                            .iter()
                            .position(|(n, _)| *n == name)
                            .map(|i| std::mem::take(&mut covs[i].1))
                            .unwrap_or_default();
                        push(&mut list, &name, &outcome, stack.unwrap_or_default(), marks, time, cov);
                        times_by_case.insert((r.si, r.id, name.to_lowercase()), time);
                        outcomes.insert((r.si, r.id, name.to_lowercase()), outcome);
                    }
                }
                Err(msg) => {
                    panics += 1;
                    for t in test_routines(&lowered.program.objects[r.oi]) {
                        let outcome = Outcome::NotRun(format!("runtime panic (bug): {msg}"));
                        push(
                            &mut list,
                            &t.name,
                            &outcome,
                            String::new(),
                            Vec::new(),
                            std::time::Duration::ZERO,
                            Vec::new(),
                        );
                        outcomes.insert((r.si, r.id, t.name.to_lowercase()), outcome);
                    }
                }
            }
        }
        let tests_time = tests_started.elapsed();
        // Test codeunits of different runs may overlap in time: a run's time is what its codeunits took.
        let suite_times: Vec<(String, std::time::Duration)> = suites
            .iter()
            .enumerate()
            .map(|(si, suite)| {
                (
                    suite.name.clone(),
                    suite_work.iter().filter(|(i, _)| *i == si).map(|(_, d)| *d).sum(),
                )
            })
            .collect();
        codeunit_times.sort_by_key(|c| std::cmp::Reverse(c.took));
        codeunit_times.truncate(SLOWEST);
        let suites = suites.iter().map(|s| s.name.clone()).collect();
        // The workspace and the lowered program are big: dropping them takes as long as lowering
        // a tenth of the corpus, and nothing waits for it.
        let compile_parts = lowered.timings;
        let coverage_statements = if opts.coverage {
            crate::coverage::statements(&lowered.program, |oi| !lowered.dependency[oi], |a| order[a])
        } else {
            Vec::new()
        };
        retire((ws, lowered));
        Ok(CorpusRuntime {
            outcomes,
            times: times_by_case,
            suites,
            panics,
            phases: Phases {
                compile: compile_time,
                baseline: baseline_time,
                tests: tests_time,
                suites: suite_times,
                compile_parts,
                cache: cache_report,
                engine: engine_time,
                baseline_skipped,
                baseline_why: baseline_note,
                compile_cache_skipped,
                source_index,
                slowest: codeunit_times,
            },
            details,
            results: list,
            marks: marks_by_test,
            coverage_statements,
        })
    }
}

impl CorpusRuntime {
    /// Every test the run decided, with its codeunit's name and app, codeunit by codeunit.
    pub fn results(&self) -> &[TestResult] {
        &self.results
    }

    /// With `RunOptions::coverage`: every statement of the workspace's apps (the position ids that
    /// `TestResult::coverage` counts).
    pub fn coverage_statements(&self) -> &[crate::coverage::CoverageStatement] {
        &self.coverage_statements
    }

    /// Where `case` of the run named `suite` was refused or failed; only with `RunOptions::trace`.
    pub fn detail_in(&self, suite: &str, case: &CaseId) -> Option<&TestDetail> {
        let si = self.suites.iter().position(|s| s == suite)?;
        self.details.get(&(si, case.codeunit, case.name.to_lowercase()))
    }

    /// The unprobed assumptions `case` ran through in the run named `suite`; empty for a clean test.
    pub fn assumptions_in(&self, suite: &str, case: &CaseId) -> &[Assumption] {
        self.suites
            .iter()
            .position(|s| s == suite)
            .and_then(|si| self.marks.get(&(si, case.codeunit, case.name.to_lowercase())))
            .map_or(&[], Vec::as_slice)
    }

    /// How long `case` took in the first run.
    pub fn first_time(&self, case: &CaseId) -> std::time::Duration {
        self.times
            .get(&(0, case.codeunit, case.name.to_lowercase()))
            .copied()
            .unwrap_or_default()
    }

    /// How long `case` took in suite `suite` (zero for a test that was not started).
    pub fn time_in(&self, suite: &str, case: &CaseId) -> std::time::Duration {
        self.suites
            .iter()
            .position(|s| s == suite)
            .and_then(|si| self.times.get(&(si, case.codeunit, case.name.to_lowercase())))
            .copied()
            .unwrap_or_default()
    }

    /// The outcome of `case` in the run named `suite` (the first run has the empty name).
    pub fn outcome_in(&self, suite: &str, case: &CaseId) -> Outcome {
        self.suites
            .iter()
            .position(|s| s == suite)
            .and_then(|si| self.outcomes.get(&(si, case.codeunit, case.name.to_lowercase())))
            .cloned()
            .unwrap_or_else(|| Outcome::NotRun("test procedure not found in the compiled corpus".into()))
    }
}

impl CorpusRuntime {
    /// The outcome of `case` in the first run (the main corpus app, or the only run).
    pub fn first_outcome(&self, case: &CaseId) -> Outcome {
        self.outcomes
            .get(&(0, case.codeunit, case.name.to_lowercase()))
            .cloned()
            .unwrap_or_else(|| Outcome::NotRun("test procedure not found in the compiled corpus".into()))
    }
}

/// The index of the lazily read dependency apps from the cache folder, as the `LazyDep`s a workspace
/// loads from (by app; `None` for an app that has its text). `Err(key)`: there is none that fits, and
/// `key` is where to save one.
fn lazy_index(
    cc: &crate::compile_cache::CompileCache,
    apps: &[al_sema::workspace::AppSource],
    manifests: &[Manifest],
    lazy: &[Option<al_sema::LazySource>],
    identities: &[Option<[u8; 32]>],
) -> Result<Vec<Option<al_sema::LazyDep>>, Box<al_cache::Key>> {
    let which: Vec<usize> = (0..apps.len()).filter(|&i| lazy[i].is_some()).collect();
    let refs: Vec<&al_sema::workspace::AppSource> = which.iter().map(|&i| &apps[i]).collect();
    let mans: Vec<Manifest> = which
        .iter()
        .map(|&i| manifests.get(i).cloned().unwrap_or_default())
        .collect();
    let ids: Vec<Option<[u8; 32]>> = which.iter().map(|&i| identities[i]).collect();
    let key = crate::compile_cache::index_key(&refs, &mans, &ids);
    let fits = |index: &Vec<Vec<al_sema::FileIndex>>| {
        index.len() == which.len() && index.iter().zip(&which).all(|(f, &i)| f.len() == apps[i].files.len())
    };
    match crate::compile_cache::read_index(&cc.dir, &key) {
        Some(index) if fits(&index) => {
            let mut index = index.into_iter();
            Ok((0..apps.len())
                .map(|i| {
                    lazy[i].as_ref().map(|l| al_sema::LazyDep {
                        index: index.next().expect("one index per lazy app"),
                        loader: l.loader.clone(),
                    })
                })
                .collect())
        }
        _ => Err(Box::new(key)),
    }
}

/// Saves the scan of the lazily read apps of `ws` (loaded with their text) as the index for `key`.
fn save_index(
    cc: &crate::compile_cache::CompileCache,
    ws: &al_sema::Workspace,
    lazy: &[Option<al_sema::LazySource>],
    key: &al_cache::Key,
) {
    let mut all = ws.export_index();
    let index: Vec<Vec<al_sema::FileIndex>> = (0..lazy.len())
        .filter(|&i| lazy[i].is_some())
        .map(|i| std::mem::take(&mut all[i]))
        .collect();
    crate::compile_cache::write_index(&cc.dir, key, &index);
}

/// The compile of a run with the compile cache (ADR-022): the key of the dependency apps (the
/// first `kinds` entries), the entry from the cache folder or built now, then the cached compile.
/// An entry that does not fit or is damaged is ignored (and removed), and the run compiles without
/// it; with `require` it is an error instead.
#[allow(clippy::too_many_arguments)]
fn compile_with_cache(
    cc: &crate::compile_cache::CompileCache,
    mut apps: Vec<al_sema::workspace::AppSource>,
    kinds: Vec<al_sema::AppKind>,
    manifests: &[Manifest],
    symbols: &BTreeSet<String>,
    target: Option<&al_version::Target>,
    lazy: &[Option<al_sema::LazySource>],
    identities: &[Option<[u8; 32]>],
    roots: al_sema::check::Roots,
) -> Result<(al_sema::Workspace, Lowered, Option<crate::compile_cache::CacheReport>), String> {
    use crate::compile_cache as cache;
    use al_sema::provided::DepSource;
    let n_dep = kinds.iter().take_while(|k| **k == al_sema::AppKind::Dependency).count();
    let dep_symbols: Vec<Option<BTreeSet<String>>> = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (*k == al_sema::AppKind::Dependency).then(|| {
                manifests
                    .get(i)
                    .map(|m| m.symbols.iter().cloned().collect())
                    .unwrap_or_default()
            })
        })
        .collect();
    // The text of a lazily read app is read when something needs it: building an entry does, a hit
    // does not.
    let filled = std::cell::Cell::new(false);
    let fill = |apps: &mut [al_sema::workspace::AppSource]| {
        if !filled.replace(true) {
            for (a, l) in apps.iter_mut().zip(lazy) {
                if let Some(l) = l {
                    l.fill(a);
                }
            }
        }
    };
    let (layers, mut report) = cache::open_or_build_with(
        cc,
        &mut apps[..n_dep],
        &manifests[..n_dep],
        target,
        &identities[..n_dep.min(identities.len())],
        &mut |a| fill(a),
    )?;
    // `open_or_build_with` checked that the layers fit each other.
    let stack = layers.stack().map_err(|why| format!("compile cache: {why}"))?;
    report.objects = DepSource::len(&stack);
    report.parse_problems = layers.parse_problems();
    let catalog = match target {
        Some(t) => al_builtins::catalog::Catalog::for_target(t),
        None => al_builtins::catalog::Catalog::load(),
    };
    let started = std::time::Instant::now();
    // A lazily read app is loaded from its index when there is one: its sources are neither read nor
    // scanned. Otherwise they are read now (once), scanned, and the index is saved.
    let ws = if lazy.iter().any(Option::is_some) {
        match lazy_index(cc, &apps, manifests, lazy, identities) {
            Ok(deps) => {
                report.source_index = Some("read");
                al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, deps)
            }
            Err(key) => {
                fill(&mut apps);
                let ws = al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, Vec::new());
                save_index(cc, &ws, lazy, &key);
                report.source_index = Some("built");
                ws
            }
        }
    } else {
        al_sema::Workspace::load_apps_symbols(apps, kinds, symbols, &dep_symbols, Vec::new())
    };
    let load = started.elapsed();
    match al_sema::lower::compile_loaded_cached(ws, load, &catalog, roots.clone(), &stack) {
        Ok((ws, lowered)) => {
            report.relowered = (0..lowered.provided.len())
                .filter(|&i| lowered.dependency[i] && !lowered.provided[i])
                .count();
            Ok((ws, lowered, Some(report)))
        }
        Err(failed) => {
            let (ws, why) = *failed;
            let note = format!("the entry is not used: {why}");
            eprintln!("compile cache: {note}");
            report.notes.push(note);
            report.hit = false;
            for l in &report.layers {
                let _ = std::fs::remove_file(&l.file);
            }
            if cc.require {
                return Err(format!("--require-compile-cache: {}", report.notes.join("; ")));
            }
            let (ws, lowered) = al_sema::lower::compile_loaded(ws, load, &catalog, roots);
            Ok((ws, lowered, Some(report)))
        }
    }
}

/// Finds the subscribers of the platform codeunit `Global Triggers` in dependency apps that cannot
/// run (Base Application's `GlobalTriggerManagement` reaches platform tables that have no row
/// provider yet, and `Database::User`) and tells the engine, which refuses to raise an event one
/// of them would receive (`dispatch.rs`). Delete this when nothing is unavailable.
pub fn global_triggers_find_unavailable(engine: &mut Arc<Engine>, lowered: &Lowered) {
    let key = engine.events.global_key;
    let mut unavailable = std::collections::HashMap::new();
    for ((object, event), subs) in &engine.events.subscribers {
        if *object != key {
            continue;
        }
        for s in subs {
            if !lowered.dependency[s.object] {
                continue;
            }
            let Some(why) = blocked_reason(lowered, s.object, s.routine) else {
                continue;
            };
            // A subscriber of an operation nothing used was never compiled; it is never asked.
            if why.contains("not compiled") {
                continue;
            }
            // What its decision reads: record variables of the routines within two calls.
            let mut config = BTreeSet::new();
            let mut level = vec![(s.object, s.routine)];
            for _ in 0..3 {
                let mut next = Vec::new();
                for (o, r) in level {
                    for slot in &lowered.program.objects[o].routines[r].locals {
                        if let al_builtins::value::RtType::Record(t, false) = &slot.ty {
                            config.insert(*t);
                        }
                    }
                    lowered.calls_into(o, r, &mut next);
                }
                level = next;
            }
            let _ = event;
            unavailable.insert(
                (s.object, s.routine),
                al_vm::events::UnavailableGlobal {
                    why,
                    config: config.into_iter().collect(),
                },
            );
        }
    }
    if !unavailable.is_empty() {
        eprintln!(
            "Global Triggers: {} dependency subscriber(s) cannot run; a write that would reach one is not run",
            unavailable.len()
        );
    }
    if let Some(e) = Arc::get_mut(engine) {
        e.events.unavailable_global = unavailable;
    }
}

/// Outcomes of every `[Test]` procedure of test codeunit `object`, in order.
pub fn run_codeunit(engine: &Arc<Engine>, lowered: &Lowered, object: usize) -> Vec<(String, Outcome)> {
    run_codeunit_from(engine, lowered, object, None)
}

/// [`run_codeunit`] on a session that starts from the install baseline (ADR-013 decision 5), under
/// `TestIsolation = Codeunit`: the isolation every corpus oracle was recorded under.
pub fn run_codeunit_from(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    object: usize,
    baseline: Option<&al_vm::Baseline>,
) -> Vec<(String, Outcome)> {
    run_codeunit_isolated(engine, lowered, object, baseline, TestIsolation::Codeunit)
}

/// One isolation unit (see `isolation.rs`): a test codeunit run to its end on a session of its own.
pub fn run_codeunit_isolated(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    object: usize,
    baseline: Option<&al_vm::Baseline>,
    isolation: TestIsolation,
) -> Vec<(String, Outcome)> {
    run_codeunit_observed(engine, lowered, object, baseline, isolation, None)
}

/// What an observer sees of a test that has just ended.
pub struct Observed<'a> {
    /// The session, with its fault site (tracing is on when there is an observer).
    pub session: &'a Session,
    /// The error that failed the test.
    pub error: Option<&'a AlError>,
    /// The routine a refusal decided before the run belongs to (object, routine).
    pub refused_in: Option<(usize, usize)>,
}

/// Called with the test's name and outcome after each test of the codeunit that ran.
pub type Observer<'a> = dyn FnMut(&str, &Outcome, &Observed) + 'a;

/// [`run_codeunit_isolated`] that tells `observe` how each test ended and turns on the session's
/// fault tracing for it (`--explain`).
pub fn run_codeunit_observed(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    object: usize,
    baseline: Option<&al_vm::Baseline>,
    isolation: TestIsolation,
    observe: Option<&mut Observer>,
) -> Vec<(String, Outcome)> {
    run_codeunit_full(
        engine,
        lowered,
        object,
        baseline,
        isolation,
        observe,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        None,
    )
}

/// [`run_codeunit_isolated`] where a test that runs longer than `limit` is stopped and not run.
pub fn run_codeunit_limited(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    object: usize,
    baseline: Option<&al_vm::Baseline>,
    isolation: TestIsolation,
    limit: Option<std::time::Duration>,
) -> Vec<(String, Outcome)> {
    run_codeunit_full(
        engine,
        lowered,
        object,
        baseline,
        isolation,
        None,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        limit,
    )
}

/// The frames BC's test runner has below a test, as `SessionInformation.Callstack` shows them
/// (corpus session/TestSessionInformationCallstackContracts.al:89-97 and the failure texts of the
/// corpus CI on 27.5, 28.5 and 29.0, where the lines are the same). The runner's own app is
/// `Test Runner by Microsoft`; its version is the loaded app's, else the run's BC version.
fn runner_frames(program: &al_ir::Program, engine: &Engine) -> String {
    let version = program
        .apps
        .iter()
        .find(|a| a.name == "Test Runner" && a.publisher == "Microsoft")
        .map(|a| a.version.clone())
        .or_else(|| {
            engine
                .target
                .as_ref()
                .map(|t| format!("{}.{}.0.0", t.bc.major, t.bc.minor))
        })
        .unwrap_or_else(|| "0.0.0.0".into());
    const FRAMES: [&str; 6] = [
        "\"Test Runner - Mgt\"(CodeUnit 130454).RunTests line 26",
        "\"Test Runner - Isol. Codeunit\"(CodeUnit 130450).OnRun(Trigger) line 4",
        "\"Test Suite Mgt.\"(CodeUnit 130456).RunTests line 2",
        "\"Test Suite Mgt.\"(CodeUnit 130456).RunSelectedTests line 35",
        "\"Test Suite Mgt.\"(CodeUnit 130456).RunNextTest line 20",
        "\"Command Line Test Tool\"(Page 130455).\"RunNextTest - OnAction\"(Trigger) line 7",
    ];
    FRAMES
        .iter()
        .map(|f| format!("{f} - Test Runner by Microsoft version {version}\\"))
        .collect()
}

/// [`run_codeunit_observed`] that also collects, for each test that failed with an AL error, the AL
/// call stack of that error as `(test name, stack)` (`GetLastErrorCallStack` form, innermost first).
#[allow(clippy::too_many_arguments)]
pub fn run_codeunit_full(
    engine: &Arc<Engine>,
    lowered: &Lowered,
    object: usize,
    baseline: Option<&al_vm::Baseline>,
    isolation: TestIsolation,
    mut observe: Option<&mut Observer>,
    stacks: &mut Vec<(String, String)>,
    times: &mut Vec<(String, std::time::Duration)>,
    hits: &mut Vec<(String, Vec<Assumption>)>,
    limit: Option<std::time::Duration>,
) -> Vec<(String, Outcome)> {
    let obj = &lowered.program.objects[object];
    let mut session = Session::with_host(engine.clone(), crate::host::system_host());
    if let Some(b) = baseline {
        session.restore_baseline(b);
    }
    session.trace_faults(observe.is_some());
    let covering = crate::coverage::enabled();
    if covering {
        session.enable_coverage();
    }
    let instance = session.instance(object);
    let tests: Vec<usize> = (0..obj.routines.len())
        .filter(|&r| {
            obj.routines[r]
                .attributes
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case("Test"))
        })
        .collect();
    let all = |o: Outcome| {
        tests
            .iter()
            .map(|&r| (obj.routines[r].name.clone(), o.clone()))
            .collect::<Vec<_>>()
    };
    // Nothing pins what a runner with TestIsolation = Disabled leaves behind, within a codeunit or
    // for the next one (docs/probes-needed.md), so it is refused (hard rule 4).
    if isolation == TestIsolation::Disabled {
        return all(Outcome::NotRun(
            "TestIsolation = Disabled: what the runner leaves between tests and test codeunits is not pinned".into(),
        ));
    }
    // A [TransactionModel] other than AutoCommit, AutoRollback or None is refused.
    let mut models = std::collections::HashMap::new();
    for &r in &tests {
        for a in obj.routines[r]
            .attributes
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case("TransactionModel"))
        {
            match al_vm::TxModel::from_attribute(&a.text) {
                Some(m) => {
                    models.insert(r, m);
                }
                None => return all(Outcome::NotRun(format!("{}: unknown transaction model", a.text))),
            }
        }
    }
    // What publishing the app does first: run its install codeunits, then commit.
    session.set_install_scope(true);
    let skipped_installs = run_install_codeunits(lowered, &mut session);
    session.set_install_scope(false);
    session.in_test_codeunit = true;
    if let Some(on_run) = (0..obj.routines.len())
        .find(|&r| obj.routines[r].is_trigger && obj.routines[r].name.eq_ignore_ascii_case("OnRun"))
    {
        if let Some(reason) = blocked_reason(lowered, object, on_run) {
            return all(Outcome::NotRun(format!("OnRun: {reason}")));
        }
        match session.call(object, on_run, instance.clone(), Vec::new()) {
            // NavTestCodeunit.DoRunAsync commits after a declared OnRun
            // (codeunit/TestTestCodeunitOnRun.al).
            Ok(_) => session.commit_transaction(),
            Err(Signal::Unsupported(r)) => return all(Outcome::NotRun(format!("OnRun: {r}"))),
            Err(Signal::Error(e)) => {
                return all(Outcome::NotRun(format!(
                    "OnRun failed (runner behaviour not pinned): {}",
                    e.message
                )));
            }
            Err(_) => {}
        }
    }
    // Function isolation returns the database to its state after OnRun when each test ends
    // (corpus isolation probe T5 and T6; oracle/28/xunit-isolation-function.xml).
    let checkpoint = (isolation == TestIsolation::Function).then(|| Checkpoint::take(&mut session));
    let mut out = Vec::new();
    // Set by the first test that was refused while it ran: it stopped half way, so what it left in
    // the codeunit's AL state (globals, single-instance codeunits) is not what BC would have left.
    let mut refused_mid_run: Option<String> = None;
    for r in tests {
        let test_started = std::time::Instant::now();
        let routine = &obj.routines[r];
        let handlers = resolve_handlers(lowered, object, r);
        let mut committed = false;
        let mut failure: Option<AlError> = None;
        let mut refused_in = None;
        session.clear_fault_site();
        al_builtins::assumptions::clear_hits();
        let outcome = if let Err(reason) = &handlers {
            Outcome::NotRun(reason.clone())
        } else if let Some(b) = blocked_site(lowered, object, r) {
            refused_in = Some((b.object, b.routine));
            Outcome::NotRun(b.text)
        } else if let Some(reason) = skipped_installs
            .iter()
            .find(|(tables, _)| !tables.is_disjoint(&tables_touched(lowered, object, r)))
            .map(|(_, why)| why)
        {
            Outcome::NotRun(reason.clone())
        } else {
            let bound = handlers.unwrap_or_default();
            match bind_handlers(&mut session, &instance, &bound) {
                Err(reason) => Outcome::NotRun(reason),
                Ok(()) => {
                    session.last_error = Default::default();
                    al_builtins::assumptions::clear_hits();
                    // The AL test runner enables every application area before each test method
                    // (corpus session/TestSessionApplicationAreaTestBoundary.al:67552).
                    session.start_test_method();
                    let model = models.get(&r).copied().unwrap_or_default();
                    // TransactionModel::None is handled before the method body: every active
                    // transaction is ended on the way in (codeunit/TestWriteTxTestBoundary.al:
                    // Test08). Nothing is pending here, because every earlier test ended its
                    // transaction at its own boundary, so which way it ends is not observable.
                    if model == al_vm::TxModel::None {
                        session.commit_transaction();
                    }
                    session.tx_model = model;
                    let commits_before = session.commits;
                    session.clear_stop();
                    let watchdog = limit.map(|l| Watchdog::start(session.stop_flag(), l));
                    session.set_runner_frames(runner_frames(&lowered.program, engine));
                    let result = session.call(object, r, instance.clone(), Vec::new());
                    session.set_runner_frames(String::new());
                    let timed_out = watchdog.is_some_and(Watchdog::finish);
                    // The reason names the limit; the VM only knows that it was stopped.
                    let result = match result {
                        Err(Signal::Unsupported(_)) if timed_out => Err(Signal::Unsupported(format!(
                            "the test used more than the time limit of {} s of CPU time and was stopped (--test-timeout-secs); BC may have ended it differently or not at all",
                            limit.map_or(0, |l| l.as_secs())
                        ))),
                        r => r,
                    };
                    committed = session.commits != commits_before;
                    session.tx_model = al_vm::TxModel::AutoCommit;
                    let unexecuted = session.end_ui_scope();
                    match result {
                        // AutoRollback rolls the test's work back the moment it finishes, pass
                        // or fail (record/TestTransactionModelAutoRollback.al:Test01-Test03).
                        Ok(_) if unexecuted.is_empty() && model == al_vm::TxModel::AutoRollback => {
                            session.rollback_transaction();
                            Outcome::Pass
                        }
                        // The default TransactionModel commits at the test boundary; a failed
                        // test's transaction is rolled back (corpus codeunit/TestWriteTxTestBoundary.al:26).
                        Ok(_) if unexecuted.is_empty() => {
                            session.commit_transaction();
                            Outcome::Pass
                        }
                        // corpus handlers/TestPageQueryCloseMessageHandler_Tests.al:51: a declared
                        // handler that never fires is a test failure. The text after the prefix
                        // is not pinned (docs/probes-needed.md).
                        Ok(_) => {
                            session.rollback_transaction();
                            Outcome::Fail(format!(
                                "The following UI handlers were not executed: {}",
                                unexecuted.join(", ")
                            ))
                        }
                        Err(Signal::Error(e)) => {
                            session.rollback_transaction();
                            let message = e.message.clone();
                            failure = Some(e);
                            Outcome::Fail(message)
                        }
                        // A refused test did not run: what it wrote before the refusal must not
                        // reach the next test of the codeunit.
                        Err(Signal::Unsupported(r)) => {
                            session.rollback_transaction();
                            refused_mid_run.get_or_insert_with(|| format!("{}: {r}", routine.name));
                            Outcome::NotRun(r)
                        }
                        Err(other) => Outcome::Fail(format!("unexpected signal {}", other.describe())),
                    }
                }
            }
        };
        // A failure after a test that was refused mid-run is not trusted: the test may have set a
        // global (an "initialized" flag, a binding) whose setup was rolled back with it, and the
        // codeunit's tests share that state (TestIsolation keeps the instance). Passes stay.
        let outcome = match (outcome, &refused_mid_run) {
            (Outcome::Fail(m), Some(first)) => Outcome::NotRun(format!(
                "an earlier test of this codeunit was refused while it ran ({first}), so its shared state is not BC's; this failure is not trusted: {m}"
            )),
            (o, _) => o,
        };
        // Whether a Commit() inside a test is kept or ignored under Function isolation is not
        // pinned (docs/probes-needed.md): a test that passed through one is not reported as run.
        let outcome = match outcome {
            Outcome::Pass if committed && checkpoint.is_some() => Outcome::NotRun(
                "Commit inside a test under TestIsolation = Function: what the runner keeps is not pinned".into(),
            ),
            o => o,
        };
        if let Some(observe) = observe.as_mut() {
            observe(
                &routine.name,
                &outcome,
                &Observed {
                    session: &session,
                    error: failure.as_ref(),
                    refused_in,
                },
            );
        }
        if let Some(cp) = &checkpoint {
            cp.restore(&mut session);
        }
        if let (Outcome::Fail(_), Some(e)) = (&outcome, &failure) {
            if !e.call_stack.is_empty() {
                stacks.push((routine.name.clone(), e.call_stack.clone()));
            }
        }
        let marks = al_builtins::assumptions::take_hits();
        if !marks.is_empty() {
            hits.push((routine.name.clone(), marks));
        }
        times.push((routine.name.clone(), test_started.elapsed()));
        if covering {
            crate::coverage::push(&routine.name, session.take_coverage());
        }
        out.push((routine.name.clone(), outcome));
    }
    out
}

/// Sets a session's stop flag when a test has used more than its limit of CPU time. The time the
/// test thread computes is counted, not the wall time, so the load of the machine cannot stop a
/// test (an outcome must not depend on it). A wall-clock backstop, `WALL_BACKSTOP` times the limit,
/// still ends a test that hangs without computing. One thread per test that is run; it polls
/// until the limit or until `finish`. Where the thread's CPU clock cannot be read, the wall clock
/// counts.
struct Watchdog {
    done: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<bool>,
}

/// How many times the limit the wall clock may run before a test that does not compute is stopped.
const WALL_BACKSTOP: u32 = 10;
/// How often the watchdog reads the CPU clock.
const WATCHDOG_POLL: std::time::Duration = std::time::Duration::from_millis(50);

impl Watchdog {
    /// Watches the calling thread.
    fn start(flag: Arc<std::sync::atomic::AtomicBool>, limit: std::time::Duration) -> Watchdog {
        let clock = crate::cpuclock::ThreadClock::current();
        // The thread's CPU clock counts from the thread's birth, and a worker thread runs many tests:
        // the limit is measured from the reading at the start of this test.
        let base = clock.as_ref().and_then(|c| c.read()).unwrap_or_default();
        let (done, rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let stop = |flag: &std::sync::atomic::AtomicBool| {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            };
            loop {
                match rx.recv_timeout(WATCHDOG_POLL) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return false,
                }
                let used = clock
                    .as_ref()
                    .and_then(|c| c.read())
                    .map(|cpu| cpu.saturating_sub(base));
                match used {
                    Some(cpu) => {
                        if cpu >= limit || started.elapsed() >= limit * WALL_BACKSTOP {
                            return stop(&flag);
                        }
                    }
                    // No CPU clock: the wall clock counts.
                    None => {
                        if started.elapsed() >= limit {
                            return stop(&flag);
                        }
                    }
                }
            }
        });
        Watchdog { done, thread }
    }

    /// Ends the watch; true when the limit passed first.
    fn finish(self) -> bool {
        let _ = self.done.send(());
        self.thread.join().unwrap_or(false)
    }
}

/// The handler functions test routine `test` of `object` declares with
/// `[HandlerFunctions('A,B')]`, as (kind, routine index), or why the test
/// cannot run with them. Handlers whose kind is not implemented are refused
/// here, by name and kind (hard rule 4).
fn resolve_handlers(lowered: &Lowered, object: usize, test: usize) -> Result<Vec<(UiKind, usize)>, String> {
    let obj = &lowered.program.objects[object];
    let mut out: Vec<(UiKind, usize)> = Vec::new();
    for attr in obj.routines[test]
        .attributes
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case("HandlerFunctions"))
    {
        let names: Vec<String> = match attr.args.as_slice() {
            [al_ir::Const::Text(list)] => list.split(',').map(|n| n.trim().to_string()).collect(),
            _ => return Err(format!("{} is not a list of handler names", attr.text)),
        };
        for name in names {
            let Some(hr) = obj.routines.iter().position(|r| r.name.eq_ignore_ascii_case(&name)) else {
                return Err(format!("handler function {name} not found in {}", obj.name));
            };
            if out.iter().any(|(_, r)| *r == hr) {
                continue;
            }
            let routine = &obj.routines[hr];
            let kind_attr =
                routine.attributes.iter().map(|a| a.name.as_str()).find(|n| {
                    n.to_ascii_lowercase().ends_with("handler") && !n.eq_ignore_ascii_case("HandlerFunctions")
                });
            let Some(kind_attr) = kind_attr else {
                return Err(format!(
                    "{} is named in [HandlerFunctions] but is no handler function",
                    routine.name
                ));
            };
            let Some(kind) = UiKind::from_attribute(kind_attr) else {
                let why = match kind_attr.to_ascii_lowercase().as_str() {
                    "filterpagehandler" => {
                        "no corpus test pins how FilterPageBuilder.RunModal reaches a filter page handler"
                    }
                    _ => "this handler kind",
                };
                return Err(format!(
                    "handler {} is a [{kind_attr}]: {why}, which is not implemented yet",
                    routine.name
                ));
            };
            let page_of = |r: &al_ir::Routine| match r.locals.first().map(|l| &l.ty) {
                Some(al_builtins::value::RtType::TestPage(p)) => Some(*p),
                _ => None,
            };
            let is_page = matches!(kind, UiKind::Page | UiKind::ModalPage | UiKind::RequestPage);
            if let Some((_, other)) = out
                .iter()
                .find(|(k, r)| *k == kind && (!is_page || page_of(&obj.routines[*r]) == page_of(routine)))
            {
                return Err(format!(
                    "{} and {} are both {kind:?} handlers; which one BC would bind is not pinned",
                    obj.routines[*other].name, routine.name
                ));
            }
            out.push((kind, hr));
        }
    }
    // Code reachable from a handler must be runnable too (decided before running).
    for (_, hr) in &out {
        if let Some(reason) = blocked_reason(lowered, object, *hr) {
            return Err(format!("handler {}: {reason}", obj.routines[*hr].name));
        }
    }
    Ok(out)
}

/// Binds the resolved handlers to the test codeunit's instance for one test method.
fn bind_handlers(
    session: &mut Session,
    instance: &std::rc::Rc<std::cell::RefCell<al_builtins::value::Instance>>,
    handlers: &[(UiKind, usize)],
) -> Result<(), String> {
    let object = instance.borrow().object as usize;
    let mut bound = Vec::new();
    for (kind, routine) in handlers {
        bound.push(session.ui_handler(*kind, object, *routine, instance.clone())?);
    }
    session.begin_ui_scope(bound);
    Ok(())
}

/// Runs every `Subtype = Install` codeunit's OnInstallAppPerCompany, then every
/// OnInstallAppPerDatabase with no company chosen (the order measured on BC 27.5), and commits
/// after each. An installer that cannot run is skipped; returned are the tables it touches and
/// why, so tests that touch them can be reported not run.
fn run_install_codeunits(lowered: &Lowered, session: &mut Session) -> Vec<(BTreeSet<u32>, String)> {
    let mut skipped = Vec::new();
    let installers: Vec<usize> = lowered
        .program
        .objects
        .iter()
        .enumerate()
        .filter(|(oi, o)| {
            o.kind == "codeunit"
                && !lowered.dependency[*oi]
                && o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Install"))
        })
        .map(|(oi, _)| oi)
        .collect();
    // Every OnInstallAppPerCompany first, then every OnInstallAppPerDatabase, which has no company
    // (measured on real BC 27.5: docs/probes-needed.md, the same order as the dependency apps').
    let company = session.set_company("");
    session.set_company(&company);
    for (trigger, no_company) in [("OnInstallAppPerCompany", false), ("OnInstallAppPerDatabase", true)] {
        for &oi in &installers {
            let o = &lowered.program.objects[oi];
            let inst = session.instance(oi);
            let Some(ri) = o
                .routines
                .iter()
                .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case(trigger))
            else {
                continue;
            };
            if no_company {
                session.set_company("");
            }
            let why = match blocked_reason(lowered, oi, ri) {
                Some(reason) => Some(reason),
                None => match session.call(oi, ri, inst.clone(), Vec::new()) {
                    Ok(_) => None,
                    Err(Signal::Unsupported(r)) => Some(r),
                    Err(Signal::Error(e)) => Some(format!("failed (behaviour not pinned): {}", e.message)),
                    Err(_) => None,
                },
            };
            session.set_company(&company);
            // A trigger that ran is durable before the next one starts: a later trigger that
            // cannot run must not take its writes with it.
            if why.is_none() {
                al_builtins::native::Ctx::commit(session).expect("commit");
            }
            if let Some(why) = why {
                session.rollback_transaction();
                // Subscribers to events the installer raises are not in its static calls,
                // but they are what it would have written through.
                let mut touched = tables_touched(lowered, oi, ri);
                for (so, sr) in subscribers_reached(&session.engine.clone(), lowered, oi, ri) {
                    touched.extend(tables_touched(lowered, so, sr));
                }
                skipped.push((touched, format!("install codeunit {} could not run: {why}", o.name)));
            }
        }
    }
    session.commit_transaction();
    skipped
}

/// Tables whose records appear in the slots of anything `routine` can reach.
pub(crate) fn tables_touched(lowered: &Lowered, object: usize, routine: usize) -> BTreeSet<u32> {
    fn record_tables(t: &al_builtins::value::RtType, out: &mut BTreeSet<u32>) {
        if let al_builtins::value::RtType::Record(id, _) = t {
            out.insert(*id);
        }
    }
    let mut out = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        let obj = &lowered.program.objects[o];
        for s in obj.routines[r].locals.iter().chain(&obj.globals) {
            record_tables(&s.ty, &mut out);
        }
        lowered.calls_into(o, r, &mut stack);
    }
    out
}

/// Subscriber routines (object, routine) that `routine` of `object` can set off
/// through static calls: those of the events it publishes, and those of the table
/// events of every table whose records it uses -- followed through the routines
/// those subscribers call in turn.
pub(crate) fn subscribers_reached(
    engine: &Engine,
    lowered: &Lowered,
    object: usize,
    routine: usize,
) -> Vec<(usize, usize)> {
    let objs = &lowered.program.objects;
    let mut seen = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        lowered.calls_into(o, r, &mut stack);
        let mut keys = Vec::new();
        if let Some(p) = engine.events.publishers.get(&(o, r)) {
            keys.push((p.object, p.event.clone()));
        }
        // Table events fire for any table whose record the routine uses.
        let mut tables = BTreeSet::new();
        for s in objs[o].routines[r].locals.iter().chain(&objs[o].globals) {
            if let al_builtins::value::RtType::Record(t, _) = &s.ty {
                tables.insert(*t as usize);
            }
        }
        keys.extend(
            engine
                .events
                .subscribers
                .keys()
                .filter(|(po, _)| tables.contains(po))
                .cloned(),
        );
        for k in keys {
            for sub in engine.events.subscribers.get(&k).into_iter().flatten() {
                if reached.insert((sub.object, sub.routine)) {
                    stack.push((sub.object, sub.routine));
                }
            }
        }
    }
    reached.into_iter().collect()
}

/// Why routine `routine` of `object` cannot run: the first reason found in
/// anything it can reach through static calls (hard rule 4: decided before running).
pub fn blocked_reason(lowered: &Lowered, object: usize, routine: usize) -> Option<String> {
    blocked_site(lowered, object, routine).map(|b| b.text)
}

/// A routine that cannot run, found from a root by [`blocked_site`].
pub struct Blocked {
    /// The routine that carries the lowering reason.
    pub object: usize,
    pub routine: usize,
    /// `Object.Routine: reason`.
    pub text: String,
}

/// [`blocked_reason`] with the routine the reason was found in.
pub fn blocked_site(lowered: &Lowered, object: usize, routine: usize) -> Option<Blocked> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![(object, routine)];
    // `AL_BLOCK_PATH=1` prints the chain of calls from the root to the routine with the reason.
    let trace = std::env::var_os("AL_BLOCK_PATH").is_some();
    let mut parent = std::collections::BTreeMap::new();
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        if let Some(reason) = lowered.unsupported[o][r].first() {
            let name = &lowered.program.objects[o].routines[r].name;
            let obj = &lowered.program.objects[o].name;
            if trace {
                let mut at = (o, r);
                let mut chain = vec![at];
                while let Some(&p) = parent.get(&at) {
                    chain.push(p);
                    at = p;
                }
                for (co, cr) in chain.iter().rev() {
                    let ob = &lowered.program.objects[*co];
                    eprintln!("  via {} {} .{}", ob.kind, ob.name, ob.routines[*cr].name);
                }
            }
            return Some(Blocked {
                object: o,
                routine: r,
                text: format!("{obj}.{name}: {reason}"),
            });
        }
        let before = stack.len();
        lowered.calls_into(o, r, &mut stack);
        if trace {
            for &next in &stack[before..] {
                parent.entry(next).or_insert((o, r));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_sema::workspace::AppSource;

    fn outcomes(src: &str) -> Vec<(String, Outcome)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l
            .program
            .objects
            .iter()
            .position(|o| {
                o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.eq_ignore_ascii_case("Test"))
            })
            .unwrap();
        run_codeunit(&engine, &l, oi)
    }

    const SRC: &str = "codeunit 60100 T
{
    Subtype = Test;
    var
        Seen: Integer;

    trigger OnRun()
    begin
        Seen := 41;
    end;

    [Test]
    procedure A_Passes()
    begin
        Seen += 1;
        if Seen <> 42 then Error('OnRun did not run first: %1', Seen);
    end;

    [Test]
    procedure B_Fails()
    begin
        Error('expected failure');
    end;

    [Test]
    procedure C_NotRun()
    begin
        Helper();
    end;

    local procedure Helper()
    begin
        MinimumActiveRowVersion();
    end;

    procedure NotATest()
    begin
        Error('must not run');
    end;

    [Test]
    [HandlerFunctions('MsgHandler')]
    procedure D_NeedsHandler()
    begin
    end;

    [MessageHandler]
    procedure MsgHandler(Msg: Text)
    begin
    end;
}";

    #[test]
    fn runs_tests_in_order_after_onrun_on_one_instance() {
        let o = outcomes(SRC);
        let names: Vec<_> = o.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["A_Passes", "B_Fails", "C_NotRun", "D_NeedsHandler"]);
        assert_eq!(o[0].1, Outcome::Pass);
        assert_eq!(o[1].1, Outcome::Fail("expected failure".into()));
    }

    #[test]
    fn unreachable_code_does_not_block_but_reachable_unsupported_code_does() {
        let o = outcomes(SRC);
        assert!(
            matches!(&o[2].1, Outcome::NotRun(r) if r.contains("MinimumActiveRowVersion")),
            "{:?}",
            o[2].1
        );
    }

    fn outcomes_of(src: &str, codeunit: &str) -> Vec<(String, Outcome)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l.program.objects.iter().position(|o| o.name == codeunit).unwrap();
        run_codeunit(&engine, &l, oi)
    }

    fn with_rows(body: &str) -> String {
        format!("{ROWS}\n{body}")
    }

    const ROWS: &str = "table 60900 S { fields { field(1; K; Code[10]) { } } }
        codeunit 60910 Inserter { trigger OnRun() var R: Record S; begin R.K := 'RUN'; R.Insert(); end; }";

    #[test]
    fn a_failure_after_a_test_that_was_refused_mid_run_is_not_trusted() {
        // A refused test stops half way: it may have set globals the next tests rely on (an
        // "initialized" flag whose setup was rolled back). A failure that follows is then
        // not BC's answer, and is reported as not run; passes are kept, and a refusal before
        // the test body starts (a static block) leaves the state alone.
        let src = "table 60901 T9 { fields { field(1; K; Code[10]) { } } }
            codeunit 60103 T { Subtype = Test; var Flag: Boolean;
                [Test] procedure T1() var R: Record T9; begin Flag := true; R.LockTable(false); end;
                [Test] procedure T2() begin if not Flag then Error('flag lost'); end;
                [Test] procedure T3() begin Error('real failure'); end; }
            codeunit 60104 U { Subtype = Test;
                [Test] procedure U1() begin Error('real failure'); end; }";
        let o = outcomes_of(src, "T");
        let by = |n: &str| o.iter().find(|(x, _)| x == n).map(|(_, x)| x.clone()).unwrap();
        assert!(matches!(by("T1"), Outcome::NotRun(_)), "{:?}", by("T1"));
        assert_eq!(by("T2"), Outcome::Pass);
        assert!(
            matches!(by("T3"), Outcome::NotRun(r) if r.contains("earlier test") && r.contains("LockTable")),
            "{:?}",
            by("T3")
        );
        let u = outcomes_of(src, "U");
        assert_eq!(u[0].1, Outcome::Fail("real failure".into()));
    }

    // record/TestTransactionModelAutoRollback.al:Test01-Test03: an AutoRollback test's write is
    // rolled back when it finishes; the next default-model test's write survives.
    #[test]
    fn an_auto_rollback_test_is_rolled_back_when_it_finishes() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure T1() var R: Record S; begin
                    R.K := 'A'; R.Insert(); if not R.Get('A') then Error('own write not visible'); end;
                [Test] procedure T2() var R: Record S; begin
                    if R.Get('A') then Error('the AutoRollback write survived');
                    R.K := 'B'; R.Insert(); end;
                [Test] procedure T3() var R: Record S; begin
                    if not R.Get('B') then Error('the default model write did not survive'); end; }",
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 3],
            "{o:?}"
        );
    }

    // record/TestTransactionModelAutoRollback.al:Test04-Test06
    #[test]
    fn auto_rollback_refuses_commit_anywhere_beneath_the_test_and_auto_commit_allows_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Direct() begin
                    asserterror Commit();
                    if GetLastErrorText() <> 'Tests cannot call the Commit function if TransactionModel property is set to AutoRollback.' then
                        Error('wrong text: %1', GetLastErrorText()); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Callee() begin
                    asserterror Down();
                    if StrPos(GetLastErrorText(), 'Tests cannot call the Commit function') = 0 then
                        Error('wrong text: %1', GetLastErrorText()); end;
                [Test] [TransactionModel(TransactionModel::AutoCommit)] procedure AutoCommitAllows() var R: Record S; begin
                    R.K := 'C'; R.Insert(); Commit(); asserterror Error('x');
                    if not R.Get('C') then Error('the commit did not stick'); R.Delete(); Commit(); end;
                local procedure Down() begin Commit(); end; }"
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 3],
            "{o:?}"
        );
    }

    // record/TestTransactionModelAutoRollback.al:Test07 and Test08
    #[test]
    fn commit_behavior_ignore_exempts_the_auto_rollback_refusal_and_error_does_not_outrank_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure Ignored() var R: Record S; begin
                    InsertAndCommitIgnoring();
                    asserterror Error('unrelated');
                    if R.Get('I') then Error('an ignored Commit moved the boundary'); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure ErrorBehavior() begin
                    asserterror CommitUnderError();
                    if StrPos(GetLastErrorText(), 'Tests cannot call the Commit function') = 0 then
                        Error('wrong text: %1', GetLastErrorText());
                    if StrPos(GetLastErrorText(), 'Commit is prohibited in the current scope') <> 0 then
                        Error('the CommitBehavior text won'); end;
                [CommitBehavior(CommitBehavior::Ignore)]
                local procedure InsertAndCommitIgnoring() var R: Record S; begin R.K := 'I'; R.Insert(); Commit(); end;
                [CommitBehavior(CommitBehavior::Error)]
                local procedure CommitUnderError() begin Commit(); end; }",
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 2],
            "{o:?}"
        );
    }

    // codeunit/TestWriteTxTestBoundary.al:Test02-Test06: no write transaction crosses a test
    // boundary, and a guarded Codeunit.Run is allowed at the start of the next test.
    #[test]
    fn no_write_transaction_survives_a_test_boundary() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure RollbackWrites() var R: Record S; begin
                    R.K := 'A'; R.Insert();
                    if not Database.IsInWriteTransaction() then Error('no write transaction after an insert'); end;
                [Test] [TransactionModel(TransactionModel::AutoRollback)] procedure NextStartsClean() var Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('inherited a write transaction');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not Ok then Error('guarded run refused'); end;
                [Test] procedure DefaultWrites() var R: Record S; begin
                    // The guarded run of NextStartsClean committed its row 'RUN': it survives an
                    // AutoRollback test (codeunit/TestCodeunitRunErrorTextsAndModels.al:259,
                    // AutoRollback_Test04_SuccessfulGuardedRun_RowSurvivesTheTestBoundary).
                    R.DeleteAll();
                    R.K := 'D'; R.Insert();
                    if not Database.IsInWriteTransaction() then Error('no write transaction after an insert'); end;
                [Test] procedure GuardedRunAfterDefaultWrite() var R: Record S; Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('the default test left a write transaction');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not (Ok and R.Get('D')) then Error('guarded run refused or D lost'); end; }"
        ));
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 4],
            "{o:?}"
        );
    }

    // codeunit/TestWriteTxTestBoundary.al:Test08-Test10: a None test starts with no
    // transaction, cannot write directly, and can write inside a Codeunit.Run.
    #[test]
    fn a_none_test_has_no_transaction_until_it_runs_a_codeunit() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] procedure Default() var R: Record S; begin R.K := 'D'; R.Insert(); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure StartsClean() var R: Record S; Ok: Boolean; begin
                    if Database.IsInWriteTransaction() then Error('a None test started inside a write transaction');
                    if not R.Get('D') then Error('the previous test row is gone');
                    Ok := Codeunit.Run(Codeunit::Inserter);
                    if not Ok then Error('guarded run refused'); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure CannotWrite() var R: Record S; begin
                    R.K := 'N';
                    asserterror R.Insert();
                    if GetLastErrorText() <> 'A transaction must be started before changes can be made to the database.' then
                        Error('wrong text: %1', GetLastErrorText());
                    if Database.IsInWriteTransaction() then Error('a refused write left a transaction open');
                    if R.Get('N') then Error('the refused insert wrote'); end;
                [Test] [TransactionModel(TransactionModel::None)] procedure DeleteAllUnderNone() var R: Record S; begin
                    R.DeleteAll();
                    end; }"
        ));
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
        assert_eq!(o[2].1, Outcome::Pass, "{o:?}");
        // DeleteAll under None is refused with the Insert text
        // (codeunit/TestWriteTxNoneModelContracts.al:91, NoneModel_Test04_DeleteAll_IsRefused).
        assert_eq!(
            o[3].1,
            Outcome::Fail("A transaction must be started before changes can be made to the database.".into()),
            "{o:?}"
        );
    }

    // codeunit/TestWriteTxTestBoundary.al:Test10_ARunCodeunitMayWriteUnderNone
    #[test]
    fn a_codeunit_run_from_a_none_test_may_write_and_its_transaction_ends_with_it() {
        let o = outcomes(&with_rows(
            "            codeunit 60102 T { Subtype = Test;
                [Test] [TransactionModel(TransactionModel::None)] procedure Run() var R: Record S; begin
                    Codeunit.Run(Codeunit::Inserter);
                    if not R.Get('RUN') then Error('the run did not write');
                    if Database.IsInWriteTransaction() then Error('the run left a transaction open'); end; }",
        ));
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
    }

    // codeunit/TestCodeunitRunNestedTestCodeunit.al: Codeunit.Run of a test codeunit from a
    // running test raises, guarded or not, in the static and the instance form, and runs nothing.
    #[test]
    fn running_a_test_codeunit_from_a_test_is_refused() {
        let o = outcomes_of(
            "codeunit 60201 Inner { Subtype = Test;
                [Test] procedure InnerTest() begin Error('the inner test ran'); end; }
            codeunit 60202 Plain { trigger OnRun() begin end; }
            codeunit 60203 Outer { Subtype = Test;
                [Test] procedure Unguarded() begin
                    asserterror Codeunit.Run(Codeunit::Inner);
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText());
                    if StrPos(GetLastErrorText(), 'Test codeunit 60201 Inner was called from another test codeunit.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure Guarded() var Ok: Boolean; begin
                    Ok := true;
                    asserterror Ok := Codeunit.Run(Codeunit::Inner);
                    if not Ok then Error('a result was assigned');
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure Instance() var I: Codeunit Inner; begin
                    asserterror I.Run();
                    if StrPos(GetLastErrorText(), 'You cannot nest the execution of test codeunits.') = 0 then Error('wrong text: %1', GetLastErrorText()); end;
                [Test] procedure PlainStillRuns() var Ok: Boolean; begin
                    Ok := Codeunit.Run(Codeunit::Plain);
                    if not Ok then Error('plain codeunit failed'); end; }",
            "Outer",
        );
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 4],
            "{o:?}"
        );
    }

    // The default TransactionModel commits a test's work at the test boundary (corpus
    // codeunit/TestWriteTxTestBoundary.al:26; record/TestCalcFormulaFlowFieldValueTests.Codeunit.al:182,
    // whose asserterror rolls back to "whatever the previous test committed").
    #[test]
    fn a_passing_test_commits_so_a_later_asserterror_rolls_back_only_to_that_point() {
        let o = outcomes(
            "table 60900 S { fields { field(1; K; Code[10]) { } } }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure A() var R: Record S; begin R.K := 'A'; R.Insert(); end;
                [Test] procedure B() var R: Record S; begin
                    R.K := 'B'; R.Insert();
                    asserterror Error('x');
                    if not R.Get('A') then Error('the first test was not committed');
                    if R.Get('B') then Error('the second tests own write was not rolled back');
                end; }",
        );
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
    }

    #[test]
    fn install_codeunits_run_before_the_tests() {
        // OnInstallAppPerDatabase has no company, so it writes a DataPerCompany = false table.
        let o = outcomes("table 60900 S { DataPerCompany = false; fields { field(1; K; Code[10]) { } } }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var R: Record S; begin R.K := 'CO'; R.Insert(); end;
                trigger OnInstallAppPerDatabase() var R: Record S; begin R.K := 'DB'; R.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure A() var R: Record S; begin if R.Count() <> 2 then Error('install did not run'); end; }");
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
    }

    // install/TestInstallSeeder.al: OnInstallAppPerDatabase seeds one table, OnInstallAppPerCompany
    // is one the runtime cannot run. The per-database seed ran and stays; only the trigger that
    // could not run is skipped, with its tables.
    #[test]
    fn an_installer_trigger_that_ran_keeps_its_rows_when_the_next_one_cannot_run() {
        let o = outcomes(
            "table 60900 Db { DataPerCompany = false; fields { field(1; K; Code[10]) { } } }
            table 60901 Co { fields { field(1; K; Code[10]) { } } }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerDatabase() var R: Record Db; begin R.K := 'DB'; R.Insert(); end;
                trigger OnInstallAppPerCompany() var R: Record Co; begin MinimumActiveRowVersion(); R.K := 'CO'; R.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure SeededByTheTriggerThatRan() var R: Record Db; begin if not R.Get('DB') then Error('the per-database seed was rolled back'); end;
                [Test] procedure SeededByTheTriggerThatDidNot() var R: Record Co; begin if R.Count() = 0 then Error('unseeded'); end; }",
        );
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
        assert!(matches!(&o[1].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
    }

    #[test]
    fn a_blocked_installer_also_blocks_tests_on_tables_its_event_subscribers_touch() {
        // install/TestInstallEventDispatch_*.al: the installer raises an event whose
        // subscriber seeds a table the installer never names; skipping the installer
        // must not let tests on that table run against an unseeded store.
        let o = outcomes(
            "table 60900 Seed { fields { field(1; K; Code[10]) { } } }
            codeunit 60904 Pub { [IntegrationEvent(false, false)] procedure OnSeed() begin end; }
            codeunit 60905 Sub {
                [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnSeed', '', false, false)]
                local procedure Seed() var R: Record Seed; begin R.Insert(); end; }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var P: Codeunit Pub; begin MinimumActiveRowVersion(); P.OnSeed(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesSeed() var R: Record Seed; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
    }

    #[test]
    fn a_blocked_installer_also_blocks_tests_on_tables_written_by_table_event_subscribers() {
        // The installer writes to Src, whose Insert event subscriber seeds Seed: skipping the
        // installer must not let tests on Seed run against an unseeded store.
        let o = outcomes(
            "table 60900 Seed { fields { field(1; K; Code[10]) { } } }
            table 60903 Src { fields { field(1; K; Code[10]) { } } }
            codeunit 60905 Sub {
                [EventSubscriber(ObjectType::Table, Database::Src, 'OnAfterInsertEvent', '', false, false)]
                local procedure Seed(var Rec: Record Src) var R: Record Seed; begin R.K := 'X'; R.Insert(); end; }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var S: Record Src; begin MinimumActiveRowVersion(); S.K := 'A'; S.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesSeed() var R: Record Seed; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
    }

    #[test]
    fn a_blocked_installer_only_blocks_tests_that_touch_its_tables() {
        let o = outcomes(
            "table 60900 S { fields { field(1; K; Code[10]) { } } }
            table 60903 Other { fields { field(1; K; Code[10]) { } } }
            codeunit 60901 Inst { Subtype = Install;
                trigger OnInstallAppPerCompany() var R: Record S; begin MinimumActiveRowVersion(); R.Insert(); end; }
            codeunit 60102 T { Subtype = Test;
                [Test] procedure UsesS() var R: Record S; begin end;
                [Test] procedure UsesOther() var R: Record Other; begin end; }",
        );
        assert!(matches!(&o[0].1, Outcome::NotRun(r) if r.contains("install")), "{o:?}");
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
    }

    // ---- test handler functions (docs/probes-needed.md lists what is unpinned) ----

    fn outcome_of(o: &[(String, Outcome)], name: &str) -> Outcome {
        o.iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name}: {o:?}"))
            .1
            .clone()
    }

    const UI: &str = "codeunit 60110 UiT { Subtype = Test;
        var Seen: Text; Fired: Integer; Asked: Text;

        // handlers/TestMessageHandler.al:15 Message_Show_HandlerCaptures
        [Test] [HandlerFunctions('Msg')]
        procedure MessageReachesHandler() begin Message('Hello'); if Seen <> 'Hello' then Error('got %1', Seen); end;

        // handlers/TestMessageHandler.al:23 Message_FormattedString_HandlerGetsFormatted
        [Test] [HandlerFunctions('Msg')]
        procedure MessageArgsAreSubstitutedBeforeTheHandler() begin Message('Value: %1', 42); if Seen <> 'Value: 42' then Error('got %1', Seen); end;

        // handlers/TestMessageHandler.al:31,41 Confirm_Question_HandlerRepliesTrue / False
        [Test] [HandlerFunctions('Yes')]
        procedure ConfirmHandlerRepliesTrue() begin if not Confirm('Sure?') then Error('false'); end;
        [Test] [HandlerFunctions('No')]
        procedure ConfirmHandlerRepliesFalse() begin if Confirm('Sure?', true) then Error('true'); end;

        // handlers/TestConfirmFormatArgs.al:Confirm_SingleFormatArg_HandlerReceivesRawUnsubstitutedQuestion
        [Test] [HandlerFunctions('No')]
        procedure ConfirmHandlerGetsTheRawQuestion() begin if Confirm('%1 items. Go?', true, 2) then; if Asked <> '%1 items. Go?' then Error('got %1', Asked); end;

        // handlers/TestConfirmFormatArgs.al:Confirm_DefaultChoiceHonored_WhenHandlerDoesNotOverrideReply
        [Test] [HandlerFunctions('Silent')]
        procedure ConfirmDefaultSurvivesAHandlerThatLeavesReply()
        begin
            if not Confirm('q', true) then Error('default true lost');
            if Confirm('q', false) then Error('default false lost');
        end;

        // handlers/TestConfirmFormatArgs.al: one handler answers two Confirms
        [Test] [HandlerFunctions('No')]
        procedure OneHandlerServesManyCalls() begin Fired := 0; if Confirm('a', true) then; if Confirm('b', true) then; if Fired <> 2 then Error('fired %1', Fired); end;

        // handlers/TestConfirmNoHandler.al:Confirm_NoHandler_TestBody_IsRefused
        [Test]
        procedure ConfirmWithoutHandlerIsRefused()
        var Reply: Boolean;
        begin
            Reply := false;
            asserterror Reply := Confirm('CNH question', true);
            if StrPos(GetLastErrorText(), 'Unhandled UI') = 0 then Error('no Unhandled UI: %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'Confirm') = 0 then Error('no Confirm: %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'CNH question') = 0 then Error('no question: %1', GetLastErrorText());
            if Reply then Error('default was assigned');
        end;

        // handlers/TestPageQueryCloseError_Tests.al:75 (Unhandled UI ... Message)
        [Test]
        procedure MessageWithoutHandlerIsRefused()
        begin
            asserterror Message('lost text');
            if StrPos(GetLastErrorText(), 'Unhandled UI: Message') = 0 then Error('got %1', GetLastErrorText());
            if StrPos(GetLastErrorText(), 'lost text') = 0 then Error('text missing: %1', GetLastErrorText());
        end;

        // a Message handler does not answer a Confirm
        [Test] [HandlerFunctions('Msg')]
        procedure HandlerOfAnotherKindDoesNotAnswer()
        begin
            asserterror if Confirm('q') then;
            if StrPos(GetLastErrorText(), 'Unhandled UI: Confirm') = 0 then Error('got %1', GetLastErrorText());
            Message('now used');
        end;

        // handlers/TestPageQueryCloseMessageHandler_Tests.al:51 (UI handlers were not executed)
        [Test] [HandlerFunctions('Msg')]
        procedure DeclaredHandlerThatNeverFiresFailsTheTest() begin end;

        // an error in the test wins over the unexecuted handler
        [Test] [HandlerFunctions('Msg')]
        procedure TestErrorIsReportedBeforeUnexecutedHandlers() begin Error('own error'); end;

        // an error raised in a handler comes out of the UI call
        [Test] [HandlerFunctions('Boom')]
        procedure HandlerErrorPropagates() begin Message('x'); end;

        // handlers of the previous test are not bound any more
        [Test]
        procedure HandlersDoNotLeakIntoTheNextTest()
        begin
            asserterror Message('leak?');
            if StrPos(GetLastErrorText(), 'Unhandled UI') = 0 then Error('got %1', GetLastErrorText());
        end;

        // network/TestHttpClient.al:36 [StrMenuHandler] AllowOnceHandler
        [Test] [HandlerFunctions('Menu')]
        procedure StrMenuHandlerChoosesAnOption()
        var C: Integer;
        begin
            C := StrMenu('One,Two,Three', 1, 'Pick');
            if C <> 2 then Error('choice %1', C);
            if Asked <> 'One,Two,Three|Pick' then Error('asked %1', Asked);
        end;

        [Test]
        procedure StrMenuWithoutHandlerIsRefused()
        begin
            asserterror if StrMenu('A,B') = 0 then;
            if StrPos(GetLastErrorText(), 'Unhandled UI: StrMenu') = 0 then Error('got %1', GetLastErrorText());
        end;

        [Test] [HandlerFunctions('Nope')]
        procedure UnknownHandlerNameIsRefused() begin end;

        [Test] [HandlerFunctions('Helper')]
        procedure NamingARoutineThatIsNoHandlerIsRefused() begin end;

        [Test] [HandlerFunctions('Blocked')]
        procedure HandlerWithUnsupportedCodeIsRefused() begin Message('x'); end;

        [Test] [HandlerFunctions('Msg,Msg2')]
        procedure TwoHandlersOfOneKindAreRefused() begin end;

        [MessageHandler] procedure Msg(M: Text[1024]) begin Seen := M; end;
        [MessageHandler] procedure Msg2(M: Text[1024]) begin end;
        [MessageHandler] procedure Boom(M: Text[1024]) begin Error('boom in handler'); end;
        [MessageHandler] procedure Blocked(M: Text[1024]) begin MinimumActiveRowVersion(); end;
        [ConfirmHandler] procedure Yes(Q: Text[1024]; var Reply: Boolean) begin Reply := true; end;
        [ConfirmHandler] procedure No(Q: Text[1024]; var Reply: Boolean) begin Fired += 1; Asked := Q; Reply := false; end;
        [ConfirmHandler] procedure Silent(Q: Text[1024]; var Reply: Boolean) begin end;
        [StrMenuHandler] procedure Menu(Options: Text; var Choice: Integer; Instruction: Text) begin Asked := Options + '|' + Instruction; Choice := 2; end;
        procedure Helper() begin end;
    }";

    fn ui(name: &str) -> Outcome {
        outcome_of(&outcomes(UI), name)
    }

    #[test]
    fn message_handler_receives_the_text_and_substituted_arguments() {
        assert_eq!(ui("MessageReachesHandler"), Outcome::Pass);
        assert_eq!(ui("MessageArgsAreSubstitutedBeforeTheHandler"), Outcome::Pass);
    }

    #[test]
    fn confirm_handler_replies_and_receives_the_raw_question() {
        assert_eq!(ui("ConfirmHandlerRepliesTrue"), Outcome::Pass);
        assert_eq!(ui("ConfirmHandlerRepliesFalse"), Outcome::Pass);
        assert_eq!(ui("ConfirmHandlerGetsTheRawQuestion"), Outcome::Pass);
        assert_eq!(ui("ConfirmDefaultSurvivesAHandlerThatLeavesReply"), Outcome::Pass);
        assert_eq!(ui("OneHandlerServesManyCalls"), Outcome::Pass);
    }

    #[test]
    fn ui_without_a_bound_handler_of_its_kind_is_refused_with_unhandled_ui() {
        assert_eq!(ui("ConfirmWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("MessageWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("HandlerOfAnotherKindDoesNotAnswer"), Outcome::Pass);
        assert_eq!(ui("StrMenuWithoutHandlerIsRefused"), Outcome::Pass);
        assert_eq!(ui("HandlersDoNotLeakIntoTheNextTest"), Outcome::Pass);
    }

    #[test]
    fn a_declared_handler_that_never_fires_fails_the_test() {
        assert!(matches!(ui("DeclaredHandlerThatNeverFiresFailsTheTest"),
                Outcome::Fail(m) if m.starts_with("The following UI handlers were not executed") && m.contains("Msg")),);
        assert_eq!(
            ui("TestErrorIsReportedBeforeUnexecutedHandlers"),
            Outcome::Fail("own error".into())
        );
    }

    #[test]
    fn an_error_in_a_handler_comes_out_of_the_ui_call() {
        assert_eq!(ui("HandlerErrorPropagates"), Outcome::Fail("boom in handler".into()));
    }

    #[test]
    fn str_menu_handler_chooses() {
        assert_eq!(ui("StrMenuHandlerChoosesAnOption"), Outcome::Pass);
    }

    #[test]
    fn handlers_the_runtime_cannot_run_are_refused_explicitly() {
        let nr = |n: &str| match ui(n) {
            Outcome::NotRun(r) => r,
            other => panic!("{n}: {other:?}"),
        };
        assert!(nr("UnknownHandlerNameIsRefused").contains("Nope"));
        assert!(nr("NamingARoutineThatIsNoHandlerIsRefused").contains("Helper"));
        assert!(nr("HandlerWithUnsupportedCodeIsRefused").contains("MinimumActiveRowVersion"));
        assert!(nr("TwoHandlersOfOneKindAreRefused").contains("Msg2"));
    }

    #[test]
    fn page_report_and_other_handlers_are_refused_with_their_kind() {
        let o = outcomes(
            "page 60120 Pg { PageType = Card; }
            report 60123 Rp { }
            codeunit 60121 T { Subtype = Test;
                [Test] [HandlerFunctions('Modal')] procedure A() begin end;
                [Test] [HandlerFunctions('Req')] procedure B() begin end;
                [Test] [HandlerFunctions('Fil')] procedure C() begin end;
                [ModalPageHandler] procedure Modal(var P: TestPage Pg) begin end;
                [RequestPageHandler] procedure Req(var P: TestRequestPage Rp) begin end;
                [FilterPageHandler] procedure Fil(var F: FilterPageBuilder) begin end; }",
        );
        // Modal and request page handlers are supported; one the test never triggers is reported
        // as BC does.
        assert!(
            matches!(&o[0].1, Outcome::Fail(r) if r.contains("not executed")),
            "{o:?}"
        );
        assert!(
            matches!(&o[1].1, Outcome::Fail(r) if r.contains("not executed")),
            "{o:?}"
        );
        assert!(
            matches!(&o[2].1, Outcome::NotRun(r) if r.contains("FilterPageHandler")),
            "{o:?}"
        );
    }

    #[test]
    fn ui_outside_a_test_method_is_not_run() {
        // OnRun is not a test method: what a UI call does there is not pinned.
        let o = outcomes(
            "codeunit 60122 T { Subtype = Test;
                trigger OnRun() begin Message('x'); end;
                [Test] procedure A() begin end; }",
        );
        assert!(
            matches!(&o[0].1, Outcome::NotRun(r) if r.contains("OnRun") && r.contains("outside a test method")),
            "{o:?}"
        );
    }

    // ---- page handlers (ADR-012) ----

    const PAGES: &str = r#"
table 60700 "PH Row" { fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[50]) { } } keys { key(PK; "No.") { } } }
page 60701 "PH Card" { PageType = Card; SourceTable = "PH Row"; layout { area(Content) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } } }
page 60702 "PH Host"
{
    PageType = Card; SourceTable = "PH Row";
    layout { area(Content) { field(NoCtl; Rec."No.") { } } }
    actions { area(Processing) { action(Pick) { trigger OnAction() var M: Page "PH Card"; R: Record "PH Row"; Res: Action; begin
        R.Init(); R."No." := 'RESULT'; Res := M.RunModal(); R.Descr := Format(Res); R.Insert(); end; } } }
}
table 60706 "PH Nr" { fields { field(1; Id; Guid) { } field(2; Name; Code[50]) { } } keys { key(PK; Id) { } } }
page 60707 "PH NrCard"
{
    PageType = Card; SourceTable = "PH Nr"; DelayedInsert = true;
    layout { area(Content) {
        field(IdField; Rec.Id) { Visible = false; }
        field(NameField; Rec.Name) { trigger OnValidate() begin CurrPage.Update(); end; } } }
    trigger OnInsertRecord(BelowxRec: Boolean): Boolean begin Rec.Id := CreateGuid(); end;
    trigger OnAfterGetRecord() var T: Codeunit PhTrace; begin T.Note('AGR:' + Rec.Name); end;
    trigger OnAfterGetCurrRecord() var T: Codeunit PhTrace; begin T.Note('AGCR'); CurrPage.Update(false); end;
}
codeunit 60708 PhTrace { SingleInstance = true; var Log: Text; procedure Note(T: Text) begin Log += T + ';'; end; procedure Get(): Text begin exit(Log); end; procedure Reset() begin Log := ''; end; }
page 60709 "PH Ro" { PageType = Card; SourceTable = "PH Row"; layout { area(Content) { field(NoCtl; Rec."No.") { } field(NoteCtl; Note) { } } } trigger OnOpenPage() begin CurrPage.Editable(false); end; var Note: Text[20]; }
page 60710 "PH Desc" { PageType = List; SourceTable = "PH Row"; SourceTableView = sorting("No.") order(descending); layout { area(Content) { repeater(R) { field(NoCtl; Rec."No.") { } field(DescrCtl; Rec.Descr) { } } } } }
table 60704 "PH Lk" { LookupPageId = "PH Card"; fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[50]) { } } keys { key(PK; "No.") { } } }
page 60705 "PH Saved" { PageType = StandardDialog; SaveValues = true; layout { area(Content) { field(Rem; RemTxt) { } } } var RemTxt: Text[30]; }
page 60711 "PH Saved X" { PageType = StandardDialog; SaveValues = true; SourceTable = "PH Row"; layout { area(Content) { field(RemX; RemTxt) { } } } var RemTxt: Text[30]; }
pageextension 60712 "PH Saved XE" extends "PH Saved X" { layout { addlast(Content) { field(DescX; Rec.Descr) { } } } }
codeunit 60703 PhT
{
    Subtype = Test;
    var Seen: Text;

    // handlers/TestPageModalHandler_Tests.al:43,83 ModalPageHandlerOkReachesTheCallingAl / CancelReaches (lookup)
    [Test] [HandlerFunctions('OkModal')]
    procedure OkReachesTheCaller()
    var R: Record "PH Row"; H: TestPage "PH Host";
    begin
        R."No." := 'A'; R.Insert(); H.OpenEdit(); H.First(); H.Pick.Invoke(); H.Close();
        R.Get('RESULT'); if R.Descr <> 'OK' then Error('got %1', R.Descr);
    end;

    // handlers/TestPageModalHandler_Tests.al:105 ModalPageWithoutAHandlerIsRefused
    [Test]
    procedure NoHandlerIsRefused()
    var R: Record "PH Row"; H: TestPage "PH Host";
    begin
        R."No." := 'N1'; R.Insert(); Commit(); H.OpenEdit(); H.First();
        asserterror H.Pick.Invoke();
        if StrPos(GetLastErrorText(), 'Unhandled UI: ModalPage') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageRunHandler_Tests.al:60-90: the handler is handed the record Page.Run was given
    [Test] [HandlerFunctions('SeePage')]
    procedure PageRunHandsTheRecord()
    var R: Record "PH Row";
    begin
        R."No." := 'P1'; R.Descr := 'Alpha'; R.Insert(); R."No." := 'P2'; R.Descr := 'Beta'; R.Insert();
        R.Get('P2');
        Page.Run(Page::"PH Card", R);
        if Seen <> 'Beta' then Error('got %1', Seen);
    end;

    // testpageclose + handlers/TestPageModalQueryClose_Tests.al (b): a handler that invokes nothing leaves a plain modal reporting OK
    [Test] [HandlerFunctions('Nothing')]
    procedure UnattendedPlainModalReportsOk()
    var M: Page "PH Card"; Res: Action;
    begin
        Res := M.RunModal();
        if Format(Res) <> 'OK' then Error('got %1', Format(Res));
    end;

    // TestPageModalQueryClose_Tests.al (d)/(e): lookup mode cancels
    [Test] [HandlerFunctions('CancelIt')]
    procedure LookupCancelReportsLookupCancel()
    var M: Page "PH Card"; Res: Action;
    begin
        M.LookupMode(true);
        Res := M.RunModal();
        if Format(Res) <> 'LookupCancel' then Error('got %1', Format(Res));
    end;

    // TestPageModalQueryClose_Tests.al (b2): a plain modal has no built-in Cancel
    [Test] [HandlerFunctions('CancelIt')]
    procedure PlainModalHasNoCancel()
    var M: Page "PH Card"; Res: Action;
    begin
        asserterror Res := M.RunModal();
        if StrPos(GetLastErrorText(), 'The built-in action = Cancel is not found on the page') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageRunModalLookupPageId_Tests.al: id 0 with a record resolves LookupPageId
    [Test] [HandlerFunctions('OkModal')]
    procedure IdZeroUsesLookupPageId()
    var K: Record "PH Lk"; R: Record "PH Row"; Res: Action;
    begin
        Res := Page.RunModal(0, K);
        if Format(Res) <> 'LookupOK' then Error('got %1', Format(Res));
        asserterror Page.RunModal(0, R);
        if StrPos(GetLastErrorText(), 'You tried to invoke the Page object with the ID 0') = 0 then Error('got %1', GetLastErrorText());
    end;

    // handlers/TestPageSaveValues.al: OK stores, the next open shows it; Cancel stores nothing
    [Test] [HandlerFunctions('SaveRem')]
    procedure SaveValuesKeepsWhatOkStored()
    begin
        Seen := 'x'; Page.RunModal(Page::"PH Saved");
        Seen := ''; Page.RunModal(Page::"PH Saved");
        if Seen <> 'kept' then Error('got %1', Seen);
    end;

    // handlers/TestPageSaveValues.al as above, on a SaveValues page that an extension extends with a
    // control bound to a field of Rec (it adds no variable-bound control: docs/probes-needed.md)
    [Test] [HandlerFunctions('SaveRemX')]
    procedure SaveValuesOnAnExtendedPageKeepsWhatOkStored()
    begin
        Seen := 'x'; Page.RunModal(Page::"PH Saved X");
        Seen := ''; Page.RunModal(Page::"PH Saved X");
        if Seen <> 'kept' then Error('got %1', Seen);
    end;

    // handlers/TestPageTrapSharedScope_Tests.al:87: a page still held by the caller keeps the trap
    [Test]
    procedure TrapSurvivesWhileAnotherVariableHoldsThePage()
    var T: TestPage "PH Card"; R: Record "PH Row";
    begin
        R."No." := 'TS'; R.Insert();
        TrapAndAssign(T);
        R.Get('TS');
        Page.Run(Page::"PH Card", R);
        if T.NoCtl.Value() <> 'TS' then Error('got %1', T.NoCtl.Value());
        T.Close();
    end;

    local procedure TrapAndAssign(var Out: TestPage "PH Card")
    var A: TestPage "PH Card"; B: TestPage "PH Card";
    begin
        A.Trap(); B := A; Out := B;
    end;

    // pageupdate/TestPageUpdateNewRow.al: Update() in OnValidate saves a DelayedInsert new row, then the refresh runs AGR for it
    [Test]
    procedure UpdateSavesTheNewRowAndRefreshesIt()
    var C: TestPage "PH NrCard"; R: Record "PH Nr"; T: Codeunit PhTrace;
    begin
        T.Reset(); C.OpenNew();
        if T.Get() <> 'AGCR;' then Error('open: %1', T.Get());
        T.Reset(); C.NameField.SetValue('Nx');
        if StrPos(T.Get(), 'AGR:NX;') = 0 then Error('set: %1', T.Get());
        C.Close();
        R.SetRange(Name, 'Nx'); if R.Count() <> 1 then Error('rows %1', R.Count());
    end;

    // handlers/TestPageControlEditableRoutes.al:548: CurrPage.Editable(false) in OnOpenPage narrows every control
    [Test]
    procedure CurrPageEditableFalseInOnOpenPageNarrowsEverything()
    var C: TestPage "PH Ro"; R: Record "PH Row";
    begin
        R."No." := 'E1'; R.Insert();
        C.OpenEdit();
        if C.Editable() or C.NoCtl.Editable() or C.NoteCtl.Editable() then Error('still editable');
        C.Close();
    end;

    // pagefindrecord/TestPageFindRecord.al:192: SourceTableView sorting descending; Find*Field walks the view
    [Test]
    procedure SourceTableViewOrdersTheRowsAndFindFieldWalksThem()
    var L: TestPage "PH Desc"; R: Record "PH Row";
    begin
        R."No." := 'ZD1'; R.Insert(); R."No." := 'ZD2'; R.Insert(); R."No." := 'ZD3'; R.Insert();
        L.OpenView(); L.First();
        if L.NoCtl.Value() <> 'ZD3' then Error('first %1', L.NoCtl.Value());
        if not L.FindNextField(L.NoCtl, 'ZD1') then Error('next');
        if L.NoCtl.Value() <> 'ZD1' then Error('at %1', L.NoCtl.Value());
        if L.FindFirstField(L.NoCtl, 5) then Error('an Integer must not match a Code control');
        if L.NoCtl.Value() <> 'ZD1' then Error('a miss keeps the row, at %1', L.NoCtl.Value());
        L.Close();
    end;

    [ModalPageHandler] procedure SaveRem(var P: TestPage "PH Saved")
    begin
        if Seen = 'x' then begin P.Rem.SetValue('kept'); P.OK().Invoke(); end
        else begin Seen := P.Rem.Value(); P.Cancel().Invoke(); end;
    end;
    [ModalPageHandler] procedure SaveRemX(var P: TestPage "PH Saved X")
    begin
        if Seen = 'x' then begin P.RemX.SetValue('kept'); P.OK().Invoke(); end
        else begin Seen := P.RemX.Value(); P.Cancel().Invoke(); end;
    end;
    [ModalPageHandler] procedure OkModal(var P: TestPage "PH Card") begin P.OK().Invoke(); end;
    [ModalPageHandler] procedure Nothing(var P: TestPage "PH Card") begin end;
    [ModalPageHandler] procedure CancelIt(var P: TestPage "PH Card") begin P.Cancel().Invoke(); end;
    [PageHandler] procedure SeePage(var P: TestPage "PH Card") begin Seen := P.DescrCtl.Value(); end;
}
"#;

    fn ph(name: &str) -> Outcome {
        outcome_of(&outcomes(PAGES), name)
    }

    #[test]
    fn modal_and_page_handlers_answer_page_opens() {
        for n in [
            "OkReachesTheCaller",
            "NoHandlerIsRefused",
            "PageRunHandsTheRecord",
            "UnattendedPlainModalReportsOk",
            "LookupCancelReportsLookupCancel",
            "PlainModalHasNoCancel",
            "IdZeroUsesLookupPageId",
            "SaveValuesKeepsWhatOkStored",
            "SaveValuesOnAnExtendedPageKeepsWhatOkStored",
            "TrapSurvivesWhileAnotherVariableHoldsThePage",
            "UpdateSavesTheNewRowAndRefreshesIt",
            "CurrPageEditableFalseInOnOpenPageNarrowsEverything",
            "SourceTableViewOrdersTheRowsAndFindFieldWalksThem",
        ] {
            assert_eq!(ph(n), Outcome::Pass, "{n}");
        }
    }

    // session/TestSessionApplicationAreaTestBoundary.al:67552 (A_FirstTest_StartsWithEveryAreaEnabled_EvenAfterOnRunSetOne,
    // C_NextTest_StartsWithEveryAreaEnabled): every test method starts with every application area enabled.
    #[test]
    fn every_test_starts_with_every_application_area_enabled() {
        let src = "codeunit 60110 T
{
    Subtype = Test;
    trigger OnRun() begin ApplicationArea('#Basic,#FromOnRun'); end;

    [Test]
    procedure A_StartsEmptyEvenAfterOnRunSetOne()
    begin
        if ApplicationArea() <> '' then Error('areas: %1', ApplicationArea());
    end;

    [Test]
    procedure B_SetsAreasForTheRestOfTheTest()
    begin
        ApplicationArea('#Basic,#Probe');
        if ApplicationArea() <> '#Basic,#Probe' then Error('areas: %1', ApplicationArea());
    end;

    [Test]
    procedure C_StartsEmptyAgain()
    begin
        if ApplicationArea() <> '' then Error('areas: %1', ApplicationArea());
    end;
}";
        let o = outcomes_of(src, "T");
        for (n, r) in &o {
            assert_eq!(*r, Outcome::Pass, "{n}");
        }
        assert_eq!(o.len(), 3);
    }

    #[test]
    fn a_page_source_table_view_applies_in_filter_group_2() {
        // corpus handlers/TestPageSourceTableView.al:129-151, reproduced without a dependency app.
        let o = outcomes(
            r#"
enum 50000 "S" { value(0; Open) { } value(1; Active) { } value(2; Closed) { } }
table 50000 "K" { fields { field(1; "Entry No."; Integer) { } field(2; Status; Enum "S") { } } keys { key(PK; "Entry No.") { Clustered = true; } } }
table 50001 "Log" { fields { field(1; "Id"; Integer) { AutoIncrement = true; } field(2; "NewValue"; Text[100]) { } field(3; "OldValue"; Text[100]) { } } keys { key(PK; "Id") { Clustered = true; } } }
page 50000 "P" { PageType = List; SourceTable = "K"; SourceTableView = where(Status = const(Active));
  layout { area(Content) { repeater(Lines) { field("Entry No."; Rec."Entry No.") { } field(Status; Rec.Status) { } } } }
  trigger OnOpenPage() var L: Record "Log"; begin
    L."OldValue" := CopyStr(Rec.GetFilter(Status), 1, 100);
    Rec.FilterGroup(2);
    L."NewValue" := CopyStr(Rec.GetFilter(Status), 1, 100);
    Rec.FilterGroup(0);
    L.Insert();
  end; }
codeunit 50001 "T" { Subtype = Test;
  [Test] procedure Go() var L: Record "Log"; P: TestPage "P"; begin
    P.OpenView(); P.Close();
    L.FindLast();
    if L."NewValue" <> 'Active' then Error('new=[%1] old=[%2]', L."NewValue", L."OldValue");
  end; }
"#,
        );
        assert_eq!(o, vec![("Go".to_string(), Outcome::Pass)]);
    }

    /// Outcomes of the test codeunit of a workspace app `main` over a dependency app `dep`.
    fn outcomes_over_dependency(main: &str, dep: &str) -> Vec<(String, Outcome)> {
        let app = |id: &str, deps: &[&str], src: &str| AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files: vec![(format!("{id}.al"), src.into())],
        };
        let (_, l) = al_sema::lower::compile_apps(
            vec![app("main", &["dep"], main), app("dep", &[], dep)],
            vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
            &Default::default(),
        );
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l
            .program
            .objects
            .iter()
            .position(|o| {
                o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.eq_ignore_ascii_case("Test"))
            })
            .unwrap();
        run_codeunit(&engine, &l, oi)
    }

    const HIDDEN_FIELD_PAGE: &str = r#"
table 50000 "K" { fields { field(1; "No."; Integer) { } field(2; Name; Text[20]) { } } keys { key(PK; "No.") { Clustered = true; } } }
page 50000 "P" { PageType = List; SourceTable = "K";
  layout { area(Content) { repeater(Lines) { field("No."; Rec."No.") { } field(Name; Rec.Name) { Visible = false; } } } } }
"#;
    const SETS_HIDDEN_FIELD: &str = r#"codeunit 50001 "T" { Subtype = Test;
  [Test] procedure Go() var P: TestPage "P"; begin P.OpenView(); P.Name.SetValue('X'); end; }"#;

    #[test]
    fn a_missing_control_of_a_local_page_is_reported_by_its_generated_id() {
        let o = outcomes(&format!("{HIDDEN_FIELD_PAGE}{SETS_HIDDEN_FIELD}"));
        assert_eq!(
            o,
            vec![(
                "Go".to_string(),
                // The id is FNV-1a of "50000Name" (pages/request.rs `action_id`), checked on its own.
                Outcome::Fail("The field with ID = 1299704023 is not found on the page.".to_string())
            )]
        );
    }

    #[test]
    fn a_missing_control_of_a_dependency_page_is_named_by_the_same_generated_id() {
        // corpus pageapplicationarea/TestPageApplicationAreaPrecompiledControlRemoval.al:92 pins
        // `ID = 301776846` for `Name` of page 329: the id derives from the declaring object's id and
        // the control name alone, so a dependency page needs no special case.
        let o = outcomes_over_dependency(SETS_HIDDEN_FIELD, HIDDEN_FIELD_PAGE);
        assert_eq!(
            o,
            vec![(
                "Go".to_string(),
                Outcome::Fail("The field with ID = 1299704023 is not found on the page.".to_string())
            )]
        );
    }

    #[test]
    fn running_a_dependency_codeunit_nothing_compiled_by_its_id_is_refused_not_run_as_empty() {
        // Nothing names codeunit 50100 but the number: it is not compiled, and running it must not
        // quietly do nothing.
        let o = outcomes_over_dependency(
            "codeunit 50001 \"T\" { Subtype = Test; [Test] procedure Go() begin Codeunit.Run(50100); end; }",
            "codeunit 50100 \"Lib\" { trigger OnRun() begin Error('ran'); end; }",
        );
        let [(name, Outcome::NotRun(why))] = o.as_slice() else {
            panic!("{o:?}")
        };
        assert_eq!(name, "Go");
        assert!(why.contains("50100"), "{why}");
    }

    #[test]
    fn opening_a_dependency_table_nothing_compiled_by_its_id_is_refused() {
        let o = outcomes_over_dependency(
            "codeunit 50001 \"T\" { Subtype = Test; [Test] procedure Go() var R: RecordRef; begin R.Open(50100); end; }",
            "table 50100 \"Lib\" { fields { field(1; K; Code[10]) { } } }",
        );
        let [(_, outcome)] = o.as_slice() else { panic!("{o:?}") };
        assert!(matches!(outcome, Outcome::NotRun(_)), "{o:?}");
    }

    #[test]
    fn running_a_dependency_page_nothing_compiled_by_its_id_is_refused() {
        let o = outcomes_over_dependency(
            "codeunit 50001 \"T\" { Subtype = Test; [Test] procedure Go() begin Page.Run(50100); end; }",
            "table 50100 \"K\" { fields { field(1; \"No.\"; Integer) { } } }\n\
             page 50100 \"Lib\" { PageType = List; SourceTable = \"K\"; layout { area(Content) { repeater(Lines) { field(\"No.\"; Rec.\"No.\") { } } } } }",
        );
        let [(_, outcome)] = o.as_slice() else { panic!("{o:?}") };
        assert!(
            matches!(outcome, Outcome::NotRun(why) if why.contains("50100")),
            "{o:?}"
        );
    }

    /// The full run (install baseline included) of workspace app `main` over dependency app `dep`:
    /// the outcome of test `name` of the test codeunit `50001`.
    fn with_baseline(main: &str, dep: &str) -> impl Fn(&str) -> Outcome {
        let app = |id: &str, deps: &[&str], src: &str| AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files: vec![(format!("{id}.al"), src.into())],
        };
        let rt = CorpusRuntime::run_for_apps(
            vec![app("main", &["dep"], main), app("dep", &[], dep)],
            vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
            &[],
            &Default::default(),
            None,
            Default::default(),
        );
        move |name: &str| {
            rt.first_outcome(&CaseId {
                codeunit: 50001,
                name: name.into(),
            })
        }
    }

    const SETUP_DEP: &str = r#"
table 50100 "Setup" { fields { field(1; "Primary Key"; Code[10]) { } field(2; Value; Text[50]) { } } keys { key(PK; "Primary Key") { } } }
table 50101 "Order Log" { fields { field(1; "No."; Integer) { AutoIncrement = true; } field(2; What; Text[30]) { } } keys { key(PK; "No.") { } } }
table 50102 "Shared Log" { DataPerCompany = false; fields { field(1; "No."; Integer) { AutoIncrement = true; } field(2; What; Text[30]) { } } keys { key(PK; "No.") { } } }
codeunit 50110 "Installer A" { Subtype = Install;
  trigger OnInstallAppPerDatabase() var L: Record "Shared Log"; begin L.What := 'A-db'; L.Insert(); end;
  trigger OnInstallAppPerCompany() var L: Record "Order Log"; S: Record "Setup"; begin L.What := 'A-company'; L.Insert(); S."Primary Key" := ''; S.Value := 'installed'; S.Insert(); end; }
codeunit 50111 "Installer B" { Subtype = Install;
  trigger OnInstallAppPerCompany() var L: Record "Order Log"; begin L.What := 'B-company'; L.Insert(); end; }
codeunit 2 "Company-Initialize" {
  trigger OnRun() var S: Record "Setup"; L: Record "Order Log"; begin
    S.Get(); S.Value := S.Value + '+initialized'; S.Modify();
    L.What := 'init'; L.Insert(); end; }
codeunit 50120 "Lib" { procedure ReadSetup(): Text var S: Record "Setup"; begin S.Get(); exit(S.Value); end; }
"#;

    #[test]
    fn the_projects_own_install_codeunits_run_per_company_first_then_per_database_with_no_company() {
        // Measured on BC 27.5 (docs/probes-needed.md): every OnInstallAppPerCompany, then every
        // OnInstallAppPerDatabase, which has no company, so a per-company table is refused there.
        let o = outcomes(
            r#"
table 50101 "Shared Log" { DataPerCompany = false; fields { field(1; "No."; Integer) { AutoIncrement = true; } field(2; What; Text[30]) { } } keys { key(PK; "No.") { } } }
codeunit 50110 "Installer A" { Subtype = Install;
  trigger OnInstallAppPerDatabase() var L: Record "Shared Log"; begin L.What := 'A-db'; L.Insert(); end;
  trigger OnInstallAppPerCompany() var L: Record "Shared Log"; begin L.What := 'A-company'; L.Insert(); end; }
codeunit 50111 "Installer B" { Subtype = Install;
  trigger OnInstallAppPerCompany() var L: Record "Shared Log"; begin L.What := 'B-company'; L.Insert(); end; }
codeunit 50001 "T" { Subtype = Test;
  [Test] procedure Order() var L: Record "Shared Log"; T: Text; begin
    L.FindSet(); repeat T += L.What + ';'; until L.Next() = 0;
    if T <> 'A-company;B-company;A-db;' then Error(T); end; }
"#,
        );
        assert_eq!(o, vec![("Order".to_string(), Outcome::Pass)], "{o:?}");
    }

    /// A dependency app whose `Global Triggers` subscribers cannot run (they reach an unimplemented
    /// builtin) once the table `Cfg` they read holds a row.
    const GT_DEP: &str = r#"
table 50100 "Cfg" { fields { field(1; K; Code[10]) { } } keys { key(PK; K) { } } }
codeunit 50120 "GT Mgt" {
  [EventSubscriber(ObjectType::Codeunit, Codeunit::"Global Triggers", 'GetDatabaseTableTriggerSetup', '', false, false)]
  local procedure Setup(TableId: Integer; var OnDatabaseInsert: Boolean; var OnDatabaseModify: Boolean; var OnDatabaseDelete: Boolean; var OnDatabaseRename: Boolean) begin Check(); end;
  [EventSubscriber(ObjectType::Codeunit, Codeunit::"Global Triggers", 'OnDatabaseInsert', '', false, false)]
  local procedure Ins(RecRef: RecordRef) begin Check(); end;
  local procedure Check() var C: Record "Cfg"; begin if C.IsEmpty() then exit; MinimumActiveRowVersion(); end; }
"#;

    const GT_WATCHED: &str = r#"
table 50001 "Watched" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50003 "Watched Later" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
"#;

    #[test]
    fn global_triggers_subscribers_that_cannot_run_are_skipped_only_while_nothing_they_read_is_configured() {
        let main = format!(
            "{GT_WATCHED}codeunit 50001 \"T\" {{ Subtype = Test; \
                 [Test] procedure Plain() var W: Record \"Watched\"; begin W.K := 1; W.Insert(); end; \
                 [Test] procedure Configured() var W: Record \"Watched Later\"; C: Record \"Cfg\"; begin \
                   C.K := 'x'; C.Insert(); W.K := 2; W.Insert(); end; }}"
        );
        let outcome = with_baseline(&main, GT_DEP);
        assert_eq!(outcome("Plain"), Outcome::Pass);
        let Outcome::NotRun(why) = outcome("Configured") else {
            panic!("{:?}", outcome("Configured"))
        };
        assert!(
            why.contains("GT Mgt.Setup") && why.contains("cannot run") && why.contains("Cfg"),
            "{why}"
        );
    }

    #[test]
    fn a_database_event_that_an_unavailable_subscriber_would_receive_is_refused() {
        // The workspace's own subscriber enables OnDatabaseInsert for Watched; BC then runs the
        // dependency's subscriber too, which cannot run here.
        let main = format!(
            "{GT_WATCHED}codeunit 50002 \"Mine\" {{ \
                 [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Global Triggers\", 'GetDatabaseTableTriggerSetup', '', false, false)] \
                 local procedure S(TableId: Integer; var OnDatabaseInsert: Boolean; var OnDatabaseModify: Boolean; var OnDatabaseDelete: Boolean; var OnDatabaseRename: Boolean) begin \
                   if TableId = Database::Watched then OnDatabaseInsert := true; end; }} \
                 codeunit 50001 \"T\" {{ Subtype = Test; \
                 [Test] procedure Enabled() var W: Record \"Watched\"; begin W.K := 1; W.Insert(); end; }}"
        );
        let outcome = with_baseline(&main, GT_DEP);
        let Outcome::NotRun(why) = outcome("Enabled") else {
            panic!("{:?}", outcome("Enabled"))
        };
        assert!(why.contains("ondatabaseinsert") && why.contains("GT Mgt.Ins"), "{why}");
    }

    #[test]
    fn install_triggers_and_company_initialize_seed_the_rows_every_test_codeunit_starts_from() {
        // ADR-013 decision 5: dependency installs, then Company-Initialize, then every test starts
        // from that state (the Base Application setup rows are read by its own code).
        let outcome = with_baseline(
            "codeunit 50001 \"T\" { Subtype = Test; var L: Codeunit \"Lib\"; \
             [Test] procedure Seeded() begin if L.ReadSetup() <> 'installed+initialized' then Error(L.ReadSetup()); end; }",
            SETUP_DEP,
        );
        assert_eq!(outcome("Seeded"), Outcome::Pass);
    }

    #[test]
    fn per_company_triggers_run_first_in_codeunit_order_then_per_database_with_no_company() {
        // Measured on BC 27.5 (docs/probes-needed.md): every OnInstallAppPerCompany, then
        // OnInstallAppPerDatabase, which has no company (a DataPerCompany = false table works there).
        let outcome = with_baseline(
            "codeunit 50001 \"T\" { Subtype = Test; \
             [Test] procedure Order() var L: Record \"Order Log\"; Sh: Record \"Shared Log\"; T: Text; begin \
               L.FindSet(); repeat T += L.What + ';'; until L.Next() = 0; \
               Sh.FindFirst(); T += Sh.What; \
               if T <> 'A-company;B-company;init;A-db' then Error(T); end; }",
            SETUP_DEP,
        );
        assert_eq!(outcome("Order"), Outcome::Pass);
    }

    #[test]
    fn a_test_codeunit_starts_from_the_baseline_whatever_an_earlier_one_committed() {
        let main = "codeunit 50001 \"T\" { Subtype = Test; \
             [Test] procedure Changes() var S: Record \"Setup\"; begin S.Get(); S.Value := 'changed'; S.Modify(); end; } \
             codeunit 50002 \"T2\" { Subtype = Test; var L: Codeunit \"Lib\"; \
             [Test] procedure Fresh() begin if L.ReadSetup() <> 'installed+initialized' then Error(L.ReadSetup()); end; }";
        let rt = {
            let app = |id: &str, deps: &[&str], src: &str| AppSource {
                id: id.into(),
                name: id.into(),
                dependencies: deps.iter().map(|d| d.to_string()).collect(),
                files: vec![(format!("{id}.al"), src.into())],
            };
            CorpusRuntime::run_for_apps(
                vec![app("main", &["dep"], main), app("dep", &[], SETUP_DEP)],
                vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
                &[],
                &Default::default(),
                None,
                Default::default(),
            )
        };
        let o = |cu: u32, name: &str| {
            rt.first_outcome(&CaseId {
                codeunit: cu,
                name: name.into(),
            })
        };
        assert_eq!(o(50001, "Changes"), Outcome::Pass);
        assert_eq!(o(50002, "Fresh"), Outcome::Pass);
    }

    #[test]
    fn a_missing_row_is_refused_only_where_the_baseline_did_not_create_it() {
        // The installer cannot run (MinimumActiveRowVersion is not implemented), so the rows it
        // would create are missing: a Get that finds nothing, from dependency code or from the
        // tests' own, is then not BC's answer. Where the baseline did create rows, the real
        // error is reported.
        let dep = r#"
table 50100 "K" { fields { field(1; "No."; Code[10]) { } } }
table 50103 "Seeded" { fields { field(1; "No."; Code[10]) { } } }
codeunit 50110 "Installer" { Subtype = Install;
  trigger OnInstallAppPerCompany() var S: Record "Seeded"; begin S."No." := 'X'; S.Insert(); Later(); end;
  local procedure Later() var K: Record "K"; begin MinimumActiveRowVersion(); K.Insert(); end; }
codeunit 50101 "Lib" { procedure ReadK() var S: Record "K"; begin S.Get(); end; procedure ReadSeeded() var S: Record "Seeded"; begin S.Get('NOPE'); end; }
"#;
        let outcome = with_baseline(
            "codeunit 50001 \"T\" { Subtype = Test; var L: Codeunit \"Lib\"; \
             [Test] procedure Own() var S: Record \"K\"; begin asserterror S.Get(); end; \
             [Test] procedure Unseeded() begin L.ReadK(); end; \
             [Test] procedure Seeded() begin asserterror L.ReadSeeded(); end; }",
            dep,
        );
        for name in ["Own", "Unseeded"] {
            let Outcome::NotRun(why) = outcome(name) else {
                panic!("{name}: {:?}", outcome(name))
            };
            assert!(why.contains("row not found"), "{name}: {why}");
        }
        // The installer's rollback leaves `Seeded` empty too, so it is unseeded as well.
        assert!(matches!(outcome("Seeded"), Outcome::NotRun(_) | Outcome::Pass));
    }

    #[test]
    fn reads_of_a_table_the_baseline_could_not_fill_are_refused_everywhere_but_a_get_that_finds_the_row() {
        // The installer cannot run, so BC's rows in "K" are missing: a read whose answer depends on
        // them (Find*, Next, Count, IsEmpty, a FlowField over it, a TableRelation check against
        // it, from the workspace or from the dependency) is not BC's answer. A Get that finds the
        // row is; a table the installer does not write, and a temporary record, are unaffected.
        let dep = r#"
table 50100 "K" { fields { field(1; "No."; Code[10]) { } } keys { key(PK; "No.") { } } }
table 50103 "Other" { fields { field(1; "No."; Code[10]) { } } keys { key(PK; "No.") { } } }
table 50104 "Holder" { fields { field(1; "No."; Code[10]) { } field(2; Cnt; Integer) { FieldClass = FlowField; CalcFormula = count(K); }
  field(3; Ref; Code[10]) { TableRelation = K; } } keys { key(PK; "No.") { } } }
codeunit 50110 "Installer" { Subtype = Install;
  trigger OnInstallAppPerCompany() var S: Record "K"; begin MinimumActiveRowVersion(); S.Insert(); end; }
codeunit 50101 "Lib" { procedure Count(): Integer var S: Record "K"; begin exit(S.Count()); end; }
"#;
        let main = r#"codeunit 50001 "T" { Subtype = Test; var L: Codeunit "Lib";
  [Test] procedure FindFirstIsRefused() var S: Record "K"; begin if S.FindFirst() then; end;
  [Test] procedure FindSetIsRefused() var S: Record "K"; begin if S.FindSet() then; end;
  [Test] procedure CountIsRefused() var S: Record "K"; begin if S.Count() = 0 then; end;
  [Test] procedure IsEmptyIsRefused() var S: Record "K"; begin if S.IsEmpty() then; end;
  [Test] procedure DependencyCountIsRefused() begin if L.Count() = 0 then; end;
  [Test] procedure FlowFieldIsRefused() var H: Record "Holder"; begin H."No." := 'A'; H.Insert(); H.CalcFields(Cnt); end;
  [Test] procedure TableRelationMissIsRefused() var H: Record "Holder"; begin H."No." := 'B'; H.Insert(); H.Validate(Ref, 'X'); end;
  [Test] procedure GetThatFindsTheRowIsAnswered() var S: Record "K"; begin S."No." := 'W'; S.Insert(); S.Get('W'); end;
  [Test] procedure AnotherTableIsRead() var O: Record "Other"; begin O."No." := 'W'; O.Insert(); if O.Count() <> 1 then Error('x'); end;
  [Test] procedure ATemporaryRecordIsRead() var S: Record "K" temporary; begin S."No." := 'W'; S.Insert(); if S.Count() <> 1 then Error('x'); end; }"#;
        let outcome = with_baseline(main, dep);
        for refused in [
            "FindFirstIsRefused",
            "FindSetIsRefused",
            "CountIsRefused",
            "IsEmptyIsRefused",
            "DependencyCountIsRefused",
            "FlowFieldIsRefused",
            "TableRelationMissIsRefused",
        ] {
            let Outcome::NotRun(why) = outcome(refused) else {
                panic!("{refused}: {:?}", outcome(refused))
            };
            assert!(why.contains("install baseline could not create"), "{refused}: {why}");
        }
        for answered in [
            "GetThatFindsTheRowIsAnswered",
            "AnotherTableIsRead",
            "ATemporaryRecordIsRead",
        ] {
            assert_eq!(outcome(answered), Outcome::Pass, "{answered}");
        }
    }

    #[test]
    fn company_initialize_that_stops_keeps_the_rows_it_wrote_before() {
        let dep = r#"
table 50100 "Setup" { fields { field(1; "Primary Key"; Code[10]) { } } keys { key(PK; "Primary Key") { } } }
table 50104 "Never" { fields { field(1; "Primary Key"; Code[10]) { } } keys { key(PK; "Primary Key") { } } }
codeunit 2 "Company-Initialize" {
  trigger OnRun() var S: Record "Setup"; begin S.Insert(); Later(); end;
  local procedure Later() var N: Record "Never"; begin MinimumActiveRowVersion(); N.Insert(); end; }
codeunit 50101 "Lib" { procedure SetupCount(): Integer var S: Record "Setup"; begin exit(S.Count()); end;
  procedure ReadNever() var N: Record "Never"; begin N.Get(); end; }
"#;
        let outcome = with_baseline(
            "codeunit 50001 \"T\" { Subtype = Test; var L: Codeunit \"Lib\"; \
             [Test] procedure Kept() begin if L.SetupCount() <> 1 then Error('rows: %1', L.SetupCount()); end; \
             [Test] procedure NotCreated() begin L.ReadNever(); end; }",
            dep,
        );
        assert_eq!(outcome("Kept"), Outcome::Pass);
        assert!(matches!(outcome("NotCreated"), Outcome::NotRun(_)));
    }

    fn outcomes_isolated(src: &str, isolation: TestIsolation) -> Vec<(String, Outcome)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l
            .program
            .objects
            .iter()
            .position(|o| {
                o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.eq_ignore_ascii_case("Test"))
            })
            .unwrap();
        run_codeunit_isolated(&engine, &l, oi, None, isolation)
    }

    // The isolation probe's fixture (corpus tests/al-language-isolation-probe/src/
    // IsolProbeFixture.Codeunit.al:25-114), shortened. Real BC, nightly run on 28.4
    // (oracle/28/xunit-isolation-function.xml): all seven pass under TestIsolation = Function;
    // oracle/28/xunit-isolation-codeunit.xml: all but T5 pass under Codeunit.
    const PROBE: &str = "table 61300 Row { fields { field(1; K; Code[20]) { } } keys { key(PK; K) { Clustered = true; } } }
        codeunit 61303 Single { SingleInstance = true;
            procedure SetMark(M: Integer) begin Mark := M; end;
            procedure GetMark(): Integer begin exit(Mark); end;
            var Mark: Integer; }
        codeunit 61302 Fixture { Subtype = Test;
            var GlobalMark: Integer; CallCount: Integer; OnRunCount: Integer;
            trigger OnRun() var R: Record Row; begin
                OnRunCount += 1;
                if not R.Get('ONRUN') then begin R.K := 'ONRUN'; R.Insert(); end; end;
            [Test] procedure T1() var R: Record Row; S: Codeunit Single; begin
                CallCount += 1; GlobalMark := 4826; S.SetMark(4826);
                R.K := 'MARK'; R.Insert();
                Expect(1, CallCount, 'T1 first on its instance'); end;
            [Test] procedure T2() begin CallCount += 1; Expect(4826, GlobalMark, 'global set by T1'); end;
            [Test] procedure T3() begin CallCount += 1; Expect(3, CallCount, 'counter counts T1 to T3'); end;
            [Test] procedure T4() var S: Codeunit Single; begin CallCount += 1; Expect(4826, S.GetMark(), 'single instance'); end;
            [Test] procedure T5() var R: Record Row; begin
                CallCount += 1;
                if R.Get('MARK') then Error('The row T1 inserted is still there in T5'); end;
            [Test] procedure T6() var R: Record Row; begin
                CallCount += 1;
                if not R.Get('ONRUN') then Error('The row OnRun inserted is gone in T6'); end;
            [Test] procedure T7() begin CallCount += 1; Expect(1, OnRunCount, 'OnRun ran once'); end;
            local procedure Expect(E: Integer; A: Integer; What: Text) begin
                if A <> E then Error('%1: expected %2, observed %3', What, E, A); end; }";

    // oracle/28/xunit-isolation-function.xml: T1 to T7 all Pass.
    #[test]
    fn function_isolation_rolls_the_database_back_per_test_and_keeps_the_instance() {
        let o = outcomes_isolated(PROBE, TestIsolation::Function);
        assert_eq!(
            o.iter().map(|(_, x)| x.clone()).collect::<Vec<_>>(),
            vec![Outcome::Pass; 7],
            "{o:?}"
        );
    }

    // oracle/28/xunit-isolation-codeunit.xml: T5 fails with the row still there, the rest pass.
    #[test]
    fn codeunit_isolation_keeps_the_database_between_tests() {
        let o = outcomes_isolated(PROBE, TestIsolation::Codeunit);
        let passes: Vec<_> = o.iter().map(|(n, x)| (n.as_str(), *x == Outcome::Pass)).collect();
        assert_eq!(
            passes,
            vec![
                ("T1", true),
                ("T2", true),
                ("T3", true),
                ("T4", true),
                ("T5", false),
                ("T6", true),
                ("T7", true)
            ]
        );
        assert_eq!(o[4].1, Outcome::Fail("The row T1 inserted is still there in T5".into()));
    }

    #[test]
    fn function_isolation_also_rolls_back_a_failed_test() {
        let o = outcomes_isolated(
            &with_rows(
                "codeunit 60102 T { Subtype = Test;
                    [Test] procedure A() var R: Record S; begin R.K := 'A'; R.Insert(); Error('boom'); end;
                    [Test] procedure B() var R: Record S; begin if R.Count() <> 0 then Error('rows left: %1', R.Count()); end; }",
            ),
            TestIsolation::Function,
        );
        assert_eq!(o[0].1, Outcome::Fail("boom".into()));
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
    }

    // Nothing pins what Disabled leaves behind, so every test of the codeunit is refused.
    #[test]
    fn disabled_isolation_is_refused_for_every_test() {
        let o = outcomes_isolated(PROBE, TestIsolation::Disabled);
        assert_eq!(o.len(), 7);
        assert!(
            o.iter()
                .all(|(_, x)| matches!(x, Outcome::NotRun(w) if w.contains("Disabled"))),
            "{o:?}"
        );
    }

    // What a Commit() inside a test does under Function isolation is not pinned: the test that
    // would depend on it is refused, and the next test still starts from the OnRun state.
    #[test]
    fn a_commit_inside_a_function_isolated_test_is_refused_and_still_rolled_back() {
        let src = with_rows(
            "codeunit 60102 T { Subtype = Test;
                [Test] procedure Commits() var R: Record S; begin R.K := 'A'; R.Insert(); Commit(); end;
                [Test] procedure Next() var R: Record S; begin if R.Count() <> 0 then Error('rows left: %1', R.Count()); end; }",
        );
        let o = outcomes_isolated(&src, TestIsolation::Function);
        assert!(
            matches!(&o[0].1, Outcome::NotRun(w) if w.contains("Commit") && w.contains("Function")),
            "{o:?}"
        );
        assert_eq!(o[1].1, Outcome::Pass, "{o:?}");
        // Under Codeunit isolation the same Commit is the pinned default behaviour.
        let o = outcomes_isolated(&src, TestIsolation::Codeunit);
        assert_eq!(o[0].1, Outcome::Pass, "{o:?}");
    }

    #[test]
    fn retire_drops_the_value_on_another_thread() {
        struct Marker(std::sync::mpsc::Sender<std::thread::ThreadId>);
        impl Drop for Marker {
            fn drop(&mut self) {
                let _ = self.0.send(std::thread::current().id());
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        super::retire(Marker(tx));
        let dropped_on = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the value is dropped");
        assert_ne!(dropped_on, std::thread::current().id());
    }

    /// A test that never ends is stopped at its time limit and reported as not run, with the limit
    /// in the reason. The tests after it still run; a failure after it is not trusted, because the
    /// stopped test left the codeunit's state half way.
    #[test]
    fn a_test_over_its_time_limit_is_stopped_and_not_run() {
        let src = "codeunit 61400 Endless { Subtype = Test;
            [Test] procedure Before() begin end;
            [Test] procedure ForEver() var I: Integer; begin while true do I += 1; end;
            [Test] procedure ForEverRepeat() var I: Integer; begin repeat I += 1; until false; end;
            [Test] procedure After() begin end;
            [Test] procedure AfterFails() begin Error('x'); end; }";
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let blocked = l
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = Engine::new(l.program.clone(), blocked);
        let oi = l.program.objects.iter().position(|o| o.name == "Endless").unwrap();
        let started = std::time::Instant::now();
        let out = run_codeunit_limited(
            &engine,
            &l,
            oi,
            None,
            TestIsolation::Codeunit,
            Some(std::time::Duration::from_millis(300)),
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let get = |n: &str| out.iter().find(|(name, _)| name == n).unwrap().1.clone();
        assert!(matches!(get("Before"), Outcome::Pass));
        for t in ["ForEver", "ForEverRepeat"] {
            match get(t) {
                Outcome::NotRun(r) => assert!(r.contains("time limit"), "{r}"),
                o => panic!("{t}: {o:?}"),
            }
        }
        assert!(matches!(get("After"), Outcome::Pass));
        assert!(matches!(get("AfterFails"), Outcome::NotRun(_)));
    }

    /// The limit counts the CPU time of the test thread: a thread that waits is not stopped at the
    /// limit (the machine's load must not decide an outcome), one that computes is.
    #[cfg(unix)]
    #[test]
    fn the_time_limit_counts_cpu_time_not_wall_time() {
        let limit = std::time::Duration::from_millis(200);
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = Watchdog::start(flag.clone(), limit);
        std::thread::sleep(limit * 3);
        assert!(!w.finish(), "a thread that waited was stopped");
        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));

        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = Watchdog::start(flag.clone(), limit);
        let started = std::time::Instant::now();
        let mut x = 0u64;
        while !flag.load(std::sync::atomic::Ordering::Relaxed) && started.elapsed() < std::time::Duration::from_secs(10)
        {
            x = x.wrapping_add(std::hint::black_box(1));
        }
        assert!(w.finish() && x > 0, "a thread that computed was not stopped");
    }

    /// The limit is per test: the CPU time a thread used before the test started does not count.
    /// Three tests on one thread, each using 60 % of the limit, all stay under it.
    #[cfg(any(unix, windows))]
    #[test]
    fn the_cpu_limit_is_measured_from_the_start_of_each_test() {
        let limit = std::time::Duration::from_millis(500);
        for n in 0..3 {
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let w = Watchdog::start(flag.clone(), limit);
            let clock = crate::cpuclock::ThreadClock::current().expect("a CPU clock");
            let from = clock.read().expect("a reading");
            let mut x = 0u64;
            while clock.read().expect("a reading") - from < limit * 6 / 10 {
                x = x.wrapping_add(std::hint::black_box(1));
            }
            // Let the watchdog poll once more after the work.
            std::thread::sleep(WATCHDOG_POLL * 3);
            assert!(!w.finish() && x > 0, "test {n} was stopped under 60 % of the limit");
        }
    }

    /// The test names with the unprobed assumptions each ran through, from the first test codeunit of `src`.
    fn assumptions_of(src: &str) -> Vec<(String, Vec<&'static str>)> {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        );
        assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
        let engine = Engine::new(l.program.clone(), Vec::new().into_iter().collect());
        let oi = l
            .program
            .objects
            .iter()
            .position(|o| {
                o.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.eq_ignore_ascii_case("Test"))
            })
            .unwrap();
        let mut hits = Vec::new();
        let done = run_codeunit_full(
            &engine,
            &l,
            oi,
            None,
            TestIsolation::Codeunit,
            None,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut hits,
            None,
        );
        for (name, outcome) in &done {
            assert_eq!(*outcome, Outcome::Pass, "{name}");
        }
        hits.into_iter()
            .map(|(n, h)| (n, h.iter().map(|a| a.info().id).collect()))
            .collect()
    }

    const FLAGGED: &str = "codeunit 60150 T
{
    Subtype = Test;

    [Test]
    procedure RunsAnAssumption()
    var
        G: Guid;
        T: Text[50];
    begin
        T := Format(G);
        T := G;
    end;

    [Test]
    procedure RunsNone()
    var
        X: Integer;
    begin
        X := 1;
    end;

    [Test]
    procedure RunsNoneAfterTheOneThatDid()
    var
        X: Integer;
    begin
        X := 2;
    end;
}";

    #[test]
    fn a_test_that_runs_an_assumed_path_is_marked_and_the_marks_do_not_leak_to_the_next_test() {
        let hits = assumptions_of(FLAGGED);
        let of = |n: &str| hits.iter().find(|(t, _)| t == n).map(|(_, h)| h.clone());
        assert_eq!(of("RunsAnAssumption"), Some(vec!["guid-to-text"]), "{hits:?}");
        assert_eq!(of("RunsNone"), None, "{hits:?}");
        assert_eq!(of("RunsNoneAfterTheOneThatDid"), None, "{hits:?}");
    }

    #[test]
    fn a_statement_marked_at_compile_time_marks_the_test_only_when_it_runs() {
        let src = "codeunit 60151 T
{
    Subtype = Test;

    [Test]
    procedure RunsTheMarkedStatement()
    var
        A: array[2, 2] of Integer;
    begin
        A[1][2] := 3;
    end;

    [Test]
    procedure SkipsTheMarkedStatement()
    var
        A: array[2, 2] of Integer;
    begin
        if false then
            A[1][2] := 3;
    end;
}";
        let hits = assumptions_of(src);
        let of = |n: &str| hits.iter().find(|(t, _)| t == n).map(|(_, h)| h.clone());
        assert_eq!(
            of("RunsTheMarkedStatement"),
            Some(vec!["chained-array-index"]),
            "{hits:?}"
        );
        assert_eq!(of("SkipsTheMarkedStatement"), None, "{hits:?}");
    }
}
