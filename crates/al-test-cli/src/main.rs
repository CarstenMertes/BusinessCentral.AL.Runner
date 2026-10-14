//! `al-test-cli`: run the corpus against the runtime and report the pass rate.
//!
//! Exit codes: 0 ok, 1 regression against the baseline, 2 setup error
//! (oracle and corpus disagree, unreadable input, bad arguments).

use al_host::dotnet::DotNetHost;
use al_test_cli::dotnet::{SidecarConfig, SidecarHost};
use al_test_cli::{config, corpus, deps, drift, oracle, output, pin, report, runner::Runner, runtime, target};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Default)]
struct Args {
    root: Option<PathBuf>,
    oracle: Option<PathBuf>,
    baseline: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    junit: Option<PathBuf>,
    update_baseline: bool,
    /// `--no-ratchet`: report only; do not compare with the baseline (for running the corpus
    /// without the dependency apps its baseline needs).
    no_ratchet: bool,
    /// Extra directories of `.app` packages, searched after the project's `.alpackages`.
    deps: Vec<PathBuf>,
    /// Run without dependency apps (the corpus alone).
    no_deps: bool,
    /// `--bc-version PREFIX`: which oracle to run (default: the newest).
    bc_version: Option<String>,
    /// `--all-versions`: run every version in `oracle.toml`, one after the other.
    all_versions: bool,
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
    /// `--no-download`: never fetch a dependency app from Microsoft's artifact feed (ADR-013).
    no_download: bool,
    /// `--cache-dir DIR`: where downloaded artifacts are kept (else `ALRUNNER_CACHE`, else
    /// `~/.cache/al-runtime`).
    cache_dir: Option<PathBuf>,
    /// `--fetch-deps`: resolve and download the dependency apps, then stop (warms the cache).
    fetch_deps: bool,
    /// `--print-builds`: print the newest published build for each version and stop. CI keys its
    /// artifact cache by this.
    print_builds: bool,
    /// `--project DIR` (with `--fetch-deps`): any AL project, not the corpus. The BC build comes
    /// from `--bc-version`, else from the app.json floor (ADR-013 Layer A, ADR-015 3.2).
    project: Option<PathBuf>,
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
            "--no-ratchet" => a.no_ratchet = true,
            "--deps" => a.deps.push(val()?),
            "--no-deps" => a.no_deps = true,
            "--all-versions" => a.all_versions = true,
            "--bc-version" => a.bc_version = Some(val()?.to_string_lossy().into_owned()),
            "--dotnet-sidecar" => a.dotnet_sidecar = Some(val()?),
            "--dotnet-assembly-dir" => a.dotnet_assembly_dirs.push(val()?),
            "--dotnet-allow-roll-forward" => a.dotnet_allow_roll_forward = true,
            "--require-dotnet" => a.require_dotnet = true,
            "--no-download" => a.no_download = true,
            "--cache-dir" => a.cache_dir = Some(val()?),
            "--fetch-deps" => a.fetch_deps = true,
            "--print-builds" => a.print_builds = true,
            "--project" => a.project = Some(val()?),
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
    if let Some(project) = &args.project {
        return fetch_project(&args, &root, project);
    }
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

