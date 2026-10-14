//! Reading the APPs given on the command line: a folder, a source archive or an `.app`.

use al_deps::load::{AppJson, read_package_at_with};
use al_deps::select::{APPLICATION_APP_ID, DepDecl, SYSTEM_APP_ID};
use al_sema::workspace::AppSource;
use al_testrun::runtime::Manifest;
use al_version::AppVersions;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One app whose tests run.
pub struct AppInput {
    pub path: PathBuf,
    pub source: AppSource,
    pub json: AppJson,
    pub manifest: Manifest,
    pub versions: AppVersions,
    /// `preprocessorSymbols` of the app's app.json (an app folder only).
    pub symbols: Vec<String>,
}

/// Reads `path`. `build` is an exact build that fills Microsoft's build variables in a source
/// archive that names none itself.
pub fn load(path: &Path, build: Option<&str>) -> Result<AppInput, String> {
    if !path.exists() {
        return Err(format!("{}: no such file or folder", path.display()));
    }
    if path.is_dir() && !path.join("app.json").is_file() {
        return Err(format!(
            "{}: an app folder needs an app.json (to run a folder of packages, name each package)",
            path.display()
        ));
    }
    let (m, files) = read_package_at_with(path, build)?;
    let mut manifest = Manifest {
        publisher: m.publisher.clone(),
        version: m.version.to_string(),
        resources: Vec::new(),
    };
    let mut versions = AppVersions::parse(
        None,
        m.platform.map(|v| v.to_string()).as_deref(),
        m.application.map(|v| v.to_string()).as_deref(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut symbols = Vec::new();
    if path.is_dir() {
        // The folder's own app.json: it can say more than the package manifest does.
        manifest = al_testrun::runtime::load_manifest(path)?;
        versions = al_testrun::target::read_app_versions(path)?;
        let text = std::fs::read_to_string(path.join("app.json")).map_err(|e| e.to_string())?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("app.json: {e}"))?;
        symbols = json["preprocessorSymbols"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(String::from))
            .collect();
    }
    let source = AppSource {
        id: m.id.clone(),
        name: m.name.clone(),
        dependencies: m.dependencies.iter().map(|d| d.id.clone()).collect(),
        files,
    };
    let json = AppJson {
        id: m.id,
        name: m.name,
        publisher: m.publisher,
        version: m.version,
        platform: m.platform,
        application: m.application,
        dependencies: m.dependencies,
    };
    Ok(AppInput {
        path: path.to_path_buf(),
        source,
        json,
        manifest,
        versions,
        symbols,
    })
}

/// The project the dependency resolution works for: all APPs together. Its dependencies are
/// those of the APPs, minus the APPs themselves (they are in the workspace).
pub fn combined_project(apps: &[AppInput]) -> AppJson {
    let own: BTreeSet<&str> = apps.iter().map(|a| a.json.id.as_str()).collect();
    let mut dependencies: Vec<DepDecl> = Vec::new();
    for a in apps {
        for d in &a.json.dependencies {
            if own.contains(d.id.as_str()) {
                continue;
            }
            match dependencies.iter_mut().find(|x| x.id == d.id) {
                Some(x) => x.min = x.min.max(d.min),
                None => dependencies.push(d.clone()),
            }
        }
    }
    let max = |f: fn(&AppJson) -> Option<al_deps::version::Version>| apps.iter().filter_map(|a| f(&a.json)).max();
    // `platform` and `application` in app.json are dependencies on the Microsoft apps `System` and
    // `Application` (which pulls in Base Application and the rest). The resolver treats them as
    // optional roots; declaring them lets the artifact feed supply them when nothing else does.
    for (id, name, floor) in [
        (SYSTEM_APP_ID, "System", max(|j| j.platform)),
        (APPLICATION_APP_ID, "Application", max(|j| j.application)),
    ] {
        if let Some(min) = floor.filter(|_| !dependencies.iter().any(|d| d.id == id)) {
            dependencies.push(DepDecl {
                id: id.to_string(),
                name: name.to_string(),
                publisher: "Microsoft".to_string(),
                min,
                optional: false,
            });
        }
    }
    AppJson {
        id: "alrunner-project".into(),
        name: "alrunner-project".into(),
        publisher: String::new(),
        version: Default::default(),
        platform: max(|j| j.platform),
        application: max(|j| j.application),
        dependencies,
    }
}

/// Whether `publisher` is Microsoft.
pub fn is_microsoft(publisher: &str) -> bool {
    publisher.eq_ignore_ascii_case("Microsoft")
}

/// The build the tests themselves come from: Microsoft's test archives and packages carry it in
/// the folder they lie in (`.../<build>/platform/...`) or as their version. Ahead of the feed's
/// newest build, the apps, `System.app` and the demo data should come from this one.
pub fn own_build(inputs: &[AppInput]) -> Option<al_deps::build::BuildId> {
    use al_deps::build::BuildId;
    inputs
        .iter()
        .filter(|i| is_microsoft(&i.manifest.publisher))
        .find_map(|i| {
            i.path
                .components()
                .rev()
                .filter_map(|c| c.as_os_str().to_str())
                .find_map(|c| BuildId::parse(c).ok())
                .or_else(|| BuildId::parse(&i.manifest.version).ok())
        })
}
