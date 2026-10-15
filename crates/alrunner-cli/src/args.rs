//! The command line of `alrunner` (ADR-016 section 2): parsing and the help text.

use std::path::PathBuf;

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Test(Box<TestArgs>),
    /// `alrunner serve`: the options every request's `alrunner test` run gets (ADR-028).
    Serve(Vec<String>),
    /// `alrunner warm`: find (or download) the demo backup and build its index.
    Warm(Box<TestArgs>),
}

/// `alrunner test`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TestArgs {
    /// The apps whose tests run: an app folder (with `app.json`), a `*.Source.zip` or an `.app`.
    pub apps: Vec<PathBuf>,
    /// `--dep PATH`: a package file, an app folder, or a folder of packages.
    pub deps: Vec<PathBuf>,
    /// `--package-cache DIR`: a folder of `.app` files.
    pub package_caches: Vec<PathBuf>,
    pub bc_version: Option<String>,
    pub no_download: bool,
    pub cache_dir: Option<PathBuf>,
    /// `--no-compile-cache`: compile the dependency apps on every run, only what the tests reach.
    pub no_compile_cache: bool,
    /// `--require-compile-cache`: a run without a usable cache entry is an error (CI).
    pub require_compile_cache: bool,
    /// `--codeunit ID`: select a test codeunit by object id.
    pub codeunits: Vec<u32>,
    /// `--filter TEXT`: select the tests whose name contains TEXT (any case).
    pub filters: Vec<String>,
    pub junit: Option<PathBuf>,
    /// `--output-json [FILE]`: the v2-shaped JSON document. `Some(None)`: on stdout, and the human report goes to
    /// stderr. `Some(Some(file))`: written to the file, the report stays on stdout.
    pub output_json: Option<Option<PathBuf>>,
    /// `--isolation codeunit|test|disabled`: the isolation of every test codeunit (default: Codeunit).
    pub isolation: Option<al_testrun::isolation::TestIsolation>,
    /// `--exclude-test TEXT`: tests whose name matches are left out of the selection. Repeatable.
    pub excludes: Vec<String>,
    /// `--test-exact NAME`: the test whose method name or `Codeunit<ID>.<Method>` is exactly NAME. Repeatable.
    pub exact: Vec<String>,
    /// `--define SYMBOL`: a preprocessor symbol for the apps. Repeatable. `--preprocessor-symbols A,B` adds several.
    pub defines: Vec<String>,
    /// `--coverage`: count the statements of the APPs each test runs.
    pub coverage: bool,
    /// `--coverage-out FILE`: write Cobertura XML there (implies `--coverage`; default `coverage.cobertura.xml`).
    pub coverage_out: Option<PathBuf>,
    /// `--coverage-json FILE`: write the statements and the statements of each test as JSON (implies `--coverage`).
    pub coverage_json: Option<PathBuf>,
    /// `--explain TEST`: run one test and say where it was refused or failed.
    pub explain: Option<String>,
    /// `--top-refusals [N]`: how many refusal reasons to list (default 10).
    pub top_refusals: Option<usize>,
    /// `--top-failures [N]`: how many failure messages to list, ranked (default 10).
    pub top_failures: Option<usize>,
    /// `--refusal-detail TEXT`: the reasons that contain TEXT, grouped finer than `--top-refusals`.
    pub refusal_detail: Option<String>,
    pub allow_not_run: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub timings: bool,
    pub dotnet_sidecar: Option<PathBuf>,
    pub dotnet_assembly_dirs: Vec<PathBuf>,
    pub dotnet_allow_roll_forward: bool,
    pub dotnet_timeout_secs: Option<u64>,
    /// `--test-timeout-secs N`: a test that uses more CPU time is stopped and not run (default 20; 0: no limit).
    pub test_timeout_secs: Option<u64>,
    pub require_dotnet: bool,
    pub files_dir: Option<PathBuf>,
    /// `--environment saas|onprem`: what Environment Information answers (default saas, as probed).
    pub environment: Option<al_host::Environment>,
    /// `--test-data [PATH]`: restore a CRONUS company from a BC backup (ADR-020).
    pub test_data: bool,
    /// `--no-test-data`: run without company data. Test data is on by default (ADR-020, owner
    /// decision of 2026-10-14).
    pub no_test_data: bool,
    /// The `.bak` or `.bacpac` named; else the cached artifact for the selected BC build.
    pub test_data_path: Option<PathBuf>,
    /// `--test-data-company NAME`: which company of the backup (default: the demo company).
    pub test_data_company: Option<String>,
}

