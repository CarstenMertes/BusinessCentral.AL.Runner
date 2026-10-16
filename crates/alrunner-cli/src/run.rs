//! `alrunner test`: load the APPs and their dependencies, compile, run, report.

use crate::apps::{self, AppInput};
use crate::args::{DEFAULT_TOP, TestArgs};
use crate::capture;
use crate::coverage;
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

/// With `--output-json` and no file, stdout holds the JSON document and nothing else: the report goes to stderr.
static REPORT_TO_STDERR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `println!` for the human report.
macro_rules! say {
    ($($t:tt)*) => {
        if REPORT_TO_STDERR.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!($($t)*)
        } else {
            println!($($t)*)
        }
    };
}

/// A test codeunit found in an APP's source, before anything is compiled.
struct Found {
    app: usize,
    id: u32,
    tests: Vec<String>,
}

/// The newest BC major this runtime is verified against (the fallback when nothing names one).
const NEWEST_VERIFIED: u16 = 29;

/// Which tests a run is about.
#[derive(Default, Clone)]
struct Selection {
    codeunits: Vec<u32>,
    filters: Vec<String>,
    exact: Vec<String>,
    excludes: Vec<String>,
}

/// `Codeunit<ID>.<Method>`, the name AL Runner v2 reports and accepts.
fn qualified(codeunit: u32, test: &str) -> String {
    format!("Codeunit{codeunit}.{test}")
}

impl Selection {
    /// Whether the run is narrowed by anything.
    fn active(&self) -> bool {
        !self.includes_none() || !self.excludes.is_empty()
    }

    fn includes_none(&self) -> bool {
        self.codeunits.is_empty() && self.filters.is_empty() && self.exact.is_empty()
    }

    /// A pattern with a dot is matched against the qualified name, one without against the method name; any case,
    /// as a substring.
    fn matches(pattern: &str, codeunit: u32, test: &str) -> bool {
        let p = pattern.to_lowercase();
        if p.contains('.') {
            qualified(codeunit, test).to_lowercase().contains(&p)
        } else {
            test.to_lowercase().contains(&p)
        }
    }

    fn picks(&self, codeunit: u32, test: &str) -> bool {
        let included = self.includes_none()
            || self.codeunits.contains(&codeunit)
            || self.filters.iter().any(|f| Self::matches(f, codeunit, test))
            || self
                .exact
                .iter()
                .any(|e| e.eq_ignore_ascii_case(test) || e.eq_ignore_ascii_case(&qualified(codeunit, test)));
        included && !self.excludes.iter().any(|x| Self::matches(x, codeunit, test))
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

/// With `alrunner serve`: the backup opened last, kept with the tables read from it, for the next request.
type KeptBackup = ((PathBuf, Option<String>), TestData);
static KEPT_BACKUP: std::sync::Mutex<Option<KeptBackup>> = std::sync::Mutex::new(None);
static KEEP_BACKUPS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `alrunner serve` keeps the opened backup (and so the tables read from it) between requests.
pub fn keep_backups() {
    KEEP_BACKUPS.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Opens a backup file.
fn open_backup(a: &TestArgs, path: PathBuf) -> Result<(TestData, PathBuf), String> {
    if KEEP_BACKUPS.load(std::sync::atomic::Ordering::Relaxed) {
        let key = (path.clone(), a.test_data_company.clone());
        if let Some((k, d)) = KEPT_BACKUP.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if *k == key {
                return Ok((d.clone(), path));
            }
        }
        let opened = open_backup_fresh(a, path)?;
        *KEPT_BACKUP.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, opened.0.clone()));
        return Ok(opened);
    }
    open_backup_fresh(a, path)
}

fn open_backup_fresh(a: &TestArgs, path: PathBuf) -> Result<(TestData, PathBuf), String> {
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

/// The test time limit in seconds: `--test-timeout-secs`, else `ALRUNNER_TEST_TIMEOUT`, else AL Runner v2's
/// `AL_RUNNER_TEST_TIMEOUT_SEC`, else 20.
fn test_timeout_secs(a: &TestArgs) -> Result<u64, String> {
    if let Some(n) = a.test_timeout_secs {
        return Ok(n);
    }
    for var in ["ALRUNNER_TEST_TIMEOUT", "AL_RUNNER_TEST_TIMEOUT_SEC"] {
        if let Some(v) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
            return v
                .trim()
                .parse()
                .map_err(|_| format!("{var} needs a number of seconds, got `{v}`"));
        }
    }
    Ok(20)
}

pub fn test(a: &TestArgs) -> Result<u8, String> {
    test_with(a, None)
}

/// What the server hears for each test as it finishes: the event, and its capture as JSON.
pub type OnTest = Arc<dyn Fn(al_testrun::events::TestEvent, Option<serde_json::Value>) + Send + Sync>;

