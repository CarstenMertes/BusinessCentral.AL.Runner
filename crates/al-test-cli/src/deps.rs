//! Dependency apps for a corpus run (ADR-013): the project's `.alpackages` is
//! the first place looked up, then any `--deps` directories.

use al_deps::load::{AppJson, Request, Role, WorkspaceApp, load_dependencies, parse_app_json};
use al_deps::select::MissingPolicy;
use al_sema::workspace::AppSource;
use std::path::{Path, PathBuf};

pub struct Dependencies {
    /// Dependency apps in dependency order.
    pub apps: Vec<AppSource>,
    /// `name version` of each, for the report.
    pub summary: Vec<String>,
    /// One line per app loaded, and one per problem, for the run's log.
    pub log: Vec<String>,
}

fn app_json(dir: &Path) -> Result<AppJson, String> {
    let p = dir.join("app.json");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    parse_app_json(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// Resolves and reads the dependencies of the app in `app_dir`. Apps in
/// `source_dirs` are built from source by the caller and satisfy their own dependents.
///
/// A dependency no package supplies is skipped and logged, not an error: the corpus
/// names two test-library apps that no `.alpackages` holds, and downloading comes
/// later. The objects they would supply stay unresolved and are refused.
pub fn load(app_dir: &Path, source_dirs: &[PathBuf], extra_dirs: &[PathBuf]) -> Result<Dependencies, String> {
    let project = app_json(app_dir)?;
    let mut workspace_apps = Vec::new();
    for d in source_dirs {
        workspace_apps.push(WorkspaceApp {
            json: app_json(d)?,
            origin: format!("source {}", d.display()),
        });
    }
    let mut search_dirs = vec![app_dir.join(".alpackages")];
    search_dirs.extend(extra_dirs.iter().cloned());
    let loaded = load_dependencies(&Request {
        project: &project,
        workspace_apps: &workspace_apps,
        search_dirs: &search_dirs,
        missing: MissingPolicy::Skip,
    })?;
    let mut log = Vec::new();
    log.extend(loaded.notes.iter().map(|n| format!("dependency note: {n}")));
    for s in &loaded.skipped {
        log.push(format!("dependency skipped: {s}"));
    }
    let mut apps = Vec::new();
    let mut summary = Vec::new();
    for d in loaded.apps {
        let files = d.app.files.len();
        if d.role == Role::Platform {
            // The platform's system tables have no row providers yet (ADR-013 step 4); a declared
            // table without behaviour would answer silently wrong, so the app stays unloaded.
            log.push(format!(
                "dependency not loaded: {} {} ({files} files; system tables need providers)",
                d.app.name, d.version
            ));
            continue;
        }
        log.push(format!(
            "dependency: {} {} ({files} files) from {}",
            d.app.name,
            d.version,
            d.path.display()
        ));
        summary.push(format!("{} {}", d.app.name, d.version));
        apps.push(d.app);
    }
    Ok(Dependencies { apps, summary, log })
}
