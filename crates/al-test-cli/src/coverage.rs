//! `--coverage-map` (issue 52): which builtin overloads the corpus tests call, per type, written to
//! `docs/corpus-coverage.md` (and `.json`). The corpus is loaded as `al-sema/tests/corpus_resolve.rs` loads it:
//! the internals fixture, the main test app and the OnPrem test app, without Microsoft's apps; calls are
//! counted in the two test apps only.

use al_sema::coverage::CoverageMap;
use al_sema::{AppSource, Workspace};
use std::path::{Path, PathBuf};

fn al_files(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let p: PathBuf = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                out.push((rel, text));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn app(dir: &Path, prefix: &str) -> Result<AppSource, String> {
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("app.json")).map_err(|e| format!("{}: {e}", dir.display()))?,
    )
    .map_err(|e| format!("{}/app.json: {e}", dir.display()))?;
    let deps = manifest["dependencies"].as_array().cloned().unwrap_or_default();
    Ok(AppSource {
        id: manifest["id"].as_str().unwrap_or_default().into(),
        name: manifest["name"].as_str().unwrap_or_default().into(),
        dependencies: deps.iter().filter_map(|d| d["id"].as_str().map(String::from)).collect(),
        // Paths relative to corpus/tests, so a call site reads `al-language/types/TestText.al:12`.
        files: al_files(dir)?
            .into_iter()
            .map(|(p, t)| (format!("{prefix}/{p}"), t))
            .collect(),
    })
}

/// The coverage map of the corpus under `root/corpus/tests`.
pub fn corpus_coverage(root: &Path) -> Result<CoverageMap, String> {
    let tests = root.join("corpus/tests");
    if !tests.join("al-language/app.json").exists() {
        return Err("corpus submodule not checked out: run `git submodule update --init`".into());
    }
    let ws = Workspace::load(
        vec![
            app(
                &tests.join("al-language-internals-fixture"),
                "al-language-internals-fixture",
            )?,
            app(&tests.join("al-language"), "al-language")?,
            app(&tests.join("al-language-onprem"), "al-language-onprem")?,
        ],
        &al_version::corpus_symbols(27),
    );
    let members = al_sema::members::build(&ws);
    let catalog = al_builtins::catalog::Catalog::load();
    let checked = al_sema::check::check(&ws, &members, &catalog);
    Ok(al_sema::coverage::coverage(&ws, &checked, &catalog, &[1, 2]))
}

/// Writes `docs/corpus-coverage.md` and `docs/corpus-coverage.json` under `root`; returns a one-line summary.
pub fn write_coverage_map(root: &Path, corpus_commit: &str) -> Result<String, String> {
    let map = corpus_coverage(root)?;
    let md = root.join("docs/corpus-coverage.md");
    std::fs::write(&md, map.markdown(corpus_commit)).map_err(|e| format!("{}: {e}", md.display()))?;
    let json = root.join("docs/corpus-coverage.json");
    let text = serde_json::to_string_pretty(&serde_json::json!({ "corpus_commit": corpus_commit, "types": map.types }))
        .map_err(|e| e.to_string())?;
    std::fs::write(&json, text + "\n").map_err(|e| format!("{}: {e}", json.display()))?;
    Ok(format!(
        "coverage map: {} of {} overloads pinned; wrote {} and {}",
        map.pinned(),
        map.total(),
        md.display(),
        json.display()
    ))
}
