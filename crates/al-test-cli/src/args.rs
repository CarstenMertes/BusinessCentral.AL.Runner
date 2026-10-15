//! Command-line arguments of `al-test-cli`: parsing and the `--help` text.

use crate::select::Selection;
use std::path::PathBuf;

#[derive(Default)]
pub struct Args {
    pub root: Option<PathBuf>,
    pub oracle: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    /// `--suite-baseline NAME=FILE` (repeatable): the ratchet file of an extra test app. With
    /// `--baseline` the extra apps are ratcheted only when named here.
    pub suite_baselines: Vec<(String, PathBuf)>,
    pub json: Option<PathBuf>,
    pub markdown: Option<PathBuf>,
    pub junit: Option<PathBuf>,
    pub update_baseline: bool,
    /// `--no-ratchet`: report only; do not compare with the baseline (for running the corpus
    /// without the dependency apps its baseline needs).
    pub no_ratchet: bool,
    /// Extra directories of `.app` packages, searched after the project's `.alpackages`.
    pub deps: Vec<PathBuf>,
    /// Run without dependency apps (the corpus alone).
    pub no_deps: bool,
    /// `--bc-version PREFIX`: which oracle to run (default: the newest).
    pub bc_version: Option<String>,
    /// `--all-versions`: run every version in `oracle.toml`, one after the other.
    pub all_versions: bool,
    /// `--dotnet-sidecar PATH`: the built sidecar (default: next to this binary, else built once
    /// into a cache from `tools/dotnet-sidecar`).
    pub dotnet_sidecar: Option<PathBuf>,
    /// `--dotnet-assembly-dir DIR` (repeatable): where community assemblies are searched.
    pub dotnet_assembly_dirs: Vec<PathBuf>,
    /// `--dotnet-allow-roll-forward`: run the sidecar on a newer .NET major than 8, with a warning.
    pub dotnet_allow_roll_forward: bool,
    /// `--dotnet-timeout-secs N`: the deadline of one `DotNet` call (default 60).
    pub dotnet_timeout_secs: Option<u64>,
    /// `--require-dotnet`: a machine without a usable .NET is an error, not a skip (CI).
    pub require_dotnet: bool,
    /// `--files-dir DIR`: the `File` data type reads and writes real files under DIR. Without it
    /// files live in memory and are gone when the test ends.
    pub files_dir: Option<PathBuf>,
    /// `--environment saas|onprem`: what Environment Information answers (default saas, as probed).
    pub environment: Option<al_host::Environment>,
    /// `--no-download`: never fetch a dependency app from Microsoft's artifact feed (ADR-013).
    pub no_download: bool,
    /// `--cache-dir DIR`: where downloaded artifacts are kept (else `ALRUNNER_CACHE`, else
    /// `~/.cache/al-runtime`).
    pub cache_dir: Option<PathBuf>,
    /// `--fetch-deps`: resolve and download the dependency apps, then stop (warms the cache).
    pub fetch_deps: bool,
    /// `--compile-cache`: use the compile cache of the dependency apps (ADR-022). The corpus
    /// ratchet runs without it: it is recorded under the demand-driven compile.
    pub compile_cache: bool,
    /// `--require-compile-cache`: a run without a usable cache entry is an error (CI).
    pub require_compile_cache: bool,
    /// `--build-compile-cache`: build the entry (or check the one there), then stop.
    pub build_compile_cache: bool,
    /// `--print-builds`: print the newest published build for each version and stop. CI keys its
    /// artifact cache by this.
    pub print_builds: bool,
    /// `--project DIR` (with `--fetch-deps`): any AL project, not the corpus. The BC build comes
    /// from `--bc-version`, else from the app.json floor (ADR-013 Layer A, ADR-015 3.2).
    pub project: Option<PathBuf>,
    /// `--codeunit`, `--test`, `--area`, `--file`: which cases to compile, run and report.
    pub select: Selection,
    /// `--must-not-compile [DIR]`: check the must-not-compile corpus (`mnc.rs`) and stop. DIR is the
    /// folder of the cases or the one that holds `cases/`; without it `corpus/tests/must-not-compile`.
    pub must_not_compile: Option<Option<PathBuf>>,
    /// `--explain TEST`: run one test and say why it ended as it did.
    pub explain: Option<String>,
    /// `--top-refusals [N]`: rank the refusal reasons; N entries (default 10).
    pub top_refusals: Option<usize>,
    /// `--top-refusals-area AREA`: the ranking counts only that area (and selects it).
    pub top_refusals_area: Option<String>,
    /// `--baseline-diff`: list what newly passes, what fails and what left the baseline.
    pub baseline_diff: bool,
    /// `--timings`: print how long loading, compiling and running took, and the slowest test codeunits.
    pub timings: bool,
    /// `--timings-table FILE` (repeatable): print the timing table of these `--json` reports, then
    /// stop. CI uses it for the runs whose reports are not part of the summary.
    pub timings_tables: Vec<PathBuf>,
}