    if args.print_builds {
        return print_builds(&versions);
    }
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
    if runs.len() > 1 && !args.fetch_deps {
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
    // A case one oracle lacks (older corpus commit, ADR-007) cannot show a runtime difference.
    let passing_sets = drift::restrict(&passing_sets, &drift::common(&oracle_sets));
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
    // ADR-007: the oracle may be recorded for an older corpus commit when the pin only added files.
    pin::check(
        &root.join(&cfg.corpus_repo),
        version.oracle_commit(&cfg.corpus_commit),
        &cfg.corpus_commit,
    )
    .map_err(|e| format!("BC {}: {e}", version.bc_version))?;
    let junit_path = oracle_path.parent().unwrap_or(Path::new(".")).join(&version.junit);
    let cases = oracle::parse_junit(&read(&junit_path)?).map_err(|e| format!("{}: {e:?}", junit_path.display()))?;
    let index = corpus::discover(&root.join(&cfg.corpus_app)).map_err(|e| format!("corpus: {e}"))?;

    // ADR-015 3.2: the corpus runs as the oracle's BC version with the corpus app's runtime.
    let leg = version.bc();
    let app_versions = target::read_app_versions(&root.join(&cfg.corpus_app))?;
    let mut chosen = al_version::choose_target(Some(&leg), &app_versions, &leg).map_err(|e| e.to_string())?;
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
    let fixture_dir = corpus_tests.join("al-language-internals-fixture");
    let app_dirs = [fixture_dir.clone(), root.join(&cfg.corpus_app)];
    let mut apps = app_dirs
        .iter()
        .map(|d| runtime::load_app(d))
        .collect::<Result<Vec<_>, _>>()?;
    let mut manifests = app_dirs
        .iter()
        .map(|d| runtime::load_manifest(d))
        .collect::<Result<Vec<_>, _>>()?;
    let mut kinds = vec![al_sema::AppKind::Workspace; apps.len()];
    let mut dependency_summary = Vec::new();
    let mut bc_build = None;
    if !args.no_deps {
        // Dependencies go after the corpus apps, so the corpus objects keep their positions.
        // ADR-013 Layer A: the oracle leg is the explicit `--bc-version`, so the Microsoft build
        // is the newest cached (else newest published) build of that minor.
        let provision = deps::Provision {
            cache_dir: args.cache_dir.clone(),
            download: !args.no_download,
            query: target::build_query(Some(&leg.to_string()), &chosen, &app_versions),
        };
        let deps = deps::load_with(
            &root.join(&cfg.corpus_app),
            &[fixture_dir],
            &args.deps,
            Some(&provision),
        )?;
        for line in &deps.log {
            eprintln!("{line}");
        }
        match &deps.build {
            Some(u) => {
                println!("bc build: {} ({})", u.build, origin_word(u.origin));
                chosen.target.bc.build = Some(u.build.to_string());
                bc_build = Some(u.build.to_string());
            }
            None => println!("bc build: none needed (every dependency came from .alpackages or --deps)"),
        }
        kinds.extend(deps.apps.iter().map(|_| al_sema::AppKind::Dependency));
        manifests.extend(deps.manifests);
        dependency_summary = deps.summary;
        apps.extend(deps.apps);
    }
    if args.fetch_deps {
        println!("dependencies ready for BC {}", chosen.target.bc);
        return Ok(VersionRun {
            code: 0,
            oracle: Default::default(),
            passing: Default::default(),
        });
    }
    // ADR-014: the sidecar starts lazily, at the first `DotNet` call that reaches it.
    let host = Arc::new(SidecarHost::new(SidecarConfig {
        sidecar: args.dotnet_sidecar.clone(),
        source_root: Some(root.to_path_buf()),
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
    let rt = runtime::CorpusRuntime::run_for_apps(apps, kinds, &manifests, &symbols, Some(&chosen.target), dotnet);
    if rt.panics > 0 {
        eprintln!(
            "warning: {} test codeunit(s) hit a runtime panic (reported as not run)",
            rt.panics
        );
    }
    let outcome = |c: &oracle::CaseId| rt.outcome(c);
    let area_of = |cu: u32| index.area_of(cu).map(str::to_string);
    let mut rep = report::build(&cases, &area_of, &outcome);
    rep.dependencies = dependency_summary;
    rep.bc_build = bc_build.clone();
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
        write(
            &per_version(p, version.major, all),
            &output::junit(&cases, &outcome, bc_build.as_deref()),
        )?;
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

    if args.no_ratchet {
        if args.update_baseline {
            return Err("--no-ratchet cannot be combined with --update-baseline".into());
        }
        return Ok(VersionRun {
            code: 0,
            oracle: oracle_ok,
            passing,
        });
    }
    let baseline_path = args.baseline.clone().unwrap_or_else(|| root.join(&version.baseline));
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
            return Ok(VersionRun {
                code: 1,
                oracle: oracle_ok,
                passing,
            });
        }
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

fn origin_word(o: al_deps::build::Origin) -> &'static str {
    match o {
        al_deps::build::Origin::Cache => "from the artifact cache",
        al_deps::build::Origin::Published => "newest published, downloaded",
    }
}

/// `--print-builds`: one line per version, `<major> <newest published build>`, from the index
/// alone (no cache, no artifact). CI keys its artifact cache by these lines.
fn print_builds(versions: &[&config::VersionEntry]) -> Result<ExitCode, String> {
    let cdn = al_deps::artifact::HttpCdn::from_env();
    for v in versions {
        let q = al_deps::build::BuildQuery::new(Some(&v.bc().to_string()), v.major, None);
        let sel = al_deps::artifact::newest_published(&cdn, &q)?;
        println!("{} {}", v.major, sel.build);
    }
    Ok(ExitCode::SUCCESS)
}

/// `--project DIR --fetch-deps`: the dependency flow for any AL project. The target comes from
/// `al_version::choose_target` (ADR-015 3.2): `--bc-version`, else the floor in the project's
/// app.json, else the newest version the oracle verifies. Layer A then picks the Microsoft build
/// for that target and the missing apps are downloaded into the cache.
fn fetch_project(args: &Args, root: &Path, project: &Path) -> Result<ExitCode, String> {
    if !args.fetch_deps {
        return Err(
            "--project is only built for --fetch-deps so far: running a project's tests is not implemented".into(),
        );
    }
    let app_versions = target::read_app_versions(project)?;
    let requested = args
        .bc_version
        .as_deref()
        .map(al_version::BcVersion::parse)
        .transpose()
        .map_err(|e| format!("--bc-version: {e}"))?;
    let oracle_path = args.oracle.clone().unwrap_or_else(|| root.join("oracle/oracle.toml"));
    let newest_verified = std::fs::read_to_string(&oracle_path)
        .ok()
        .and_then(|t| config::OracleFile::parse(&t).ok())
        .and_then(|c| c.select(None).ok().map(|v| v.bc()))
        .unwrap_or_else(|| al_version::BcVersion::new(al_version::MIN_BC_MAJOR, 0));
    let chosen =
        al_version::choose_target(requested.as_ref(), &app_versions, &newest_verified).map_err(|e| e.to_string())?;
    println!("target: BC {}, runtime {}", chosen.target.bc, chosen.target.runtime);
    for note in &chosen.notes {
        eprintln!("note: {note}");
    }
    let provision = deps::Provision {
        cache_dir: args.cache_dir.clone(),
        download: !args.no_download,
        query: target::build_query(args.bc_version.as_deref(), &chosen, &app_versions),
    };
    let deps = deps::load_with(project, &[], &args.deps, Some(&provision))?;
    for line in &deps.log {
        eprintln!("{line}");
    }
    match &deps.build {
        Some(u) => println!("bc build: {} ({})", u.build, origin_word(u.origin)),
        None => println!("bc build: none needed (every dependency came from .alpackages or --deps)"),
    }
    println!("dependencies: {}", deps.summary.join(", "));
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
