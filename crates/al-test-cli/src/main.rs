//! `al-test-cli`: run the corpus against the runtime and report the pass rate.
//!
//! Exit codes: 0 ok, 1 regression against the baseline, 2 setup error
//! (oracle and corpus disagree, unreadable input, bad arguments).

use al_host::dotnet::DotNetHost;
use al_test_cli::dotnet::{SidecarConfig, SidecarHost};
use al_test_cli::{corpus, oracle, output, report, runner::Runner, runtime};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

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
    /// `--dotnet-sidecar PATH`: the built sidecar (default: next to this binary, else built once
    /// into a cache from `tools/dotnet-sidecar`).
    dotnet_sidecar: Option<PathBuf>,
    /// `--dotnet-assembly-dir DIR` (repeatable): where community assemblies are searched.
    dotnet_assembly_dirs: Vec<PathBuf>,
    /// `--dotnet-allow-roll-forward`: run the sidecar on a newer .NET major than 8, with a warning.
    dotnet_allow_roll_forward: bool,
    /// `--dotnet-timeout-secs N`: the deadline of one `DotNet` call (default 60).
    dotnet_timeout_secs: Option<u64>,
    /// `--require-dotnet`: a machine without a usable .NET is an error, not a skip (CI).
    require_dotnet: bool,
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
            "--dotnet-sidecar" => a.dotnet_sidecar = Some(val()?),
            "--dotnet-assembly-dir" => a.dotnet_assembly_dirs.push(val()?),
            "--dotnet-allow-roll-forward" => a.dotnet_allow_roll_forward = true,
            "--require-dotnet" => a.require_dotnet = true,
            "--dotnet-timeout-secs" => {
                let v = val()?;
                a.dotnet_timeout_secs = Some(
                    v.to_string_lossy()
                        .parse()
                        .map_err(|_| format!("{flag} needs a number of seconds"))?,
                );
            }
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

    // ADR-007: BC 27.5 defines BC27PLUS.
    let symbols = ["BC27PLUS".to_string()].into();
    let corpus_tests = root
        .join(&cfg.corpus_app)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let apps = vec![
        runtime::load_app(&corpus_tests.join("al-language-internals-fixture"))?,
        runtime::load_app(&root.join(&cfg.corpus_app))?,
    ];
    // ADR-014: the sidecar starts lazily, at the first `DotNet` call that reaches it.
    let host = Arc::new(SidecarHost::new(SidecarConfig {
        sidecar: args.dotnet_sidecar.clone(),
        source_root: Some(root.clone()),
        assembly_dirs: args.dotnet_assembly_dirs.clone(),
        allow_roll_forward: args.dotnet_allow_roll_forward,
        timeout: Duration::from_secs(args.dotnet_timeout_secs.unwrap_or(60)),
        ..Default::default()
    }));
    if args.require_dotnet {
        if let Err(why) = host.availability() {
            return Err(format!("--require-dotnet: DotNet {why}"));
        }
    }
    let dotnet = al_vm::dotnet::DotNetConfig {
        host,
        ..Default::default()
    };
    let rt = runtime::CorpusRuntime::run_with(apps, &symbols, dotnet);
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
    let ratchet = report::ratchet(&baseline, &outcome);
    if !ratchet.skipped.is_empty() {
        // ADR-014 section 11: not a regression, but never silent.
        eprintln!(
            "{} baseline case(s) skipped, .NET is unavailable on this machine (CI uses --require-dotnet):",
            ratchet.skipped.len()
        );
        for (c, why) in &ratchet.skipped {
            eprintln!("  {} {}: {why}", c.codeunit, c.name);
        }
    }
    match report::decide(&ratchet, args.update_baseline, args.require_dotnet) {
        report::Decision::Ok => {}
        report::Decision::Setup(why) => return Err(why),
        report::Decision::Regression => {
            eprintln!(
                "{} regression(s): cases in {} no longer pass:",
                ratchet.regressions.len(),
                baseline_path.display()
            );
            for c in &ratchet.regressions {
                eprintln!("  {} {}", c.codeunit, c.name);
            }
            return Ok(ExitCode::from(1));
        }
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