impl TestArgs {
    /// Whether the run restores a company: unless `--no-test-data` says otherwise.
    pub fn wants_test_data(&self) -> bool {
        !self.no_test_data
    }

    /// Whether the command line asked for test data (a backup or `--test-data`), so a failure to
    /// find it stops the run instead of turning the data off with a message.
    pub fn test_data_asked_for(&self) -> bool {
        self.test_data || self.test_data_path.is_some()
    }
}

/// Whether `v` names a database backup, which is how `--test-data PATH` tells a path from an APP.
fn is_backup(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    v.ends_with(".bak") || v.ends_with(".bacpac")
}

pub const DEFAULT_TOP: usize = 10;

pub const HELP: &str = "\
alrunner: run AL tests without a service tier (AL Runner v3, preview)

USAGE
    alrunner test [OPTIONS] APP...
    alrunner serve [--protocol 2] [OPTIONS]
    alrunner --version | --help

APP is an app folder (with app.json), a source archive (`Tests-SMB.Source.zip`) or an `.app`
that carries its source. Every `Subtype = Test` codeunit of every APP runs, each in its own
session, under TestIsolation = Codeunit (the default of Microsoft's test runner).

DEPENDENCIES
    Looked up for the closure of the APPs' app.json dependencies, in this order:
    --dep, --package-cache, each APP folder's .alpackages, then Microsoft's artifact feed.
    --dep PATH           A package file (.app, *.Source.zip), an app folder or a folder of
                         packages. Repeatable.
    --package-cache DIR  A folder of .app files. Repeatable.
    --bc-version V       BC major, major.minor or an exact build (four numbers). An exact build
                         also fills Microsoft's $(app_...) variables in source archives that
                         lie outside a folder named for their build.
    --no-download        Never fetch from the artifact feed (apps, System.app, demo data).
    Microsoft's apps, and the platform's System.app, come from the artifact feed. Without
    --bc-version they come from the build of the tests when the tests are Microsoft's (a
    Tests-*.Source.zip or .app in a folder named for its build, or with the build as its
    version); if that build is gone from the feed the newest match is used and the run says so.
    A build named exactly (four numbers) that is in ~/.bcartifacts.cache/sandbox is read from
    there, without the feed; any matching build there is the fallback when the feed has none or
    is offline.
    --cache-dir DIR      Where downloaded artifacts are kept (else $ALRUNNER_CACHE).

COMPILE CACHE
    The Microsoft apps and the other dependency apps are compiled completely once, and the result
    is kept in <cache dir>/compile/ (about 20 MB per set of apps). Later runs with the same apps,
    versions, sources, target and the same alrunner build load it instead of compiling them.
    The first run of a new set pays for the build (about 10 s for Business Central).
    --no-compile-cache   Compile the dependency apps on every run, only what the tests reach.
                         Same as ALRUNNER_NO_COMPILE_CACHE=1.
    --require-compile-cache  Fail when there is no usable cache entry instead of building one (CI).

SELECTION (a test is selected when any option matches it)
    --codeunit ID        A test codeunit by object id. Repeatable.
    --filter TEXT        Tests whose name contains TEXT, any case. Repeatable.
    --explain TEST       Run one test (its name, or a part of it that only one test has) and print
                         its outcome and, for a failure or a refusal, the routines and source
                         lines it left from. Selects that test like --filter.
    A selected test's codeunit runs whole (tests of a codeunit share state); only the selected
    tests are reported. Only the selected codeunits are compiled.

