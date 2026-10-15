//! `alrunner test`: load the APPs and their dependencies, compile, run, report.

use crate::apps::{self, AppInput};
use crate::args::{DEFAULT_TOP, TestArgs};
use crate::output::{self, Counts};
use crate::testdata;
use al_bcdb_std::BackupProvider;
use al_deps::load::WorkspaceApp;
use al_host::dotnet::DotNetHost;
use al_host::testdata::TestData;
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

/// Opens a backup file.
fn open_backup(a: &TestArgs, path: PathBuf) -> Result<(TestData, PathBuf), String> {
    // The page map of the backup is kept next to the compile cache, so a cold or a warm open of a
    // 900 MB backup reads the map instead of verifying every extent.
    let index_dir = (!a.no_compile_cache
        && !std::env::var("ALRUNNER_NO_COMPILE_CACHE").is_ok_and(|v| !v.is_empty() && v != "0"))
    .then(|| al_deps::cache::cache_root_from_env(a.cache_dir.as_deref()).join("backup-index"));
    let provider = BackupProvider::open_with_index_dir(&path, a.test_data_company.as_deref(), index_dir.as_deref())
        .map_err(|e| format!("--test-data {}: {e}", path.display()))?;
    Ok((TestData::new(Arc::new(provider)), path))
}

/// What the test data of a run came to.
struct DataChoice {
    data: Option<(TestData, PathBuf)>,
    /// Lines for stderr: where the data is from when that is not the build of the apps, why it is
    /// off, what failed on the way.
    notes: Vec<String>,
}

/// Finds and opens the demo backup (ADR-020): the build of the Microsoft apps first, on disk, then
/// in the cache, then downloaded; see [`testdata::locate`]. A default run whose backup cannot be
/// found or opened has test data off and says why; a run that asked for test data fails instead.
fn choose_test_data(
    a: &TestArgs,
    builds: &[String],
    downloadable: &[al_deps::build::BuildId],
    major: u16,
    minor: Option<u16>,
) -> Result<DataChoice, String> {
    let asked = a.test_data_asked_for();
    let fail = |why: String| -> Result<DataChoice, String> {
        if asked {
            Err(format!("--test-data: {why}"))
        } else {
            Ok(DataChoice {
                data: None,
                notes: vec![format!(
                    "test data is off: {why}. Microsoft's tests expect a CRONUS company; failures of such a run are mostly missing setup data. Name a backup with --test-data PATH.bak"
                )],
            })
        }
    };
    let (path, mut notes) = match &a.test_data_path {
        Some(p) => (p.clone(), Vec::new()),
        None => {
            let cache_root = al_deps::cache::cache_root_from_env(a.cache_dir.as_deref());
            let local = al_deps::cache::bcartifacts_sandbox_from_env();
            let download = (!a.no_download).then_some(|build: &al_deps::build::BuildId| -> Result<PathBuf, String> {
                let cdn = al_deps::artifact::HttpCdn::from_env();
                let mut printed = 0u64;
                let t = Instant::now();
                eprintln!("test data: downloading the demo data of build {build} (about 520 MB, once) ...");
                let got = al_deps::demo::ensure_demo_backup(&cdn, &cache_root, build, "w1", &mut |read, total| {
                    let pct = (read * 100).checked_div(total).unwrap_or(100);
                    if pct >= printed + 10 || (pct == 100 && printed < 100) {
                        printed = pct - pct % 10;
                        eprintln!(
                            "test data: {pct}% ({} of {} MB, {:.0} s)",
                            read >> 20,
                            total >> 20,
                            t.elapsed().as_secs_f64()
                        );
                    }
                })?;
                Ok(got.path)
            });
            let found = testdata::locate(
                builds,
                downloadable,
                major,
                minor,
                testdata::Where {
                    local_root: local.as_deref(),
                    cache_root: &cache_root,
                    download,
                },
                "w1",
            );
            match found.path {
                Some(p) => (p, found.notes),
                None => return fail(found.off.unwrap_or_default()),
            }
        }
    };
    match open_backup(a, path) {
        Ok(d) => Ok(DataChoice {
            data: Some(d),
            notes: std::mem::take(&mut notes),
        }),
        Err(e) if asked => Err(e),
        Err(e) => fail(e),
    }
}

