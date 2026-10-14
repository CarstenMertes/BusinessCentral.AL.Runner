//! `alrunner test`: load the APPs and their dependencies, compile, run, report.

use crate::apps::{self, AppInput};
use crate::args::{DEFAULT_TOP, TestArgs};
use crate::output::{self, Counts};
use al_deps::load::WorkspaceApp;
use al_host::dotnet::DotNetHost;
use al_testrun::deps::{self, Provision};
use al_testrun::discover;
use al_testrun::dotnet::{SidecarConfig, SidecarHost};
use al_testrun::refusals;
use al_testrun::runtime::{CorpusRuntime, RunOptions, SuiteRun, TestResult};
use al_testrun::{CaseId, Outcome, target};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A test codeunit found in an APP's source, before anything is compiled.
struct Found {
    app: usize,
    id: u32,
    tests: Vec<String>,
}

/// The newest BC major this runtime is verified against (the fallback when nothing names one).
const NEWEST_VERIFIED: u16 = 29;

/// Which tests a run is about.
#[derive(Default)]
struct Selection {
    codeunits: Vec<u32>,
    filters: Vec<String>,
}

impl Selection {
    fn active(&self) -> bool {
        !self.codeunits.is_empty() || !self.filters.is_empty()
    }

    fn picks(&self, codeunit: u32, test: &str) -> bool {
        if !self.active() || self.codeunits.contains(&codeunit) {
            return true;
        }
        let t = test.to_lowercase();
        self.filters.iter().any(|f| t.contains(&f.to_lowercase()))
    }
}

/// How many worker threads a phase uses: the variable's value, else one per core.
fn threads(var: &str) -> String {
    match std::env::var(var).ok().and_then(|v| v.parse::<usize>().ok()) {
        Some(n) => format!("{n} ({var})"),
        None => format!(
            "{} (cores)",
            std::thread::available_parallelism().map_or(1, |n| n.get())
        ),
    }
}

/// The peak resident memory of this process so far, in MB (Linux; `None` elsewhere).
fn peak_memory_mb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = s
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb / 1024)
}

fn secs(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// The repository root when this binary was built inside one (for the .NET helper's source).
fn source_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.join("tools/dotnet-sidecar").is_dir())
        .map(Path::to_path_buf)
}

