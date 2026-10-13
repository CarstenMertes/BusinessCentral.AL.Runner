//! `al-test-cli`: run the corpus against the runtime and report the pass rate.
//!
//! Exit codes: 0 ok, 1 regression against the baseline, 2 setup error
//! (oracle and corpus disagree, unreadable input, bad arguments).

use al_test_cli::{corpus, oracle, output, report, runner::Runner, runtime};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(serde::Deserialize)]
struct OracleConfig {
    bc_version: String,
    corpus_commit: String,
    junit: String,
    corpus_repo: String,
    corpus_app: String,
}

#[derive(Default)]
struct Args {
    root: Option<PathBuf>,
    oracle: Option<PathBuf>,
    baseline: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    junit: Option<PathBuf>,
    update_baseline: bool,
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
    let cfg: OracleConfig =
        toml::from_str(&read(&oracle_path)?).map_err(|e| format!("{}: {e}", oracle_path.display()))?;

    let pinned = git_head(&root.join(&cfg.corpus_repo))?;
    if pinned != cfg.corpus_commit {
        return Err(format!(
            "corpus pin {pinned} does not match the oracle's corpus_commit {}; the oracle must come from the CI run of the pinned commit",
            cfg.corpus_commit
        ));
    }

    let junit_path = oracle_path.parent().unwrap_or(Path::new(".")).join(&cfg.junit);
    let cases = oracle::parse_junit(&read(&junit_path)?).map_err(|e| format!("{}: {e:?}", junit_path.display()))?;
    let index = corpus::discover(&root.join(&cfg.corpus_app)).map_err(|e| format!("corpus: {e}"))?;

    // ADR-015: the symbols the corpus CI defines for BC 27.
    let symbols = al_version::corpus_symbols(27);
    let corpus_tests = root
        .join(&cfg.corpus_app)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let apps = vec![
        runtime::load_app(&corpus_tests.join("al-language-internals-fixture"))?,
        runtime::load_app(&root.join(&cfg.corpus_app))?,
    ];
    let rt = runtime::CorpusRuntime::run(apps, &symbols);
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

    if let Some(p) = &args.json {
        write(p, &serde_json::to_string_pretty(&rep).expect("report serializes"))?;
    }
    if let Some(p) = &args.markdown {
        write(p, &output::markdown(&rep))?;
    }
    if let Some(p) = &args.junit {
        write(p, &output::junit(&cases, &outcome))?;
    }
    println!(
        "BC {}: {}/{} pass ({:.1}%), {} fail, {} not run",
        cfg.bc_version,
        rep.overall.pass,
        rep.overall.total,
        rep.overall.rate() * 100.0,
        rep.overall.fail,
        rep.overall.not_run
    );

    let baseline_path = args
        .baseline
        .clone()
        .unwrap_or_else(|| root.join("metrics/passing.txt"));
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
        return Ok(ExitCode::from(1));
    }
    if args.update_baseline {
        let passing = cases
            .iter()
            .filter(|c| !c.bc_failed && outcome(&c.id) == report::Outcome::Pass)
            .map(|c| c.id.clone())
            .collect();
        write(&baseline_path, &report::format_baseline(&passing))?;
    }
    Ok(ExitCode::SUCCESS)
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