/// `alrunner warm`: finds the backup, opens it, and so builds the index kept next to the compile cache.
pub fn warm(a: &TestArgs) -> Result<(), String> {
    let version = a.bc_version.as_deref();
    let numbers: Vec<u16> = version
        .map(|v| v.split('.').map_while(|p| p.parse().ok()).collect())
        .unwrap_or_default();
    let (major, minor) = (numbers.first().copied().unwrap_or(0), numbers.get(1).copied());
    let builds: Vec<String> = version.map(str::to_string).into_iter().collect();
    let downloadable: Vec<al_deps::build::BuildId> = version
        .and_then(|v| al_deps::build::BuildId::parse(v).ok())
        .into_iter()
        .collect();
    let choice = choose_test_data(a, &builds, &downloadable, major, minor)?;
    for n in &choice.notes {
        eprintln!("{n}");
    }
    let (data, path) = choice.data.ok_or("no backup could be opened")?;
    println!(
        "{}: {}",
        path.display(),
        data.provider.cost().unwrap_or_else(|| "opened".to_string())
    );
    Ok(())
}

pub fn test(a: &TestArgs) -> Result<u8, String> {
    let started = Instant::now();
    // The key of the compile cache starts with a hash of this executable (10 ms): make it beside the loading.
    if !a.no_compile_cache {
        std::thread::spawn(|| {
            al_testrun::compile_cache::runtime_build();
        });
    }
    if let Some(env) = a.environment {
        al_testrun::host::set_environment(env);
    }
    if let Some(dir) = &a.files_dir {
        al_testrun::host::set_files_dir(dir.clone());
    }
    // Fail on an unwritable output path before the long part, not after it.
    if let Some(p) = &a.junit {
        write_file(p, "")?;
    }
    // A backup named on the command line is opened before the long part: a wrong path or company
    // fails here. The cached artifact is chosen once the Microsoft apps are known.
    let mut test_data = match (a.wants_test_data(), &a.test_data_path) {
        (true, Some(p)) => Some(open_backup(a, p.clone())?),
        _ => None,
    };
    let mut data_notes: Vec<String> = Vec::new();
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
        // `--explain NAME --codeunit ID` explains the test of that one codeunit; the name alone
        // selects every codeunit that has a test of that name.
        filters: a
            .filters
            .iter()
            .chain(a.explain.iter().filter(|_| a.codeunits.is_empty()))
            .cloned()
            .collect(),
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
    // Which Microsoft build the project targets: see `apps::choose_build` (ADR-013, "Amendment 2026-10-15").
    let own = apps::own_build(&inputs);
    let floor = floor_versions.floor().map(|f| (u32::from(f.major), u32::from(f.minor)));
    let packages = if a.bc_version.is_none() && own.is_none() {
        al_deps::load::microsoft_packages_build(&search, None)
    } else {
        None
    };
    let choice = apps::choose_build(
        a.bc_version.as_deref(),
        own,
        packages,
        floor,
        u32::from(chosen.target.bc.major),
    );
    if let Some(note) = &choice.note {
        eprintln!("note: {note}");
    }
    if let Some(minor) = choice.minor {
        chosen.target.bc.minor = minor as u16;
    }
    let from_packages = choice.from_packages;
    let mut build_rule = choice.rule;
    let preferred = choice.preferred;
    let preferred_build = preferred.clone();
    let provision = Provision {
        cache_dir: a.cache_dir.clone(),
        download: !a.no_download,
        query: target::build_query(a.bc_version.as_deref(), &chosen, floor_versions).with_preferred(preferred),
        local_artifacts: al_deps::cache::bcartifacts_sandbox_from_env(),
    };
    // The text of a package's files is left unread: a run whose tests reach nothing in it never needs it.
    // ALRUNNER_EAGER_SOURCES=1 reads it at once, as before (the other side of the differential check).
    let lazy_sources = std::env::var_os("ALRUNNER_EAGER_SOURCES").is_none();
    let dependencies = deps::load_project_with(
        &project,
        &workspace,
        &search,
        Some(&provision),
        a.bc_version.as_deref(),
        true,
        lazy_sources,
    )?;
    if a.wants_test_data() && test_data.is_none() {
        // The build of the Microsoft apps is the build of the data: what the feed gave, what
        // --bc-version named, then the versions of the loaded packages and of the APPs
        // (Microsoft's test archives carry the build as their version).
        let mut builds: Vec<String> = Vec::new();
        builds.extend(dependencies.build.as_ref().map(|u| u.build.to_string()));
        builds.extend(a.bc_version.clone());
        builds.extend(apps::own_build(&inputs).map(|b| b.to_string()));
        builds.extend(dependencies.manifests.iter().map(|m| m.version.clone()));
        builds.extend(inputs.iter().map(|i| i.manifest.version.clone()));
        let mut downloadable: Vec<al_deps::build::BuildId> = Vec::new();
        let named = dependencies
            .build
            .as_ref()
            .map(|u| u.build.clone())
            .into_iter()
            .chain(
                a.bc_version
                    .as_deref()
                    .and_then(|v| al_deps::build::BuildId::parse(v).ok()),
            )
            .chain(apps::own_build(&inputs));
        for b in named {
            if !downloadable.contains(&b) {
                downloadable.push(b);
            }
        }
        // A default run with no Microsoft app has no use for the CRONUS company.
        let microsoft = inputs.iter().any(|i| apps::is_microsoft(&i.manifest.publisher))
            || dependencies.manifests.iter().any(|m| apps::is_microsoft(&m.publisher));
        if microsoft || a.test_data_asked_for() {
            let chosen_data = choose_test_data(
                a,
                &builds,
                &downloadable,
                chosen.target.bc.major,
                Some(chosen.target.bc.minor),
            )?;
            test_data = chosen_data.data;
            data_notes = chosen_data.notes;
        }
    }
    let mut bc_build = None;
    if let Some(u) = &dependencies.build {
        chosen.target.bc.build = Some(u.build.to_string());
        bc_build = Some(u.build.to_string());
        if from_packages && Some(&u.build) != preferred_build.as_ref() {
            // The build the packages come from is neither cached nor published: the usual rules chose
            // another one of the same major, and the target follows what the apps really are.
            build_rule.push_str(&format!(
                "; that build is not cached, on disk or published, so BC {} was used",
                u.build
            ));
            if u.build.major() == u32::from(chosen.target.bc.major) {
                chosen.target.bc.minor = u.build.minor() as u16;
            }
        }
    }
    // The resolver repeats a note on each round of the artifact feed; say each once.
    let mut seen = BTreeSet::new();
    let mut hidden = 0;
    for line in &dependencies.log {
        let is_problem = line.starts_with("dependency skipped")
            || line.starts_with("dependency note")
            || line.starts_with("warning:");
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
    for note in &data_notes {
        eprintln!("note: {note}");
    }

    if !a.quiet {
        println!("alrunner {} ({})", env!("CARGO_PKG_VERSION"), env!("ALRUNNER_COMMIT"));
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
        println!("build chosen by: {build_rule}");
        println!("dependencies: {} apps", dependencies.apps.len());
        println!(
            "threads: check {}, lower {}, test {}",
            threads("AL_CHECK_THREADS"),
            threads("AL_LOWER_THREADS"),
            threads("AL_TEST_THREADS")
        );
        println!("isolation: TestIsolation = Codeunit (the default of Microsoft's test runner)");
        if let Some((data, path)) = &test_data {
            println!(
                "test data: {} (company '{}', rows read when a test first touches a table)",
                path.display(),
                data.provider.company()
            );
        }
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
    let sidecar_host = host.clone();
    let opts_only = selection.active().then(|| {
        let mut v: Vec<(usize, u32)> = chosen_units.iter().map(|f| (f.app, f.id)).collect();
        v.sort_unstable();
        v.dedup();
        v
    });
    // Tests that look free of data are compiled first without the baseline and may need no .NET helper
    // at all: start the helpers only when the run turns out to need them (a session starts one then).
    let looks_pure = opts_only.is_some()
        && std::env::var_os("AL_FORCE_BASELINE").is_none()
        && al_testrun::observe::text_looks_pure(&all_apps, &kinds, &symbols, opts_only.as_deref());
    if !dependencies.apps.is_empty() && !looks_pure {
        // The Microsoft apps call .NET (Media, images). Start the helper processes now so the
        // starts overlap the compile; a run without them starts nothing (ADR-014 section 6).
        sidecar_host.prewarm(al_testrun::dotnet::STANDBY_SIDECARS);
    }
    let dotnet = al_vm::dotnet::DotNetConfig {
        host,
        ..Default::default()
    };
    let opts = RunOptions {
        only: opts_only,
        lazy: std::iter::repeat_n(None, inputs.len())
            .chain(dependencies.lazy.iter().cloned())
            .collect(),
        trace: a.explain.is_some() || std::env::var_os("AL_STACK_OF").is_some(),
        test_time_limit: match a.test_timeout_secs.unwrap_or(20) {
            0 => None,
            n => Some(std::time::Duration::from_secs(n)),
        },
        test_data: test_data.as_ref().map(|(d, _)| d.clone()),
        compile_cache: (!a.no_compile_cache
            && !std::env::var("ALRUNNER_NO_COMPILE_CACHE").is_ok_and(|v| !v.is_empty() && v != "0"))
        .then(|| al_testrun::compile_cache::CompileCache {
            dir: al_testrun::compile_cache::CompileCache::under(&al_deps::cache::cache_root_from_env(
                a.cache_dir.as_deref(),
            )),
            require: a.require_compile_cache,
        }),
    };
    let load_time = started.elapsed();
    let rt = CorpusRuntime::try_run_suites_with(
        all_apps,
        kinds,
        &manifests,
        &symbols,
        Some(&chosen.target),
        dotnet,
        &[SuiteRun::everything()],
        &opts,
    )?;
    let p = &rt.phases;
    let d = sidecar_host.stats();
    sidecar_host.shutdown();

    // The selected tests, in a stable order: app, codeunit, then declaration order.
    let mut results: Vec<&TestResult> = rt
        .results()
        .iter()
        .filter(|r| selection.picks(r.codeunit, &r.test))
        .collect();
    results.sort_by_key(|r| (r.app, r.codeunit));
    let counts = Counts::of(results.iter().copied());
    // A missing .NET helper stops tests for a reason that is the machine's, not the tests': say so
    // before the results and again with the summary.
    let dotnet_warning = results
        .iter()
        .find_map(|r| match &r.outcome {
            al_testrun::Outcome::NotRun(why) if why.starts_with(al_vm::dotnet::UNAVAILABLE) => Some(why.as_str()),
            _ => None,
        })
        .and_then(|why| output::dotnet_warning(&counts, why));
    if let Some(w) = &dotnet_warning {
        eprintln!("{w}");
    }

    // Debug aid: `AL_STACK_OF=TEXT` prints, for up to five not-run tests whose reason contains TEXT
    // (and, with `AL_STACK_NAME`, whose name contains it),
    // the routines they were refused in (as `--explain` does for one test).
    if let Some(text) = std::env::var_os("AL_STACK_OF").map(|t| t.to_string_lossy().to_string()) {
        for r in results
            .iter()
            .filter(|r| matches!(&r.outcome, Outcome::NotRun(w) if w.contains(&text)))
            .filter(|r| std::env::var("AL_STACK_NAME").map_or(true, |n| r.test.contains(&n)))
            .take(5)
        {
            let id = CaseId {
                codeunit: r.codeunit,
                name: r.test.clone(),
            };
            let head = format!("{} {}.{}", r.codeunit, r.codeunit_name, r.test);
            eprintln!(
                "{}",
                al_testrun::explain::render_without_oracle(&head, &r.outcome, rt.detail_in("", &id))
            );
        }
    }
    if let Some(pattern) = &a.explain {
        return explain(&rt, &results, pattern, &a.codeunits, &chosen.target.bc.to_string());
    }

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
        if let Some(line) = p.baseline_line() {
            eprintln!("{line}");
        }
        if let Some(cost) = test_data.as_ref().and_then(|(d, _)| d.provider.cost()) {
            eprintln!("test data: {cost}");
        }
        if let Some(c) = &p.cache {
            eprintln!(
                "compile cache: {}; {} {}",
                if c.hit {
                    format!("hit, opened in {}", secs(c.open))
                } else if c.built {
                    format!("miss, built in {}", secs(c.build))
                } else {
                    "not used".to_string()
                },
                c.file.display(),
                if c.notes.is_empty() {
                    String::new()
                } else {
                    format!("({})", c.notes.join("; "))
                }
            );
            for (i, l) in c.layers.iter().enumerate() {
                eprintln!(
                    "compile cache: layer {} of {} ({}): {}, {:.1} MB",
                    i + 1,
                    c.layers.len(),
                    l.apps.join(", "),
                    if l.hit {
                        "hit".to_string()
                    } else {
                        format!("built in {}", secs(l.build))
                    },
                    l.bytes as f64 / 1e6
                );
            }
            eprintln!(
                "compile cache: {:.1} MB, {} dependency objects, {} compiled again because the workspace extends or implements what they embed",
                c.bytes as f64 / 1e6,
                c.objects,
                c.relowered
            );
            eprintln!(
                "compile cache: reading the blocks {}",
                secs(p.compile_parts.cache_blocks)
            );
        }
        let slowest: Vec<String> = p
            .slowest
            .iter()
            .map(|c| {
                let start = if c.sidecar_start.is_zero() {
                    String::new()
                } else {
                    format!(" (sidecar start {})", secs(c.sidecar_start))
                };
                format!("{} {} {}{start}", c.id, c.name, secs(c.took))
            })
            .collect();
        eprintln!("slowest test codeunits: {}", slowest.join("; "));
        if d.started > 0 {
            eprintln!(
                "dotnet: {} .NET sidecar process(es) started: starting {} in total, {} at most; finding dotnet {}; sessions waited {} for theirs; {} request(s) {}",
                d.started,
                secs(d.start_total),
                secs(d.start_max),
                secs(d.find_total),
                secs(d.waited_total),
                d.calls,
                secs(d.call_total)
            );
        }
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
                    let mut line = format!(
                        "FAIL    {} {}.{}: {}",
                        r.codeunit,
                        r.codeunit_name,
                        r.test,
                        output::first_line(why, 300)
                    );
                    for frame in output::stack_lines(&r.call_stack) {
                        line.push_str(&format!("\n          {frame}"));
                    }
                    line
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

    if let Some(n) = a.top_failures.filter(|_| counts.fail > 0) {
        let list = refusals::rank(results.iter().filter_map(|r| match &r.outcome {
            Outcome::Fail(why) => Some((
                CaseId {
                    codeunit: r.codeunit,
                    name: r.test.clone(),
                },
                why.clone(),
            )),
            _ => None,
        }));
        println!("\n{}", refusals::render_top_failures("failed tests", &list, n));
    }

    if let Some(needle) = &a.refusal_detail {
        let list = refusals::detail(
            results.iter().filter_map(|r| match &r.outcome {
                Outcome::NotRun(why) => Some((
                    CaseId {
                        codeunit: r.codeunit,
                        name: r.test.clone(),
                    },
                    why.clone(),
                )),
                _ => None,
            }),
            needle,
        );
        println!(
            "\n{}",
            refusals::render_top(&format!("not-run tests containing `{needle}`"), &list, 30)
        );
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
    if let Some((data, _)) = &test_data {
        let summary = data.stats.summary();
        println!("{}", summary.line(&data.provider.source(), data.provider.company()));
        let refusals = summary.refusals();
        for line in refusals.iter().take(if a.verbose { usize::MAX } else { 10 }) {
            println!("{}", al_testrun::refusals::clip(line, 400));
        }
        if !a.verbose && refusals.len() > 10 {
            println!("... {} more refused tables (--verbose lists them)", refusals.len() - 10);
        }
    }
    println!("{}", output::summary_line(&counts, test_data.is_some()));
    if let Some(w) = &dotnet_warning {
        println!("{w}");
    }
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
            8 => "DOTNET UNAVAILABLE",
            _ => "ERROR",
        }
    );
    Ok(code)
}

/// `--explain PATTERN`: the one test the pattern means (a test named exactly so, else the only one
/// whose name contains it), with where it was refused or failed.
fn explain(
    rt: &CorpusRuntime,
    results: &[&TestResult],
    pattern: &str,
    codeunits: &[u32],
    target: &str,
) -> Result<u8, String> {
    // `--codeunit` narrows the candidates here; elsewhere it selects the whole codeunit.
    let named = |r: &&&TestResult| r.test.to_lowercase().contains(&pattern.to_lowercase());
    let narrowed: Vec<&&TestResult> = results
        .iter()
        .filter(|r| codeunits.is_empty() || codeunits.contains(&r.codeunit))
        .filter(named)
        .collect();
    let exact: Vec<&&TestResult> = narrowed
        .iter()
        .copied()
        .filter(|r| r.test.eq_ignore_ascii_case(pattern))
        .collect();
    let candidates = if exact.len() == 1 { exact } else { narrowed };
    let [r] = candidates.as_slice() else {
        if candidates.is_empty() {
            println!("--explain: no test matches `{pattern}`");
            return Ok(6);
        }
        let list: Vec<String> = candidates
            .iter()
            .take(10)
            .map(|r| format!("  {} {}.{}", r.codeunit, r.codeunit_name, r.test))
            .collect();
        return Err(format!(
            "--explain `{pattern}` matches {} tests; give a longer name or add --codeunit ID:\n{}",
            candidates.len(),
            list.join("\n")
        ));
    };
    let head = format!(
        "BC {target}: test {} in codeunit {} ({})",
        r.test, r.codeunit, r.codeunit_name
    );
    let id = CaseId {
        codeunit: r.codeunit,
        name: r.test.clone(),
    };
    println!(
        "{}",
        al_testrun::explain::render_without_oracle(&head, &r.outcome, rt.detail_in("", &id))
    );
    Ok(match r.outcome {
        Outcome::Pass => 0,
        Outcome::Fail(_) => 1,
        Outcome::NotRun(_) => 7,
    })
}
