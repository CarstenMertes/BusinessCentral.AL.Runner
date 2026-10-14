//! The command line of `alrunner` (ADR-016 section 2): parsing and the help text.

use std::path::PathBuf;

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Test(Box<TestArgs>),
}

/// `alrunner test`.
#[derive(Debug, Default, PartialEq)]
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
    /// `--codeunit ID`: select a test codeunit by object id.
    pub codeunits: Vec<u32>,
    /// `--filter TEXT`: select the tests whose name contains TEXT (any case).
    pub filters: Vec<String>,
    pub junit: Option<PathBuf>,
    /// `--top-refusals [N]`: how many refusal reasons to list (default 10).
    pub top_refusals: Option<usize>,
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
    pub require_dotnet: bool,
    pub files_dir: Option<PathBuf>,
}

pub const DEFAULT_TOP: usize = 10;

pub const HELP: &str = "\
alrunner: run AL tests without a service tier (AL Runner v3, preview)

USAGE
    alrunner test [OPTIONS] APP...
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
    --no-download        Never fetch Microsoft's apps from the artifact feed.
    --cache-dir DIR      Where downloaded artifacts are kept (else $ALRUNNER_CACHE).

SELECTION (a test is selected when any option matches it)
    --codeunit ID        A test codeunit by object id. Repeatable.
    --filter TEXT        Tests whose name contains TEXT, any case. Repeatable.
    A selected test's codeunit runs whole (tests of a codeunit share state); only the selected
    tests are reported. Only the selected codeunits are compiled.

OUTPUT
    Default: header, one line per failing test, the top refusal reasons, a summary.
    --verbose            One line per test (PASS, FAIL, NOT RUN with its reason).
    --quiet              Summary only.
    --top-refusals [N]   List the N most common reasons tests were not run (default 10).
    --refusal-detail TEXT  List the not-run reasons that contain TEXT (site and call arguments
                         folded, names kept), most common first (default 30 lines).
    --junit FILE         Write JUnit XML. A test that did not run is <skipped> with its reason.
    --timings            Compile phases and the slowest test codeunits.

EXIT CODES
    0  every selected test passed
    1  a test failed
    2  bad invocation or the run could not start
    6  the selection matched no test
    7  some tests were not run (a refusal); --allow-not-run makes this 0
    Order when several apply: 2, 6, 1, 7.

TEST DATA
    There is no test data yet (ADR-020 is Proposed). Microsoft's tests expect a CRONUS company,
    so many of them fail on missing setup data. Trust the NOT RUN counts and reasons of such a
    run; do not trust its failures.

OTHER
    --files-dir DIR      Let the File data type read and write real files under DIR.
    --dotnet-sidecar PATH, --dotnet-assembly-dir DIR, --dotnet-allow-roll-forward,
    --dotnet-timeout-secs N, --require-dotnet     The optional .NET helper (ADR-014).

ENVIRONMENT
    AL_CHECK_THREADS, AL_LOWER_THREADS, AL_TEST_THREADS  Worker threads of the check, the
    lowering and the test codeunits (default: one per core).
";

fn number(flag: &str, v: &str) -> Result<usize, String> {
    v.parse().map_err(|_| format!("{flag} needs a number, got `{v}`"))
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
        "run" | "serve" | "debug" | "deps" | "package" | "helper" | "doctor" | "migrate" | "guide" => {
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
            "--cache-dir" => a.cache_dir = Some(text()?.into()),
            "--codeunit" => {
                let v = text()?;
                a.codeunits
                    .push(v.parse().map_err(|_| format!("{flag} needs an object id, got `{v}`"))?);
            }
            "--filter" | "--test" => a.filters.push(text()?),
            "--refusal-detail" => a.refusal_detail = Some(text()?),
            "--junit" => a.junit = Some(text()?.into()),
            "--allow-not-run" => a.allow_not_run = true,
            "--verbose" | "-v" => a.verbose = true,
            "--quiet" | "-q" => a.quiet = true,
            "--timings" => a.timings = true,
            "--files-dir" => a.files_dir = Some(text()?.into()),
            "--dotnet-sidecar" => a.dotnet_sidecar = Some(text()?.into()),
            "--dotnet-assembly-dir" => a.dotnet_assembly_dirs.push(text()?.into()),
            "--dotnet-allow-roll-forward" => a.dotnet_allow_roll_forward = true,
            "--require-dotnet" => a.require_dotnet = true,
            "--dotnet-timeout-secs" => {
                let v = text()?;
                a.dotnet_timeout_secs = Some(number(&flag, &v)? as u64);
            }
            "--test-data" | "--test-data-company" | "--test-data-normalize-company" => {
                return Err(format!(
                    "{flag} is not available yet: test data from a BC backup needs ADR-020 (Proposed). Until then a run has no company data"
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
    fn refusal_detail_takes_a_text() {
        assert_eq!(
            test_args("test A --refusal-detail is_not_lowered").refusal_detail.as_deref(),
            Some("is_not_lowered")
        );
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
        assert!(p("test A --test-data").unwrap_err().contains("ADR-020"));
        assert!(p("test A --watch").unwrap_err().contains("deferred"));
        assert!(p("serve").unwrap_err().contains("not implemented yet"));
    }
}