OUTPUT
    Default: header, one line per failing test with the AL call stack of its error below it (a
    frame whose format is not known reads as such), the top refusal reasons, a summary.
    --junit also holds the stack as the text of each <failure>.
    --verbose            One line per test (PASS, FAIL, NOT RUN with its reason).
    --quiet              Summary only.
    --top-refusals [N]   List the N most common reasons tests were not run (default 10).
    --top-failures [N]   List the N most common failure messages (default 10), numbers and quoted
                         values folded.
    --refusal-detail TEXT  List the not-run reasons that contain TEXT (site and call arguments
                         folded, names kept), most common first (default 30 lines).
    --junit FILE         Write JUnit XML. A test that did not run is <skipped> with its reason.
    --output-json [FILE] Write one JSON document of the shape of AL Runner v2: `tests[]` with
                         `name` (`Codeunit<ID>.<Method>`), `status` (`pass`, `fail`, `error`),
                         `durationMs`, `message`, `stackTrace`, and `passed`, `failed`, `errors`,
                         `skipped`, `total`, `notRun`, `exitCode`, `wallSeconds`. A test that did not
                         run is `error` with the message `NotRun: out-of-scope: <reason>`; one the
                         time limit stopped is `error` with `Test exceeded <N>s timeout.`. Without
                         FILE the document is the only thing on stdout and the report goes to
                         stderr. `--output-json=FILE` also works. See docs/integrations/migrating-tools-to-v3.md.
    --timings            Compile phases and the slowest test codeunits.

COVERAGE
    --coverage           Count how often each statement of the APPs runs, per test. Writes Cobertura XML to
                         `coverage.cobertura.xml` (see --coverage-out). Off, a run pays one branch per statement.
    --coverage-out FILE  Write the Cobertura XML to FILE (implies --coverage). Files are named relative to
                         the working directory when they lie below it; a line has the runs of the
                         statements that start on it.
    --coverage-json FILE Write a JSON document with `coverage` ([{file, statements: [{id, scope, line,
                         column, endLine, hits}]}]) and `perTestCoverage` ([{test, coverage: [...]}]) (implies --coverage): every statement with
                         its runs, and for each test the statements it ran. File names are absolute with
                         forward slashes (`archive.zip!/inner/path.al` inside a package). Statements are the
                         ones the lowering marks, one per AL statement.

ISOLATION
    --isolation MODE     `codeunit` (default): the database is rolled back when a test codeunit ends.
                         `test` (also `function`, `method`): it is rolled back to its state after OnRun
                         after every test method; the codeunit's AL variables are kept, as BC's
                         TestIsolation = Function does. `disabled`: not pinned, so every test is
                         reported as not run.

EXIT CODES
    0  every selected test passed
    1  a test failed
    2  bad invocation or the run could not start
    6  the selection matched no test
    7  some tests were not run (a refusal); --allow-not-run makes this 0
    8  some tests were not run because the .NET helper could not be used (not found, no .NET 8
       runtime): a problem of the machine; --allow-not-run makes this 0
    Order when several apply: 2, 6, 1, 8, 7.

SERVE
    alrunner serve       AL Runner v2's server protocol (ALchemist, LethAL): one JSON object per line on
                         stdin, answers on stdout (a ready line first), logs on stderr. Commands:
                         runTests, cancel, shutdown (execute: later). Each runTests request runs
                         `alrunner test` as a child process, so nothing is kept between requests.
                         The options are those of `test` that do not depend on a request (--package-cache,
                         --define, --test-timeout, --bc-version, --cache-dir, --no-download, --dep, ...);
                         the request carries sourcePaths, packagePaths, coverage, perTestCoverage, test,
                         excludeTests, testIsolation. See docs/adr/028-alrunner-serve.md.

WARM
    alrunner warm [PATH.bak] [--bc-version BUILD] [--cache-dir DIR] [--no-download]
                  [--test-data-company NAME]
        Finds the demo backup the way `test` does (downloading it if need be), opens it and
        keeps its page map and catalog in the cache. The first open of a backup verifies the
        whole file (2 to 3 s on a 900 MB backup); later opens take milliseconds. Run it once
        before a demo on a new machine.

TEST DATA
    On by default for a run that uses Microsoft's apps: the CRONUS demo company of the build is
    restored. Microsoft's tests expect it, and many of them fail on missing setup data without it.
    A table is read the first time a test touches it; the rest is never read. The backup is found
    in this order: the build of the Microsoft apps in ~/.bcartifacts.cache/sandbox/<build>/w1,
    then the artifact cache, then it is downloaded once from the artifact feed (about 520 MB,
    progress on stderr) into the cache, then a backup of another build of the same BC version
    (the line says which). If none can be had, test data is off and the run says why.
    --no-test-data       Run without company data.
    --test-data [PATH]   Name a backup (.bak or .bacpac), or insist on test data: a missing
                         backup then stops the run. Write `--test-data=PATH` or end the name in
                         .bak/.bacpac: a bare name after --test-data is an APP.
    --test-data-company NAME  The company to restore (exact name or prefix). Default: the only
                         company, else 'CRONUS International Ltd.'.
    The summary says what was loaded and every exclusion: tables the runtime could not rebuild,
    tables the reader failed on (with its text), and columns left out.