/// What the command line asks for.
pub enum Parsed {
    Help,
    Run(Box<Args>),
}

/// The default length of `--top-refusals`.
pub const DEFAULT_TOP: usize = 10;

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut a = Args::default();
    let mut it = args.into_iter().peekable();
    while let Some(flag) = it.next() {
        if matches!(flag.as_str(), "--help" | "-h") {
            return Ok(Parsed::Help);
        }
        if flag == "--must-not-compile" {
            // The folder is optional.
            let dir = match it.peek() {
                Some(v) if !v.starts_with("--") => it.next().map(PathBuf::from),
                _ => None,
            };
            a.must_not_compile = Some(dir);
            continue;
        }
        if flag == "--top-refusals" {
            // The count is optional: `--top-refusals` or `--top-refusals 5`.
            let n = match it.peek() {
                Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                    let n = it
                        .next()
                        .unwrap_or_default()
                        .parse()
                        .map_err(|_| "--top-refusals: number too large")?;
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
        let mut text = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--root" => a.root = Some(text()?.into()),
            "--oracle" => a.oracle = Some(text()?.into()),
            "--baseline" => a.baseline = Some(text()?.into()),
            "--suite-baseline" => {
                let v = text()?;
                let (name, file) = v
                    .split_once('=')
                    .ok_or_else(|| format!("{flag} needs NAME=FILE, got `{v}`"))?;
                a.suite_baselines.push((name.to_string(), PathBuf::from(file)));
            }
            "--json" => a.json = Some(text()?.into()),
            "--markdown" => a.markdown = Some(text()?.into()),
            "--junit" => a.junit = Some(text()?.into()),
            "--update-baseline" => a.update_baseline = true,
            "--no-ratchet" => a.no_ratchet = true,
            "--deps" => a.deps.push(text()?.into()),
            "--no-deps" => a.no_deps = true,
            "--all-versions" => a.all_versions = true,
            "--bc-version" => a.bc_version = Some(text()?),
            "--dotnet-sidecar" => a.dotnet_sidecar = Some(text()?.into()),
            "--dotnet-assembly-dir" => a.dotnet_assembly_dirs.push(text()?.into()),
            "--dotnet-allow-roll-forward" => a.dotnet_allow_roll_forward = true,
            "--require-dotnet" => a.require_dotnet = true,
            "--files-dir" => a.files_dir = Some(text()?.into()),
            "--environment" => a.environment = Some(text()?.parse().map_err(|e| format!("--environment: {e}"))?),
            "--no-download" => a.no_download = true,
            "--cache-dir" => a.cache_dir = Some(text()?.into()),
            "--fetch-deps" => a.fetch_deps = true,
            "--compile-cache" => a.compile_cache = true,
            "--require-compile-cache" => {
                a.compile_cache = true;
                a.require_compile_cache = true;
            }
            "--build-compile-cache" => {
                a.compile_cache = true;
                a.build_compile_cache = true;
            }
            "--print-builds" => a.print_builds = true,
            "--project" => a.project = Some(text()?.into()),
            "--dotnet-timeout-secs" => {
                a.dotnet_timeout_secs = Some(
                    text()?
                        .parse()
                        .map_err(|_| format!("{flag} needs a number of seconds"))?,
                );
            }
            "--codeunit" => {
                let v = text()?;
                a.select
                    .codeunits
                    .push(v.parse().map_err(|_| format!("{flag} needs an object id, got `{v}`"))?);
            }
            "--test" => a.select.tests.push(text()?),
            "--area" => a.select.areas.push(text()?),
            "--file" => a.select.files.push(text()?),
            "--explain" => a.explain = Some(text()?),
            "--top-refusals-area" => {
                let area = text()?;
                // The ranking is of one area, so only that area is compiled and run.
                a.select.areas.push(area.clone());
                a.top_refusals_area = Some(area);
                a.top_refusals.get_or_insert(DEFAULT_TOP);
            }
            "--baseline-diff" => a.baseline_diff = true,
            "--timings" => a.timings = true,
            "--timings-table" => a.timings_tables.push(text()?.into()),
            _ => return Err(format!("unknown argument `{flag}` (see --help)")),
        }
    }
    if a.update_baseline && a.select.is_active() {
        return Err(format!(
            "--update-baseline cannot be combined with a selection ({}): the baseline would lose every case outside it",
            a.select.describe()
        ));
    }
    if a.update_baseline && a.explain.is_some() {
        return Err("--update-baseline cannot be combined with --explain".into());
    }
    Ok(Parsed::Run(Box::new(a)))
}