/// What `alrunner serve` keeps between requests, and how it hears about a request's run (ADR-028).
#[derive(Default)]
pub struct WarmCtx {
    pub runtime: al_testrun::runtime::Warm,
    /// Called for each selected test as it finishes.
    /// With a capture asked for, the second argument is its JSON for that test (`capture::json`).
    pub on_test: Option<OnTest>,
    /// Set from outside: the run stops after the current test.
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// What the last run came to.
    pub report: Option<Report>,
    /// Whether the last run used the compile of an earlier request.
    pub used: Option<al_testrun::runtime::WarmUse>,
}

/// The result of a run for a caller that is not the console.
pub struct Report {
    /// The `--output-json` document.
    pub doc: serde_json::Value,
    /// With coverage: the `--coverage-json` document.
    pub coverage: Option<serde_json::Value>,
    /// Per test of `doc["tests"]`, in its order: the text of the `Message` calls of a scratch run.
    pub messages: Vec<Vec<String>>,
    /// Per test of `doc["tests"]`: the name of its codeunit.
    pub object_names: Vec<String>,
    /// Per test of `doc["tests"]`: its capture as JSON (`capture::json`), when one was asked for.
    pub captured: Vec<Option<serde_json::Value>>,
}

/// Everything the compile depends on but the selection and the text of the workspace files: the workspace file names, the dependency apps, the symbols, the target.
fn warm_key(
    inputs: &[AppInput],
    dependencies: &al_testrun::deps::Dependencies,
    symbols: &BTreeSet<String>,
    target: &str,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in inputs {
        i.source.id.hash(&mut h);
        i.source.dependencies.hash(&mut h);
        // The names of the files, not their text: a change of text is the warm compile's to take in.
        for (path, _) in &i.source.files {
            path.hash(&mut h);
        }
        i.manifest.version.hash(&mut h);
    }
    for (app, lazy) in dependencies.apps.iter().zip(&dependencies.lazy) {
        app.id.hash(&mut h);
        app.files.len().hash(&mut h);
        match lazy {
            Some(l) => l.identity.hash(&mut h),
            None => {
                for (path, text) in &app.files {
                    path.hash(&mut h);
                    text.len().hash(&mut h);
                }
            }
        }
    }
    dependencies.summary.hash(&mut h);
    symbols.hash(&mut h);
    target.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn test_with(a: &TestArgs, mut warm: Option<&mut WarmCtx>) -> Result<u8, String> {
    let started = Instant::now();
    REPORT_TO_STDERR.store(
        matches!(a.output_json, Some(None)) || warm.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let test_timeout = test_timeout_secs(a)?;
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
    if let Some(Some(p)) = &a.output_json {
        write_file(p, "")?;
    }
    for p in [&a.coverage_out, &a.coverage_json].into_iter().flatten() {
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
        exact: a.exact.clone(),
        excludes: a.excludes.clone(),
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
        .filter(|f| f.tests.iter().any(|t| selection.picks(f.id, t)))
        .collect();
    if chosen_units.is_empty() && !a.execute {
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
        say!("alrunner {} ({})", env!("CARGO_PKG_VERSION"), env!("ALRUNNER_COMMIT"));
        say!(
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
            say!(
                "app: {} {} ({} files, {units} test codeunits, {tests} tests) from {}",
                app.json.name,
                app.manifest.version,
                app.source.files.len(),
                app.path.display()
            );
        }
        say!("build chosen by: {build_rule}");
        say!("dependencies: {} apps", dependencies.apps.len());
        say!(
            "threads: check {}, lower {}, test {}",
            threads("AL_CHECK_THREADS"),
            threads("AL_LOWER_THREADS"),
            threads("AL_TEST_THREADS")
        );
        match a.isolation {
            None | Some(al_testrun::isolation::TestIsolation::Codeunit) => {
                say!("isolation: TestIsolation = Codeunit (the default of Microsoft's test runner)")
            }
            Some(i) => say!("isolation: TestIsolation = {} (--isolation)", i.name()),
        }
        if let Some((data, path)) = &test_data {
            say!(
                "test data: {} (company '{}', rows read when a test first touches a table)",
                path.display(),
                data.provider.company()
            );
        }
        if selection.active() {
            say!("selection: {} test codeunits of {}", chosen_units.len(), found.len());
        }
    }

    // Compile and run.
    let build_apps = || {
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
        (all_apps, kinds, manifests)
    };
    let symbols: BTreeSet<String> = inputs
        .iter()
        .flat_map(|i| i.symbols.iter().cloned())
        .chain(a.defines.iter().cloned())
        .collect();

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
    let opts_only = (selection.active() && !a.execute).then(|| {
        let mut v: Vec<(usize, u32)> = chosen_units.iter().map(|f| (f.app, f.id)).collect();
        v.sort_unstable();
        v.dedup();
        v
    });
    // Tests that look free of data are compiled first without the baseline and may need no .NET helper
    // at all: start the helpers only when the run turns out to need them (a session starts one then).
    let looks_pure = warm.is_some() || {
        // A warm server starts no helper ahead: a session starts one when a test needs it.
        let (all_apps, kinds, _) = build_apps();
        opts_only.is_some()
            && std::env::var_os("AL_FORCE_BASELINE").is_none()
            && al_testrun::observe::text_looks_pure(&all_apps, &kinds, &symbols, opts_only.as_deref())
    };
    if !dependencies.apps.is_empty() && !looks_pure {
        // The Microsoft apps call .NET (Media, images). Start the helper processes now so the
        // starts overlap the compile; a run without them starts nothing (ADR-014 section 6).
        sidecar_host.prewarm(al_testrun::dotnet::STANDBY_SIDECARS);
    }
    let dotnet = al_vm::dotnet::DotNetConfig {
        host,
        ..Default::default()
    };
    let hooks = match warm.as_deref() {
        Some(w) => {
            let pick = selection.clone();
            let forward = w.on_test.clone();
            let cfg = a.capture;
            let paths: Vec<PathBuf> = inputs.iter().map(|i| i.path.clone()).collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            // Loop ids are not repeated within a request, whatever test they come from.
            let next_loop = Arc::new(std::sync::atomic::AtomicU32::new(0));
            al_testrun::events::Hooks {
                on_test: forward.map(|f| {
                    Arc::new(move |e: al_testrun::events::TestEvent| {
                        if pick.picks(e.codeunit, &e.test) {
                            let json = e.captured.as_ref().map(|c| {
                                let base = next_loop
                                    .fetch_add(c.captured.loops.len() as u32, std::sync::atomic::Ordering::SeqCst);
                                capture::json(c, cfg, base, &|app: usize, file: &str| {
                                    coverage::label(&paths[app], file, &cwd)
                                })
                            });
                            f(e, json)
                        }
                    }) as Arc<dyn Fn(al_testrun::events::TestEvent) + Send + Sync>
                }),
                cancel: w.cancel.clone(),
            }
        }
        None => Default::default(),
    };
    let opts = RunOptions {
        only: opts_only,
        lazy: std::iter::repeat_n(None, inputs.len())
            .chain(dependencies.lazy.iter().cloned())
            .collect(),
        trace: a.explain.is_some() || std::env::var_os("AL_STACK_OF").is_some(),
        test_time_limit: match test_timeout {
            0 => None,
            n => Some(std::time::Duration::from_secs(n)),
        },
        isolation: a.isolation,
        coverage: a.coverage,
        execute: a.execute,
        capture: a.capture,
        hooks,
        clock_starts: None,
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
    // A scratch run has a run of its own for every app; a test run has one for all of them.
    let suites: Vec<SuiteRun> = if a.execute {
        inputs
            .iter()
            .enumerate()
            .map(|(i, input)| SuiteRun {
                name: input.source.name.clone(),
                app: Some(i),
                runner: None,
            })
            .collect()
    } else {
        vec![SuiteRun::everything()]
    };
    let rt = match warm.as_deref_mut() {
        None => {
            let (all_apps, kinds, manifests) = build_apps();
            CorpusRuntime::try_run_suites_with(
                all_apps,
                kinds,
                &manifests,
                &symbols,
                Some(&chosen.target),
                dotnet,
                &suites,
                &opts,
            )?
        }
        Some(w) => {
            let target_key = format!("{:?} {}", chosen.target, a.execute);
            let key = warm_key(&inputs, &dependencies, &symbols, &target_key);
            let files: Vec<al_testrun::runtime::FileChange> = inputs
                .iter()
                .enumerate()
                .flat_map(|(app, i)| {
                    i.source
                        .files
                        .iter()
                        .map(move |(path, text)| al_testrun::runtime::FileChange { app, path, text })
                })
                .collect();
            let (rt, used) = w.runtime.run(
                &key,
                &files,
                build_apps,
                &symbols,
                Some(&chosen.target),
                dotnet,
                &suites,
                &opts,
            )?;
            w.used = Some(used);
            rt
        }
    };
    let p = &rt.phases;
    let d = sidecar_host.stats();
    sidecar_host.shutdown();

    // The selected tests, in a stable order: app, codeunit, then declaration order.
    let mut results: Vec<&TestResult> = rt
        .results()
        .iter()
        .filter(|r| a.execute || selection.picks(r.codeunit, &r.test))
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
            say!("{line}");
        }
        if hidden_failures > 0 {
            say!("... {hidden_failures} more failures (--verbose lists them, --junit has every message)");
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
        say!("\n{}", refusals::render_top("not-run tests", &list, n));
    }

    // The assumptions the run went through: the counts are kept for `--list-assumptions`; the ranking is printed
    // with --top-assumptions, or as a pointer when passes were flagged.
    {
        let rows = al_testrun::flagged::rank(
            results
                .iter()
                .map(|r| (matches!(r.outcome, Outcome::Pass), r.assumptions.as_slice())),
        );
        al_testrun::flagged::save_hits(&al_deps::cache::cache_root_from_env(a.cache_dir.as_deref()), &rows);
        if let Some(n) = a.top_assumptions {
            println!("\n{}", al_testrun::flagged::render_top(&rows, n));
        } else if counts.flagged > 0 && !a.quiet {
            println!(
                "\n{} passes ran through an unprobed assumption (flagged); --top-assumptions ranks the assumptions, --list-assumptions describes them.",
                counts.flagged
            );
        }
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
        say!("\n{}", refusals::render_top_failures("failed tests", &list, n));
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
        say!(
            "\n{}",
            refusals::render_top(&format!("not-run tests containing `{needle}`"), &list, 30)
        );
    }

    let mut coverage_doc: Option<String> = None;
    if a.coverage {
        let cwd = std::env::current_dir().unwrap_or_default();
        let labels = |app: usize, file: &str| coverage::label(&inputs[app].path, file, &cwd);
        let names: Vec<String> = inputs.iter().map(|i| i.json.name.clone()).collect();
        let statements = rt.coverage_statements();
        let cwd_text = cwd.to_string_lossy().replace('\\', "/");
        // A server request gets its coverage in the summary and writes a file only when it names one.
        if warm.is_none() || a.coverage_out.is_some() {
            let cobertura_path = a
                .coverage_out
                .clone()
                .unwrap_or_else(|| PathBuf::from("coverage.cobertura.xml"));
            write_file(
                &cobertura_path,
                &coverage::cobertura(statements, &results, &names, &labels, &cwd_text),
            )?;
            say!(
                "coverage: {} statements of {} app(s); Cobertura in {}",
                statements.len(),
                inputs.len(),
                cobertura_path.display()
            );
        }
        if a.coverage_json.is_some() || warm.is_some() {
            coverage_doc = Some(coverage::json(statements, &results, &labels));
        }
        if let (Some(path), Some(doc)) = (&a.coverage_json, &coverage_doc) {
            write_file(path, doc)?;
        }
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
        say!("{}", summary.line(&data.provider.source(), data.provider.company()));
        let refusals = summary.refusals();
        for line in refusals.iter().take(if a.verbose { usize::MAX } else { 10 }) {
            say!("{}", al_testrun::refusals::clip(line, 400));
        }
        if !a.verbose && refusals.len() > 10 {
            say!("... {} more refused tables (--verbose lists them)", refusals.len() - 10);
        }
    }
    say!("{}", output::summary_line(&counts, test_data.is_some()));
    if let Some(w) = &dotnet_warning {
        say!("{w}");
    }
    say!(
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
    say!(
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
    if let Some(w) = warm {
        w.report = Some(Report {
            doc: serde_json::from_str(&output::json(&results, code, total_time.as_secs_f64())).unwrap_or_default(),
            coverage: coverage_doc.as_deref().and_then(|d| serde_json::from_str(d).ok()),
            messages: results.iter().map(|r| r.messages.clone()).collect(),
            object_names: results.iter().map(|r| r.codeunit_name.clone()).collect(),
            captured: {
                let cwd = std::env::current_dir().unwrap_or_default();
                let mut base = 0u32;
                results
                    .iter()
                    .map(|r| {
                        r.captured.as_ref().map(|c| {
                            let json = capture::json(c, a.capture, base, &|app: usize, file: &str| {
                                coverage::label(&inputs[app].path, file, &cwd)
                            });
                            base += c.captured.loops.len() as u32;
                            json
                        })
                    })
                    .collect()
            },
        });
    }
    if let Some(target) = &a.output_json {
        let doc = output::json(&results, code, total_time.as_secs_f64());
        match target {
            Some(path) => write_file(path, &doc)?,
            None => println!("{doc}"),
        }
    }
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
            say!("--explain: no test matches `{pattern}`");
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
    say!(
        "{}",
        al_testrun::explain::render_without_oracle(&head, &r.outcome, rt.detail_in("", &id))
    );
    if !r.assumptions.is_empty() {
        println!(
            "{}",
            al_testrun::flagged::render_explain(&r.assumptions, matches!(r.outcome, Outcome::Pass))
        );
    }
    Ok(match r.outcome {
        Outcome::Pass => 0,
        Outcome::Fail(_) => 1,
        Outcome::NotRun(_) => 7,
    })
}