OTHER
    --environment ENV    The server Environment Information describes: `saas` (default; the probed
                         sandbox: IsSaaS and IsSandbox Yes) or `onprem` (IsSaaS and IsSandbox No
                         until a test library's SetTestability* calls change them; Microsoft's own
                         test suites expect this, for example codeunit 134614). The on-prem values
                         are an unprobed assumption (docs/probes-needed.md).
    --files-dir DIR      Let the File data type read and write real files under DIR.
    --define SYMBOL      A preprocessor symbol (`#if SYMBOL`) for the APPs. Repeatable.
                         `--preprocessor-symbols A,B` names several.
    --auto-provision     Accepted and ignored (v2 downloaded platform apps with it; v3 always does,
                         unless --no-download).
    --test-timeout-secs N  (`--test-timeout N` is the same) A test that uses more than N seconds of CPU time (default 20; the
                         load of the machine does not count; a test that hangs without
                         computing is stopped at 10 times N of wall time) is stopped at its
                         next loop iteration and reported as not run, with the limit as the
                         reason; the codeunit's later failures are then not trusted. 0: no limit.
    --dotnet-sidecar PATH, --dotnet-assembly-dir DIR, --dotnet-allow-roll-forward,
    --dotnet-timeout-secs N, --require-dotnet     The optional .NET helper (ADR-014).

ENVIRONMENT
    ALRUNNER_TEST_TIMEOUT, AL_RUNNER_TEST_TIMEOUT_SEC  The test time limit in seconds when
    --test-timeout-secs is not given (the second is AL Runner v2's name).
    AL_CHECK_THREADS, AL_LOWER_THREADS, AL_TEST_THREADS  Worker threads of the check, the
    lowering and the test codeunits (default: one per core).
";

fn number(flag: &str, v: &str) -> Result<usize, String> {
    v.parse().map_err(|_| format!("{flag} needs a number, got `{v}`"))
}

/// `alrunner warm [PATH.bak] [--bc-version V] [--cache-dir DIR] [--no-download] [--test-data-company NAME]`.
fn parse_warm(mut it: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut a = TestArgs {
        test_data: true,
        ..TestArgs::default()
    };
    while let Some(flag) = it.next() {
        let mut text = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--bc-version" => a.bc_version = Some(text()?),
            "--cache-dir" => a.cache_dir = Some(text()?.into()),
            "--no-download" => a.no_download = true,
            "--test-data-company" => a.test_data_company = Some(text()?),
            f if !f.starts_with("--") && is_backup(f) => a.test_data_path = Some(PathBuf::from(f)),
            f => {
                return Err(format!(
                    "unknown option or argument `{f}` for `alrunner warm` (see --help)"
                ));
            }
        }
    }
    if a.test_data_path.is_none() && a.bc_version.is_none() {
        return Err("alrunner warm needs a backup (PATH.bak) or a build (--bc-version BUILD)".into());
    }
    Ok(Command::Warm(Box::new(a)))
}