pub const HELP: &str = "\
al-test-cli: run the corpus against the runtime and report the pass rate.

USAGE
  al-test-cli [--root DIR] [FLAGS]

EXIT CODES
  0 ok    1 a baseline case no longer passes    2 setup error (bad arguments, oracle and corpus disagree)

INPUT
  --root DIR                  the repository root (default: .)
  --oracle FILE               the oracle table (default: ROOT/oracle/oracle.toml)
  --bc-version PREFIX         which BC version's oracle to run, e.g. 28 or 28.5 (default: the newest)
  --all-versions              run every version in the oracle table, then compare them
  --deps DIR                  extra directory of .app packages for the dependencies (repeatable)
  --no-deps                   run the corpus alone, without Microsoft's apps
  --no-download               never fetch a dependency app from the artifact feed
  --environment ENV           the server Environment Information describes: saas (default, as probed) or
                              onprem (IsSaaS and IsSandbox No until a test library changes them;
                              an unprobed assumption Microsoft's suites need)
  --files-dir DIR             let the File data type read and write real files under DIR (default: in memory)
  --cache-dir DIR             where downloaded artifacts are kept (default: ~/.cache/al-runtime)

SELECTION (each flag repeats; the selection is their union; without one, everything runs)
  --codeunit ID               the test codeunit with this object id
  --test NAME                 tests whose name is NAME or contains it (any case)
  --area FOLDER               the test codeunits in this corpus folder, e.g. record
  --file SUFFIX               the test codeunits in a file whose path ends with SUFFIX
  A selection compiles only the selected test codeunits and the code they reach, runs those
  codeunits whole (tests of a codeunit share state), and reports and ratchets only the selected
  cases. --update-baseline refuses to run with a selection.

DIAGNOSE
  --must-not-compile [DIR]    check the corpus of AL files the real compiler rejects (DIR, else
                              corpus/tests/must-not-compile): the same ALnnnn codes, per BC major
                              (--bc-version, default 29); exit 1 on a mismatch, then stop
  --explain TEST              run one test; print its outcome, where it was refused or failed
                              (routine, file, line), and for a failure the error, code and stack
  --top-refusals [N]          rank the refusal reasons of the not-run cases (default 10), numbers
                              and quoted names folded together, with an example test each
  --top-refusals-area FOLDER  the same for one corpus folder; also selects that folder
  --baseline-diff             per version: cases that newly pass, cases that fail and are not in
                              the baseline, and baseline cases that no longer pass; listed by area
  --timings                   print how long loading, compiling and running took, and the slowest
                              test codeunits (the --json and --markdown reports always carry the
                              times)
  --timings-table FILE        print the timing table of the --json reports named (repeatable), then stop

BASELINE AND REPORTS
  --baseline FILE             ratchet against this file instead of metrics/<major>/passing.txt
  --suite-baseline NAME=FILE  the ratchet file of an extra test app (repeatable)
  --update-baseline           rewrite the baseline files from this run (refused on any regression)
  --no-ratchet                report only; do not compare with the baseline
  --json FILE                 write the pass-rate report as JSON
  --markdown FILE             write the pass-rate report as Markdown
  --junit FILE                write per-test outcomes and refusal reasons as JUnit XML

DOTNET (ADR-014)
  --dotnet-sidecar PATH       the built sidecar (default: next to this binary)
  --dotnet-assembly-dir DIR   where community assemblies are searched (repeatable)
  --dotnet-allow-roll-forward run the sidecar on a newer .NET major than 8, with a warning
  --dotnet-timeout-secs N     the deadline of one DotNet call (default 60)
  --require-dotnet            a machine without .NET is an error, not a skip (CI)

DEPENDENCY TOOLING
  --compile-cache             use the compile cache of the dependency apps (ADR-022), in
                              <cache dir>/compile. The first run builds the entry. Outcomes equal the
                              uncached run's, except the tests in tools/demand-eager-known-differences.txt
  --require-compile-cache     with --compile-cache: a miss is an error, not a build (CI)
  --build-compile-cache       build the entry for each selected version (or read and check the one
                              there), print how long it took, then stop
  --fetch-deps                resolve and download the dependency apps, then stop
  --project DIR               with --fetch-deps: any AL project, not the corpus
  --print-builds              print the newest published build of each version, then stop

  -h, --help                  this text
";

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Box<Args>, String> {
        match parse(args.iter().map(|s| s.to_string()))? {
            Parsed::Run(a) => Ok(a),
            Parsed::Help => Err("help".into()),
        }
    }

    #[test]
    fn selection_flags_repeat() {
        let a = run(&[
            "--codeunit",
            "5",
            "--codeunit",
            "6",
            "--test",
            "x",
            "--area",
            "a",
            "--file",
            "f",
        ])
        .unwrap();
        assert_eq!(a.select.codeunits, [5, 6]);
        assert_eq!(a.select.tests, ["x"]);
        assert!(a.select.is_active());
    }

    #[test]
    fn top_refusals_takes_an_optional_count() {
        assert_eq!(run(&["--top-refusals"]).unwrap().top_refusals, Some(DEFAULT_TOP));
        assert_eq!(run(&["--top-refusals", "3"]).unwrap().top_refusals, Some(3));
        let a = run(&["--top-refusals", "--no-deps"]).unwrap();
        assert!(a.no_deps && a.top_refusals == Some(DEFAULT_TOP));
        assert!(run(&["--top-refusals", "0"]).is_err());
    }

    #[test]
    fn a_top_refusals_area_selects_the_area() {
        let a = run(&["--top-refusals-area", "record"]).unwrap();
        assert_eq!(a.select.areas, ["record"]);
        assert_eq!(a.top_refusals, Some(DEFAULT_TOP));
    }

    #[test]
    fn update_baseline_refuses_a_selection() {
        let e = run(&["--update-baseline", "--area", "json"]).err().unwrap();
        assert!(e.contains("--update-baseline") && e.contains("--area json"), "{e}");
    }

    #[test]
    fn every_flag_the_parser_knows_is_in_the_help_text() {
        let src = include_str!("args.rs");
        for line in src.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("\"--").filter(|_| t.contains("\" =>")) {
                let flag = rest.split('"').next().unwrap_or_default();
                assert!(HELP.contains(&format!("--{flag}")), "--{flag} is not in HELP");
            }
        }
    }
}
