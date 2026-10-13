//! `al-test-cli`: run the corpus against the runtime and report the pass rate.
//!
//! Exit codes: 0 ok, 1 regression against the baseline, 2 setup error
//! (oracle and corpus disagree, unreadable input, bad arguments).

use al_test_cli::{config, corpus, drift, oracle, output, report, runner::Runner, runtime, target};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Default)]
struct Args {
    root: Option<PathBuf>,
    oracle: Option<PathBuf>,
    baseline: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    junit: Option<PathBuf>,
    update_baseline: bool,
    /// `--bc-version PREFIX`: which oracle to run (default: the newest).
    bc_version: Option<String>,
    /// `--all-versions`: run every version in `oracle.toml`, one after the other.
    all_versions: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--root" => a.root = Some(val()?),
            "--oracle" => a.oracle = Some(val()?),
            "--baseline" => a.baseline = Some(val()?),
            "--json" => a.json = Some(val()?),
            "--markdown" => a.markdown = Some(val()?),
            "--junit" => a.junit = Some(val()?),
            "--update-baseline" => a.update_baseline = true,
            "--all-versions" => a.all_versions = true,
            "--bc-version" => a.bc_version = Some(val()?.to_string_lossy().into_owned()),
            _ => return Err(format!("unknown argument `{flag}`")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    // The tree-walking VM recurses on the native stack per AL call; give it room.
    let worker = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(run)
        .expect("spawn runner thread");
    match worker.join().expect("runner thread panicked") {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn write(p: &Path, s: &str) -> Result<(), String> {
    std::fs::write(p, s).map_err(|e| format!("{}: {e}", p.display()))
}

fn run() -> Result<ExitCode, String> {
    let args = parse_args()?;
    let root = args.root.clone().unwrap_or_else(|| PathBuf::from("."));
    let oracle_path = args.oracle.clone().unwrap_or_else(|| root.join("oracle/oracle.toml"));
    let cfg = config::OracleFile::parse(&read(&oracle_path)?).map_err(|e| format!("{}: {e}", oracle_path.display()))?;
    if args.all_versions && (args.bc_version.is_some() || args.baseline.is_some()) {
        return Err("--all-versions cannot be combined with --bc-version or --baseline".into());
    }
    let versions: Vec<&config::VersionEntry> = if args.all_versions {
        let mut v: Vec<_> = cfg.versions.iter().collect();
        v.sort_by_key(|v| v.major);
        v
    } else {
        vec![cfg.select(args.bc_version.as_deref())?]
    };

    let pinned = git_head(&root.join(&cfg.corpus_repo))?;
    if pinned != cfg.corpus_commit {
        return Err(format!(
            "corpus pin {pinned} does not match the oracle's corpus_commit {}; the oracle must come from the CI run of the pinned commit",
            cfg.corpus_commit
        ));
    }

    let mut code = 0u8;
    let mut runs = Vec::new();
    for v in versions {
        let r = run_version(&args, &root, &oracle_path, &cfg, v)?;
        code = code.max(r.code);
        runs.push((v.major, r));
    }
    if runs.len() > 1 {
        print!("{}", drift_summary(&runs));
    }
    Ok(ExitCode::from(code))
}

/// What one version's run leaves for the comparison across versions.
struct VersionRun {
    code: u8,
    /// Cases real BC passed in this version's oracle.
    oracle: std::collections::BTreeSet<oracle::CaseId>,
    /// Cases the runtime passed.
    passing: std::collections::BTreeSet<oracle::CaseId>,
}

/// `pass-rate.json` becomes `pass-rate-29.json` when several versions run.
fn per_version(p: &Path, major: u16, all: bool) -> PathBuf {
    if !all {
        return p.to_path_buf();
    }
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let name = match p.extension() {
        Some(e) => format!("{stem}-{major}.{}", e.to_string_lossy()),
        None => format!("{stem}-{major}"),
    };
    p.with_file_name(name)
}

fn drift_summary(runs: &[(u16, VersionRun)]) -> String {
    let oracle_sets: Vec<_> = runs.iter().map(|(m, r)| (m.to_string(), r.oracle.clone())).collect();
    let passing_sets: Vec<_> = runs.iter().map(|(m, r)| (m.to_string(), r.passing.clone())).collect();
    let mut out = String::from("\nVersion comparison\n");
    out.push_str(&drift::table("oracle", &oracle_sets));
    out.push_str(&drift::table("runtime", &passing_sets));
    out
}

fn run_version(
    args: &Args,
    root: &Path,
    oracle_path: &Path,
    cfg: &config::OracleFile,
    version: &config::VersionEntry,
) -> Result<VersionRun, String> {
    let junit_path = oracle_path.parent().unwrap_or(Path::new(".")).join(&version.junit);
    let cases = oracle::parse_junit(&read(&junit_path)?).map_err(|e| format!("{}: {e:?}", junit_path.display()))?;
    let index = corpus::discover(&root.join(&cfg.corpus_app)).map_err(|e| format!("corpus: {e}"))?;

    // ADR-015 3.2: the corpus runs as the oracle's BC version with the corpus app's runtime.
    let leg = version.bc();
    let chosen = al_version::choose_target(
        Some(&leg),
        &target::read_app_versions(&root.join(&cfg.corpus_app))?,
        &leg,
    )
    .map_err(|e| e.to_string())?;
    println!("target: BC {}, runtime {}", chosen.target.bc, chosen.target.runtime);
    for note in &chosen.notes {
        eprintln!("note: {note}");
    }
    let symbols = al_version::corpus_symbols(chosen.target.bc.major);
    let corpus_tests = root
        .join(&cfg.corpus_app)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let app_dirs = [
        corpus_tests.join("al-language-internals-fixture"),
        root.join(&cfg.corpus_app),
    ];
    let apps = app_dirs
        .iter()
        .map(|d| runtime::load_app(d))
        .collect::<Result<Vec<_>, _>>()?;
    let manifests = app_dirs
        .iter()
        .map(|d| runtime::load_manifest(d))
        .collect::<Result<Vec<_>, _>>()?;
    let rt = runtime::CorpusRuntime::run_for(apps, &manifests, &symbols, Some(&chosen.target));
    if rt.panics > 0 {
        eprintln!(
            "warning: {} test codeunit(s) hit a runtime panic (reported as not run)",
            rt.panics
        );
    }
    let outcome = |c: &oracle::CaseId| rt.outcome(c);
    let area_of = |cu: u32| index.area_of(cu).map(str::to_string);
    let rep = report::build(&cases, &area_of, &outcome);
    if !rep.unmapped.is_empty() {
        return Err(format!(
            "{} oracle case(s) have no test codeunit in the corpus, e.g. {}",
            rep.unmapped.len(),
            rep.unmapped[0]
        ));
    }

    let all = args.all_versions;
    if let Some(p) = &args.json {
        write(
            &per_version(p, version.major, all),
            &serde_json::to_string_pretty(&rep).expect("report serializes"),
        )?;
    }
    if let Some(p) = &args.markdown {
        write(&per_version(p, version.major, all), &output::markdown(&rep))?;
    }
    if let Some(p) = &args.junit {
        write(&per_version(p, version.major, all), &output::junit(&cases, &outcome))?;
    }
    let oracle_ok: std::collections::BTreeSet<_> =
        cases.iter().filter(|c| !c.bc_failed).map(|c| c.id.clone()).collect();
    let passing: std::collections::BTreeSet<_> = oracle_ok
        .iter()
        .filter(|c| outcome(c) == report::Outcome::Pass)
        .cloned()
        .collect();
    println!(
        "BC {}: {}/{} pass ({:.1}%), {} fail, {} not run",
        version.bc_version,
        rep.overall.pass,
        rep.overall.total,
        rep.overall.rate() * 100.0,
        rep.overall.fail,
        rep.overall.not_run
    );

    let baseline_path = args.baseline.clone().unwrap_or_else(|| root.join(&version.baseline));
    let baseline =
        report::parse_baseline(&read(&baseline_path)?).map_err(|e| format!("{}: {e}", baseline_path.display()))?;
    let regressed = report::regressions(&baseline, &outcome);
    if !regressed.is_empty() {
        eprintln!(
            "{} regression(s): cases in {} no longer pass:",
            regressed.len(),
            baseline_path.display()
        );
        for c in &regressed {
            eprintln!("  {} {}", c.codeunit, c.name);
        }
        return Ok(VersionRun {
            code: 1,
            oracle: oracle_ok,
            passing,
        });
    }
    if args.update_baseline {
        write(&baseline_path, &report::format_baseline(&passing))?;
    }
    Ok(VersionRun {
        code: 0,
        oracle: oracle_ok,
        passing,
    })
}

fn git_head(repo: &Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} is not a git checkout; run `git submodule update --init`",
            repo.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