/// `alrunner serve [--protocol 2] [OPTIONS]`: the options are those of `alrunner test` that do not depend on a request;
/// they are checked with its parser and handed to every run.
fn parse_serve(args: Vec<String>) -> Result<Command, String> {
    let mut daemon = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--protocol" => match it.next().as_deref() {
                Some("2") => {}
                other => {
                    return Err(format!(
                        "--protocol: only protocol 2 exists, not `{}`",
                        other.unwrap_or("(nothing)")
                    ));
                }
            },
            _ => daemon.push(a),
        }
    }
    // A throw-away parse tells a good option from a bad one, and keeps out what only a request decides.
    let mut probe = vec!["test".to_string(), "APP".to_string()];
    probe.extend(daemon.iter().cloned());
    let Command::Test(t) = parse(probe)? else {
        return Err("alrunner serve: options of `alrunner test` only".into());
    };
    let t = *t;
    if t.apps.len() != 1
        || !t.codeunits.is_empty()
        || !t.filters.is_empty()
        || !t.excludes.is_empty()
        || !t.exact.is_empty()
        || t.junit.is_some()
        || t.output_json.is_some()
        || t.coverage
        || t.explain.is_some()
        || t.isolation.is_some()
    {
        return Err("alrunner serve takes only the options that do not depend on a request: --package-cache, --dep, --define, --preprocessor-symbols, --test-timeout, --bc-version, --cache-dir, --no-download, --test-data and the like; the rest comes in each `runTests` request".into());
    }
    Ok(Command::Serve(daemon))
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut it = args.into_iter().peekable();
    let Some(first) = it.next() else {
        return Ok(Command::Help);
    };
    match first.as_str() {
        "--help" | "-h" | "help" => return Ok(Command::Help),
        "--version" | "-V" | "version" => return Ok(Command::Version),
        "test" => {}
        "warm" => return parse_warm(it),
        "serve" => return parse_serve(it.collect()),
        "run" | "debug" | "deps" | "package" | "helper" | "doctor" | "migrate" | "guide" => {
            return Err(format!(
                "`alrunner {first}` is not implemented yet (ADR-016 lists it for a later step); only `alrunner test` exists"
            ));
        }
        other => return Err(format!("unknown command `{other}` (see --help)")),
    }
    let mut a = TestArgs::default();
    while let Some(flag) = it.next() {
        if matches!(flag.as_str(), "--help" | "-h") {
            return Ok(Command::Help);
        }
        if flag == "--top-refusals" {
            // The count is optional: `--top-refusals` or `--top-refusals 5`.
            let n = match it.peek() {
                Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                    let v = it.next().unwrap_or_default();
                    let n = number("--top-refusals", &v)?;
                    if n == 0 {
                        return Err("--top-refusals needs a count of at least 1".into());
                    }
                    n
                }
                _ => DEFAULT_TOP,
            };
            a.top_refusals = Some(n);
            continue;
        }
        if flag == "--top-failures" {
            let n = match it.peek() {
                Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                    let v = it.next().unwrap_or_default();
                    let n = number("--top-failures", &v)?;
                    if n == 0 {
                        return Err("--top-failures needs a count of at least 1".into());
                    }
                    n
                }
                _ => DEFAULT_TOP,
            };
            a.top_failures = Some(n);
            continue;
        }
        if flag == "--output-json" {
            // The file is optional and an APP is positional: only a name that ends in `.json` is the file.
            a.output_json = Some(match it.peek() {
                Some(v) if v.to_ascii_lowercase().ends_with(".json") => it.next().map(PathBuf::from),
                _ => None,
            });
            continue;
        }
        if let Some(path) = flag.strip_prefix("--output-json=") {
            a.output_json = Some(Some(PathBuf::from(path)));
            continue;
        }
        if flag == "--test-data" {
            // The path is optional and an APP is positional, so only a name that ends in a
            // backup's extension is taken as the path: `--test-data Tests.Source.zip` is an APP.
            a.test_data = true;
            if it.peek().is_some_and(|v| is_backup(v)) {
                a.test_data_path = it.next().map(PathBuf::from);
            }
            continue;
        }
        if let Some(path) = flag.strip_prefix("--test-data=") {
            a.test_data = true;
            a.test_data_path = Some(PathBuf::from(path));
            continue;
        }
        if !flag.starts_with("--") {
            a.apps.push(PathBuf::from(flag));
            continue;
        }
        let mut text = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--dep" => a.deps.push(text()?.into()),
            "--package-cache" => a.package_caches.push(text()?.into()),
            "--bc-version" => a.bc_version = Some(text()?),
            "--no-download" => a.no_download = true,
            "--no-test-data" => a.no_test_data = true,
            "--cache-dir" => a.cache_dir = Some(text()?.into()),
            "--no-compile-cache" => a.no_compile_cache = true,
            "--require-compile-cache" => a.require_compile_cache = true,
            "--codeunit" => {
                let v = text()?;
                a.codeunits
                    .push(v.parse().map_err(|_| format!("{flag} needs an object id, got `{v}`"))?);
            }
            "--filter" | "--test" => a.filters.push(text()?),
            "--coverage" => a.coverage = true,
            "--coverage-out" => {
                a.coverage = true;
                a.coverage_out = Some(text()?.into());
            }
            "--coverage-json" => {
                a.coverage = true;
                a.coverage_json = Some(text()?.into());
            }
            "--test-exact" => a.exact.push(text()?),
            "--exclude-test" => a.excludes.push(text()?),
            "--define" => a.defines.push(text()?),
            "--preprocessor-symbols" => a.defines.extend(
                text()?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from),
            ),
            "--auto-provision" => {}
            "--isolation" | "--test-isolation" => {
                let v = text()?;
                a.isolation = Some(match v.to_ascii_lowercase().as_str() {
                    "codeunit" => al_testrun::isolation::TestIsolation::Codeunit,
                    "test" | "function" | "method" => al_testrun::isolation::TestIsolation::Function,
                    "disabled" => al_testrun::isolation::TestIsolation::Disabled,
                    _ => return Err(format!("{flag} needs codeunit, test or disabled, got `{v}`")),
                });
            }
            "--refusal-detail" => a.refusal_detail = Some(text()?),
            "--junit" => a.junit = Some(text()?.into()),
            "--explain" => a.explain = Some(text()?),
            "--allow-not-run" => a.allow_not_run = true,
            "--verbose" | "-v" => a.verbose = true,
            "--quiet" | "-q" => a.quiet = true,
            "--timings" => a.timings = true,
            "--files-dir" => a.files_dir = Some(text()?.into()),
            "--environment" => a.environment = Some(text()?.parse().map_err(|e| format!("--environment: {e}"))?),
            "--dotnet-sidecar" => a.dotnet_sidecar = Some(text()?.into()),
            "--dotnet-assembly-dir" => a.dotnet_assembly_dirs.push(text()?.into()),
            "--dotnet-allow-roll-forward" => a.dotnet_allow_roll_forward = true,
            "--require-dotnet" => a.require_dotnet = true,
            "--dotnet-timeout-secs" => {
                let v = text()?;
                a.dotnet_timeout_secs = Some(number(&flag, &v)? as u64);
            }
            "--test-timeout-secs" | "--test-timeout" => {
                let v = text()?;
                a.test_timeout_secs = Some(number(&flag, &v)? as u64);
            }
            "--test-data-company" => a.test_data_company = Some(text()?),
            "--test-data-normalize-company" => {
                return Err(format!(
                    "{flag} is not available: AL Runner v2's company normalization rules are not ported (ADR-020)"
                ));
            }
            "--watch" | "--affected" | "--tdd" | "--per-suite" | "--include-failing" | "--strict-environment" => {
                return Err(format!(
                    "{flag} is deferred in v3 (ADR-016, owner direction 2026-10-13): features that only hide start-up or run cost wait until they are measured to matter"
                ));
            }
            _ => return Err(format!("unknown option `{flag}` (see --help)")),
        }
    }
    if a.apps.is_empty() {
        return Err("alrunner test needs at least one APP (an app folder or a *.Source.zip)".into());
    }
    if a.require_compile_cache && a.no_compile_cache {
        return Err("--require-compile-cache and --no-compile-cache cannot be combined".into());
    }
    if a.no_test_data && a.test_data_asked_for() {
        return Err("--test-data and --no-test-data cannot be combined".into());
    }
    if a.no_test_data && a.test_data_company.is_some() {
        return Err("--test-data-company cannot be combined with --no-test-data".into());
    }
    if a.verbose && a.quiet {
        return Err("--verbose and --quiet cannot be combined".into());
    }
    Ok(Command::Test(Box::new(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(s.split_whitespace().map(String::from))
    }

    fn test_args(s: &str) -> TestArgs {
        match p(s).unwrap() {
            Command::Test(a) => *a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn warm_takes_a_backup_or_a_build_and_nothing_else() {
        match p("warm x.bak --test-data-company TP --cache-dir c").unwrap() {
            Command::Warm(a) => {
                assert_eq!(a.test_data_path, Some(PathBuf::from("x.bak")));
                assert_eq!(a.test_data_company.as_deref(), Some("TP"));
                assert_eq!(a.cache_dir, Some(PathBuf::from("c")));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            p("warm --bc-version 27.5.46862.48827 --no-download").unwrap(),
            Command::Warm(_)
        ));
        assert!(p("warm").unwrap_err().contains("--bc-version"));
        assert!(p("warm App").unwrap_err().contains("unknown"));
    }

    #[test]
    fn no_arguments_and_help_print_the_help() {
        assert_eq!(p("").unwrap(), Command::Help);
        assert_eq!(p("--help").unwrap(), Command::Help);
        assert_eq!(p("test --help").unwrap(), Command::Help);
        assert_eq!(p("--version").unwrap(), Command::Version);
    }

    #[test]
    fn test_takes_apps_and_repeatable_options() {
        let a = test_args(
            "test A B.Source.zip --dep d1 --dep d2 --package-cache pc --codeunit 5 --codeunit 6 --filter x --test y",
        );
        assert_eq!(a.apps, vec![PathBuf::from("A"), PathBuf::from("B.Source.zip")]);
        assert_eq!(a.deps.len(), 2);
        assert_eq!(a.package_caches, vec![PathBuf::from("pc")]);
        assert_eq!(a.codeunits, vec![5, 6]);
        assert_eq!(a.filters, vec!["x", "y"]);
    }

    #[test]
    fn the_flags_of_the_v2_tools_parse() {
        use al_testrun::isolation::TestIsolation as I;
        let a = test_args(
            "test A --isolation test --exclude-test Codeunit1.X --exclude-test Y --test-exact Codeunit1.Z --define FOO \
             --preprocessor-symbols A,B --auto-provision --test-timeout 7 --test Codeunit1.Z",
        );
        assert_eq!(a.isolation, Some(I::Function));
        assert_eq!(a.excludes, vec!["Codeunit1.X", "Y"]);
        assert_eq!(a.exact, vec!["Codeunit1.Z"]);
        assert_eq!(a.defines, vec!["FOO", "A", "B"]);
        assert_eq!(a.test_timeout_secs, Some(7));
        assert_eq!(a.filters, vec!["Codeunit1.Z"]);
        assert_eq!(test_args("test A --isolation codeunit").isolation, Some(I::Codeunit));
        assert_eq!(
            test_args("test A --test-isolation disabled").isolation,
            Some(I::Disabled)
        );
        assert!(
            p("test A --isolation nope")
                .unwrap_err()
                .contains("codeunit, test or disabled")
        );
    }

    #[test]
    fn serve_takes_the_options_that_do_not_depend_on_a_request() {
        match p("serve --protocol 2 --package-cache pc --define S --test-timeout 5 --no-download").unwrap() {
            Command::Serve(d) => assert_eq!(
                d,
                [
                    "--package-cache",
                    "pc",
                    "--define",
                    "S",
                    "--test-timeout",
                    "5",
                    "--no-download"
                ]
            ),
            other => panic!("{other:?}"),
        }
        assert!(matches!(p("serve").unwrap(), Command::Serve(_)));
        assert!(p("serve --protocol 1").unwrap_err().contains("only protocol 2"));
        assert!(p("serve --nope").unwrap_err().contains("unknown option"));
        assert!(p("serve --test Codeunit1.X").unwrap_err().contains("do not depend"));
        assert!(p("serve --coverage").unwrap_err().contains("do not depend"));
        assert!(p("serve OtherApp").unwrap_err().contains("do not depend"));
    }

    #[test]
    fn coverage_flags_turn_coverage_on() {
        assert!(!test_args("test A").coverage);
        assert!(test_args("test A --coverage").coverage);
        let a = test_args("test A --coverage-out c.xml --coverage-json c.json");
        assert!(a.coverage);
        assert_eq!(a.coverage_out, Some(PathBuf::from("c.xml")));
        assert_eq!(a.coverage_json, Some(PathBuf::from("c.json")));
    }

    #[test]
    fn output_json_takes_a_file_only_when_the_name_ends_in_json() {
        assert_eq!(test_args("test A --output-json").output_json, Some(None));
        assert_eq!(
            test_args("test A --output-json r.json").output_json,
            Some(Some(PathBuf::from("r.json")))
        );
        assert_eq!(
            test_args("test A --output-json=out/r.txt").output_json,
            Some(Some(PathBuf::from("out/r.txt")))
        );
        let a = test_args("test --output-json A");
        assert_eq!(a.output_json, Some(None));
        assert_eq!(a.apps, vec![PathBuf::from("A")]);
    }

    #[test]
    fn refusal_detail_takes_a_text() {
        assert_eq!(
            test_args("test A --refusal-detail is_not_lowered")
                .refusal_detail
                .as_deref(),
            Some("is_not_lowered")
        );
    }

    #[test]
    fn top_failures_count_is_optional_like_top_refusals() {
        assert_eq!(test_args("test A --top-failures").top_failures, Some(DEFAULT_TOP));
        assert_eq!(test_args("test A --top-failures 15").top_failures, Some(15));
        let a = test_args("test --top-failures A");
        assert_eq!(a.apps, vec![PathBuf::from("A")]);
        assert!(p("test A --top-failures 0").is_err());
    }

    #[test]
    fn top_refusals_count_is_optional() {
        assert_eq!(test_args("test A --top-refusals").top_refusals, Some(DEFAULT_TOP));
        assert_eq!(test_args("test A --top-refusals 25").top_refusals, Some(25));
        // The app after a bare --top-refusals is an app, not a count.
        let a = test_args("test --top-refusals A");
        assert_eq!(a.top_refusals, Some(DEFAULT_TOP));
        assert_eq!(a.apps, vec![PathBuf::from("A")]);
        assert!(p("test A --top-refusals 0").is_err());
    }

    #[test]
    fn test_data_takes_an_optional_backup_path_that_cannot_be_mistaken_for_an_app() {
        let a = test_args("test A --test-data");
        assert!(a.test_data && a.test_data_path.is_none());
        let a = test_args("test A --test-data x/y.bak");
        assert_eq!(a.test_data_path, Some(PathBuf::from("x/y.bak")));
        assert_eq!(a.apps, vec![PathBuf::from("A")]);
        let a = test_args("test --test-data B.Source.zip");
        assert!(
            a.test_data && a.test_data_path.is_none(),
            "an app after a bare --test-data is an app"
        );
        assert_eq!(a.apps, vec![PathBuf::from("B.Source.zip")]);
        let a = test_args("test A --test-data=some/Demo.BACPAC");
        assert_eq!(a.test_data_path, Some(PathBuf::from("some/Demo.BACPAC")));
        let a = test_args("test A --test-data --test-data-company CRONUS");
        assert_eq!(a.test_data_company.as_deref(), Some("CRONUS"));
        // Test data is on by default, so a company can be named without --test-data.
        let a = test_args("test A --test-data-company X");
        assert!(!a.no_test_data && a.test_data_company.as_deref() == Some("X"));
    }

    #[test]
    fn test_data_is_on_unless_no_test_data_turns_it_off() {
        let a = test_args("test A");
        assert!(
            !a.no_test_data && !a.test_data,
            "no flag: the default decides, not the user"
        );
        assert!(a.wants_test_data() && !a.test_data_asked_for());
        let a = test_args("test A --no-test-data");
        assert!(a.no_test_data && !a.wants_test_data());
        let a = test_args("test A --test-data");
        assert!(a.wants_test_data() && a.test_data_asked_for());
        assert!(
            p("test A --test-data --no-test-data")
                .unwrap_err()
                .contains("cannot be combined")
        );
        assert!(
            p("test A --test-data=x.bak --no-test-data")
                .unwrap_err()
                .contains("cannot be combined")
        );
        assert!(
            p("test A --no-test-data --test-data-company X")
                .unwrap_err()
                .contains("--no-test-data")
        );
    }

    #[test]
    fn bad_invocations_say_what_is_wrong() {
        assert!(p("test").unwrap_err().contains("at least one APP"));
        assert!(p("test A --nope").unwrap_err().contains("unknown option `--nope`"));
        assert!(p("test A --codeunit x").unwrap_err().contains("object id"));
        assert!(p("test A --dep").unwrap_err().contains("needs a value"));
        assert!(p("frobnicate").unwrap_err().contains("unknown command"));
        assert!(p("test A --verbose --quiet").is_err());
    }

    #[test]
    fn deferred_and_unavailable_options_are_refused_by_name() {
        assert!(
            p("test A --test-data-normalize-company")
                .unwrap_err()
                .contains("not available")
        );
        assert!(p("test A --watch").unwrap_err().contains("deferred"));
        assert!(p("run").unwrap_err().contains("not implemented yet"));
    }
}