/// Writes `text` to `path`, creating the parent folders (OUT-11).
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn test(a: &TestArgs) -> Result<u8, String> {
    let started = Instant::now();
    if let Some(dir) = &a.files_dir {
        al_testrun::host::set_files_dir(dir.clone());
    }
    // Fail on an unwritable output path before the long part, not after it.
    if let Some(p) = &a.junit {
        write_file(p, "")?;
    }
    let inputs = a
        .apps
        .iter()
        .map(|p| apps::load(p, a.bc_version.as_deref()))
        .collect::<Result<Vec<AppInput>, String>>()?;

    // Which test codeunits there are, and which of them this run is about.
    let mut found: Vec<Found> = Vec::new();
    for (i, app) in inputs.iter().enumerate() {
        for (_, text) in &app.source.files {
            for (id, tests) in discover::test_codeunits(text) {
                found.push(Found { app: i, id, tests });
            }
        }
    }
    let selection = Selection {
        codeunits: a.codeunits.clone(),
        filters: a.filters.clone(),
    };
    let chosen_units: Vec<&Found> = found
        .iter()
        .filter(|f| f.tests.iter().any(|t| selection.picks(f.id, t)) || selection.codeunits.contains(&f.id))
        .collect();
    if chosen_units.is_empty() {
        let what = if selection.active() {
            "the selection matches no test"
        } else {
            "no test codeunit (Subtype = Test) in the given apps"
        };
        eprintln!("error: {what}");
        return Ok(6);
    }

    // The target: --bc-version, else the floor of the first app (ADR-015 3.2).
    let requested = a
        .bc_version
        .as_deref()
        .map(al_version::BcVersion::parse)
        .transpose()
        .map_err(|e| format!("--bc-version: {e}"))?;
    let mut chosen = al_version::choose_target(
        requested.as_ref(),
        &inputs[0].versions,
        &al_version::BcVersion::new(NEWEST_VERIFIED, 0),
    )
    .map_err(|e| e.to_string())?;
    for note in &chosen.notes {
        eprintln!("note: {note}");
    }

    // Dependencies: --dep, --package-cache, each folder APP's .alpackages, then the feed.
    let project = apps::combined_project(&inputs);
    let workspace: Vec<WorkspaceApp> = inputs
        .iter()
        .map(|i| WorkspaceApp {
            json: i.json.clone(),
            origin: format!("source {}", i.path.display()),
        })
        .collect();
    let mut search: Vec<PathBuf> = a.deps.clone();
    search.extend(a.package_caches.iter().cloned());
    search.extend(
        inputs
            .iter()
            .filter(|i| i.path.is_dir())
            .map(|i| i.path.join(".alpackages")),
    );
    let floor_versions = &inputs[0].versions;
    let provision = Provision {
        cache_dir: a.cache_dir.clone(),
        download: !a.no_download,
        query: target::build_query(a.bc_version.as_deref(), &chosen, floor_versions),
    };
    let dependencies = deps::load_project(
        &project,
        &workspace,
        &search,
        Some(&provision),
        a.bc_version.as_deref(),
        true,
    )?;
    let mut bc_build = None;
    if let Some(u) = &dependencies.build {
        chosen.target.bc.build = Some(u.build.to_string());
        bc_build = Some(u.build.to_string());
    }
    // The resolver repeats a note on each round of the artifact feed; say each once.
    let mut seen = BTreeSet::new();
    let mut hidden = 0;
    for line in &dependencies.log {
        let is_problem = line.starts_with("dependency skipped") || line.starts_with("dependency note");
        if (a.verbose || is_problem) && seen.insert(line.as_str()) {
            if a.verbose || seen.len() <= 12 {
                // A missing-dependency line repeats the whole search list; the start says it all.
                eprintln!(
                    "{}",
                    if a.verbose {
                        line.clone()
                    } else {
                        al_testrun::refusals::clip(line, 400)
                    }
                );
            } else {
                hidden += 1;
            }
        }
    }
    if hidden > 0 {
        eprintln!("... {hidden} more dependency notes (--verbose lists them)");
    }

    if !a.quiet {
        println!("alrunner {}", env!("CARGO_PKG_VERSION"));
        println!(
            "target: BC {}, runtime {}{}",
            chosen.target.bc,
            chosen.target.runtime,
            match &bc_build {
                Some(b) => format!(", Microsoft apps from build {b}"),
                None => String::new(),
            }
        );
        for (i, app) in inputs.iter().enumerate() {
            let units = found.iter().filter(|f| f.app == i).count();
            let tests: usize = found.iter().filter(|f| f.app == i).map(|f| f.tests.len()).sum();
            println!(
                "app: {} {} ({} files, {units} test codeunits, {tests} tests) from {}",
                app.json.name,
                app.manifest.version,
                app.source.files.len(),
                app.path.display()
            );
        }
        println!("dependencies: {} apps", dependencies.apps.len());
        println!(
            "threads: check {}, lower {}, test {}",
            threads("AL_CHECK_THREADS"),
            threads("AL_LOWER_THREADS"),
            threads("AL_TEST_THREADS")
        );
        println!("isolation: TestIsolation = Codeunit (the default of Microsoft's test runner)");
        if selection.active() {
            println!("selection: {} test codeunits of {}", chosen_units.len(), found.len());
        }
    }

    // Compile and run.
    let mut all_apps: Vec<al_sema::workspace::AppSource> = inputs.iter().map(|i| i.source.clone()).collect();
    // An app with an `application` or `platform` floor sees `Application`, what it propagates and
    // `System` without declaring them.
    for (app, input) in all_apps.iter_mut().zip(&inputs) {
        let implied =
            dependencies.implied_dependencies(input.json.platform.is_some(), input.json.application.is_some());
        for id in implied {
            if !id.eq_ignore_ascii_case(&app.id) && !app.dependencies.contains(&id) {
                app.dependencies.push(id);
            }
        }
    }
    let mut kinds = vec![al_sema::AppKind::Workspace; all_apps.len()];
    let mut manifests: Vec<al_testrun::runtime::Manifest> = inputs.iter().map(|i| i.manifest.clone()).collect();
    kinds.extend(dependencies.apps.iter().map(|_| al_sema::AppKind::Dependency));
    manifests.extend(dependencies.manifests.iter().cloned());
    all_apps.extend(dependencies.apps.iter().cloned());
    let symbols: BTreeSet<String> = inputs.iter().flat_map(|i| i.symbols.iter().cloned()).collect();

    let host = Arc::new(SidecarHost::new(SidecarConfig {
        sidecar: a.dotnet_sidecar.clone(),
        source_root: source_root(),
        assembly_dirs: a.dotnet_assembly_dirs.clone(),
        allow_roll_forward: a.dotnet_allow_roll_forward,
        timeout: Duration::from_secs(a.dotnet_timeout_secs.unwrap_or(60)),
        ..Default::default()
    }));
    if a.require_dotnet {
        if let Err(why) = host.availability() {
            return Err(format!("--require-dotnet: DotNet {why}"));
        }
    }
    let dotnet = al_vm::dotnet::DotNetConfig {
        host,
        ..Default::default()
    };
    let opts = RunOptions {
        only: selection.active().then(|| {
            let mut v: Vec<(usize, u32)> = chosen_units.iter().map(|f| (f.app, f.id)).collect();
            v.sort_unstable();
            v.dedup();
            v
        }),
        trace: false,
    };
    let load_time = started.elapsed();
    let rt = CorpusRuntime::run_suites_with(
        all_apps,
        kinds,
        &manifests,
        &symbols,
        Some(&chosen.target),
        dotnet,
        &[SuiteRun::everything()],
        &opts,
    );
    let p = &rt.phases;

    // The selected tests, in a stable order: app, codeunit, then declaration order.
    let mut results: Vec<&TestResult> = rt
        .results()
        .iter()
        .filter(|r| selection.picks(r.codeunit, &r.test))
        .collect();
    results.sort_by_key(|r| (r.app, r.codeunit));
    let counts = Counts::of(results.iter().copied());

    if a.timings {
        let c = &p.compile_parts;
        eprintln!(
            "compile: load {}, members {}, check workspace {}, baseline roots {}, demand {}, check rest {}, lower {}, engine {}",
            secs(c.load),
            secs(c.members),
            secs(c.check.workspace),
            secs(c.check.baseline),
            secs(c.check.demand),
            secs(c.check.rest),
            secs(c.lower),
            secs(p.engine)
        );
        let slowest: Vec<String> = p
            .slowest
            .iter()
            .map(|(id, name, t)| format!("{id} {name} {}", secs(*t)))
            .collect();
        eprintln!("slowest test codeunits: {}", slowest.join("; "));
    }
    if rt.panics > 0 {
        eprintln!(
            "warning: {} test codeunit(s) hit a runtime panic (reported as not run)",
            rt.panics
        );
    }

    // One line per test, or per failure.
    if !a.quiet {
        let mut shown_failures = 0;
        let mut hidden_failures = 0;
        for r in &results {
            let line = match &r.outcome {
                Outcome::Pass if a.verbose => format!("PASS    {} {}.{}", r.codeunit, r.codeunit_name, r.test),
                Outcome::NotRun(why) if a.verbose => format!(
                    "NOT RUN {} {}.{}: {}",
                    r.codeunit,
                    r.codeunit_name,
                    r.test,
                    output::first_line(why, 300)
                ),
                Outcome::Fail(why) => {
                    if !a.verbose && shown_failures >= 30 {
                        hidden_failures += 1;
                        continue;
                    }
                    shown_failures += 1;
                    format!(
                        "FAIL    {} {}.{}: {}",
                        r.codeunit,
                        r.codeunit_name,
                        r.test,
                        output::first_line(why, 300)
                    )
                }
                _ => continue,
            };
            println!("{line}");
        }
        if hidden_failures > 0 {
            println!("... {hidden_failures} more failures (--verbose lists them, --junit has every message)");
        }
    }

    // The ranking of refusal reasons: every run, unless --quiet; --top-refusals N sets N.
    let top = a.top_refusals.or((!a.quiet).then_some(DEFAULT_TOP));
    if let Some(n) = top.filter(|_| counts.not_run > 0) {
        let list = refusals::rank(results.iter().filter_map(|r| match &r.outcome {
            Outcome::NotRun(why) => Some((
                CaseId {
                    codeunit: r.codeunit,
                    name: r.test.clone(),
                },
                why.clone(),
            )),
            _ => None,
        }));
        println!("\n{}", refusals::render_top("not-run tests", &list, n));
    }

    let total_time = started.elapsed();
    if let Some(path) = &a.junit {
        write_file(
            path,
            &output::junit(&results, bc_build.as_deref(), total_time.as_secs_f64()),
        )?;
    }

    let rate = if p.tests.as_secs_f64() > 0.0 {
        counts.total as f64 / p.tests.as_secs_f64()
    } else {
        0.0
    };
    println!("{}", output::summary_line(&counts));
    println!(
        "Time: load {}, compile {}, install baseline {}, run {} ({:.0} tests/s), total {}{}",
        secs(load_time),
        secs(p.compile),
        secs(p.baseline),
        secs(p.tests),
        rate,
        secs(total_time),
        peak_memory_mb().map_or(String::new(), |m| format!(", peak memory {m} MB"))
    );
    let code = output::exit_code(&counts, a.allow_not_run);
    println!(
        "Result: {} (exit {code})",
        match code {
            0 => "PASSED",
            1 => "FAILED",
            6 => "NOTHING SELECTED",
            7 => "TESTS NOT RUN",
            _ => "ERROR",
        }
    );
    Ok(code)
}
