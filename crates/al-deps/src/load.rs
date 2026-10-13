//! Finding, choosing and reading the dependency apps of a project (ADR-013).
//!
//! The pool of packages is the project's `.alpackages` first, then any extra
//! directories (`--deps`). Downloading from the artifact feed is a later
//! fallback and is not here yet.

use crate::package::{Manifest, PackageError, Sources, read_info, read_package};
use crate::select::{Candidate, DepDecl, MissingPolicy, Options, Project, SYSTEM_APP_ID, SelectError, resolve};
use crate::version::Version;
use al_sema::workspace::AppSource;
use std::path::{Path, PathBuf};

/// What `app.json` says about an app: enough to resolve its dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppJson {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: Version,
    pub platform: Option<Version>,
    pub application: Option<Version>,
    pub dependencies: Vec<DepDecl>,
}

/// Parses an `app.json`. Versions are minimums; a missing one is 0.0.0.0.
pub fn parse_app_json(text: &str) -> Result<AppJson, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let s = |o: &serde_json::Value, k: &str| o[k].as_str().unwrap_or_default().to_string();
    let ver = |o: &serde_json::Value, k: &str| -> Result<Option<Version>, String> {
        match o[k].as_str().filter(|t| !t.is_empty()) {
            Some(t) => t.parse().map(Some).map_err(|e| format!("`{k}`: {e}")),
            None => Ok(None),
        }
    };
    let mut dependencies = Vec::new();
    for d in v["dependencies"].as_array().map(Vec::as_slice).unwrap_or_default() {
        // Older manifests spell the id `appId`.
        let id = if d["id"].is_string() { s(d, "id") } else { s(d, "appId") };
        dependencies.push(DepDecl {
            id: id.to_ascii_lowercase(),
            name: s(d, "name"),
            publisher: s(d, "publisher"),
            min: ver(d, "version")?.unwrap_or_default(),
            optional: false,
        });
    }
    Ok(AppJson {
        id: s(&v, "id").to_ascii_lowercase(),
        name: s(&v, "name"),
        publisher: s(&v, "publisher"),
        version: ver(&v, "version")?.unwrap_or_default(),
        platform: ver(&v, "platform")?,
        application: ver(&v, "application")?,
        dependencies,
    })
}

/// A package file found in a search directory.
#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    pub manifest: Manifest,
    pub has_source: bool,
}

fn ext_is(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))
}

/// A file a search directory may hold: an `.app`, or a `.zip` (a source archive such as
/// `System Application Test Library.Source.zip`).
fn is_package_file(p: &Path) -> bool {
    ext_is(p, "app") || ext_is(p, "zip")
}

/// Files that are meant to be packages: every `.app` and every `*.Source.zip`.
fn is_named_package(p: &Path) -> bool {
    ext_is(p, "app")
        || p.file_name()
            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".source.zip"))
}

/// Reads the manifest of every `*.app` and `*.zip` file directly inside `dir`. A file that
/// cannot be read is returned as an error line, not dropped: the caller prints it.
pub fn scan_dir(dir: &Path) -> (Vec<Found>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (found, problems);
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_package_file(p))
        .collect();
    paths.sort();
    for path in paths {
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| read_info(&bytes).map_err(|e: PackageError| e.to_string()));
        match read {
            Ok((manifest, has_source)) => found.push(Found {
                path,
                manifest,
                has_source,
            }),
            // A stray `.zip` that is no package is not a problem; a broken `.app` or `.Source.zip` is.
            Err(_) if !is_named_package(&path) => {}
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    (found, problems)
}

/// An app the caller already has as source (built in this run).
#[derive(Debug, Clone)]
pub struct WorkspaceApp {
    pub json: AppJson,
    /// Shown in messages.
    pub origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The platform's `System` app: system tables, no providers yet (ADR-013 step 4).
    Platform,
    Standard,
}

#[derive(Debug, Clone)]
pub struct DependencyApp {
    pub app: AppSource,
    pub publisher: String,
    pub version: Version,
    pub path: PathBuf,
    pub role: Role,
}

#[derive(Debug, Default)]
pub struct Loaded {
    /// Dependency apps in dependency order (dependencies first). Workspace apps are not here.
    pub apps: Vec<DependencyApp>,
    /// Lines worth printing: below-minimum Microsoft apps, skipped optional ones, unreadable files.
    pub notes: Vec<String>,
    /// Dependencies no package satisfied.
    pub skipped: Vec<SelectError>,
}

pub struct Request<'a> {
    pub project: &'a AppJson,
    /// Apps the caller loads from source itself, e.g. the internals fixture.
    pub workspace_apps: &'a [WorkspaceApp],
    /// `<project>/.alpackages` first, then any `--deps` directories.
    pub search_dirs: &'a [PathBuf],
    pub missing: MissingPolicy,
}

/// Chooses the packages that satisfy the project's dependencies and reads their source.
pub fn load_dependencies(req: &Request<'_>) -> Result<Loaded, String> {
    let mut pool: Vec<Candidate> = Vec::new();
    let mut paths: Vec<Option<PathBuf>> = Vec::new();
    let mut notes = Vec::new();
    for w in req.workspace_apps {
        pool.push(Candidate {
            id: w.json.id.clone(),
            name: w.json.name.clone(),
            publisher: w.json.publisher.clone(),
            version: w.json.version,
            has_source: true,
            tier: 0,
            from_workspace: true,
            origin: w.origin.clone(),
            deps: w.json.dependencies.clone(),
            platform: w.json.platform,
            application: w.json.application,
        });
        paths.push(None);
    }
    for (tier, dir) in req.search_dirs.iter().enumerate() {
        let (found, problems) = scan_dir(dir);
        notes.extend(problems);
        for f in found {
            let m = f.manifest;
            pool.push(Candidate {
                id: m.id,
                name: m.name,
                publisher: m.publisher,
                version: m.version,
                has_source: f.has_source,
                tier,
                from_workspace: false,
                origin: f.path.display().to_string(),
                deps: m.dependencies,
                platform: m.platform,
                application: m.application,
            });
            paths.push(Some(f.path));
        }
    }
    let project = Project {
        deps: req.project.dependencies.clone(),
        platform: req.project.platform,
        application: req.project.application,
    };
    let opts = Options {
        missing: req.missing,
        searched: req.search_dirs.iter().map(|d| d.display().to_string()).collect(),
    };
    let plan = resolve(&project, &pool, &opts).map_err(|e| e.to_string())?;
    notes.extend(plan.notes);
    let mut apps = Vec::new();
    for i in plan.order {
        let Some(path) = paths[i].clone() else { continue };
        let c = &pool[i];
        if !c.has_source {
            // Symbols only (or an aggregate such as `Application`): nothing to compile. Its
            // objects stay unresolved, so what needs them is refused. Say so.
            notes.push(format!(
                "{}: `{}` {} carries no AL source; the runtime compiles source (ADR-013), so its objects are unavailable",
                path.display(),
                c.name,
                c.version
            ));
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (manifest, sources): (Manifest, Sources) =
            read_package(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let role = if manifest.id.eq_ignore_ascii_case(SYSTEM_APP_ID) {
            Role::Platform
        } else {
            Role::Standard
        };
        apps.push(DependencyApp {
            app: AppSource {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                dependencies: manifest.dependencies.iter().map(|d| d.id.clone()).collect(),
                files: sources
                    .into_iter()
                    .map(|(p, text)| (format!("{}/{p}", manifest.name), text))
                    .collect(),
            },
            publisher: manifest.publisher.clone(),
            version: manifest.version,
            path,
            role,
        });
    }
    Ok(Loaded {
        apps,
        notes,
        skipped: plan.skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::testutil::{Builder, navx};

    fn manifest(id: &str, name: &str, version: &str, deps: &[(&str, &str, &str)]) -> String {
        let deps: String = deps
            .iter()
            .map(|(id, name, min)| {
                format!(r#"<Dependency Id="{id}" Name="{name}" Publisher="P" MinVersion="{min}" />"#)
            })
            .collect();
        format!(
            r#"<Package><App Id="{id}" Name="{name}" Publisher="P" Version="{version}" /><Dependencies>{deps}</Dependencies></Package>"#
        )
    }

    fn write_app(
        dir: &Path,
        file: &str,
        id: &str,
        name: &str,
        version: &str,
        deps: &[(&str, &str, &str)],
        al: &[(&str, &str)],
    ) {
        let mut b = Builder::new().add("NavxManifest.xml", manifest(id, name, version, deps).as_bytes(), true);
        for (p, t) in al {
            b = b.add(p, t.as_bytes(), true);
        }
        std::fs::write(dir.join(file), navx(&b.finish())).unwrap();
    }

    fn project(deps: &[(&str, &str, &str)]) -> AppJson {
        AppJson {
            id: "p".into(),
            name: "Proj".into(),
            publisher: "P".into(),
            version: Version(1, 0, 0, 0),
            platform: None,
            application: None,
            dependencies: deps
                .iter()
                .map(|(id, name, min)| DepDecl {
                    id: id.to_string(),
                    name: name.to_string(),
                    publisher: "P".into(),
                    min: min.parse().unwrap(),
                    optional: false,
                })
                .collect(),
        }
    }

    #[test]
    fn app_json_dependencies_and_floors_are_read() {
        let a = parse_app_json(
            r#"{"id":"A1B2","name":"N","publisher":"P","version":"1.2.0.0","platform":"27.0.0.0","application":"27.0.0.0",
                "dependencies":[{"id":"C3D4","name":"D","publisher":"Q","version":"27.0.0.0"},{"appId":"e5","name":"O","publisher":"Q","version":"1.0.0.0"}]}"#,
        )
        .unwrap();
        assert_eq!(a.id, "a1b2");
        assert_eq!(a.platform, Some(Version(27, 0, 0, 0)));
        assert_eq!(a.dependencies.len(), 2);
        assert_eq!(a.dependencies[0].id, "c3d4");
        assert_eq!(a.dependencies[1].id, "e5");
        assert!(parse_app_json("{").is_err());
    }

    #[test]
    fn loads_the_chosen_packages_in_dependency_order_with_prefixed_paths() {
        let dir = tempfile::tempdir().unwrap();
        write_app(
            dir.path(),
            "top.app",
            "top",
            "Top",
            "2.0.0.0",
            &[("base", "Base", "1.0.0.0")],
            &[("src/T.al", "codeunit 2 T { }")],
        );
        write_app(
            dir.path(),
            "base.app",
            "base",
            "Base",
            "1.0.0.0",
            &[],
            &[("src/B.al", "codeunit 1 B { }")],
        );
        write_app(
            dir.path(),
            "unused.app",
            "unused",
            "Unused",
            "1.0.0.0",
            &[],
            &[("src/U.al", "codeunit 3 U { }")],
        );
        std::fs::write(dir.path().join("readme.txt"), "x").unwrap();
        let loaded = load_dependencies(&Request {
            project: &project(&[("top", "Top", "2.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap();
        let names: Vec<_> = loaded.apps.iter().map(|a| a.app.name.as_str()).collect();
        assert_eq!(names, ["Base", "Top"]);
        assert_eq!(loaded.apps[1].app.dependencies, ["base"]);
        assert_eq!(loaded.apps[0].app.files[0].0, "Base/src/B.al");
        assert_eq!(loaded.apps[0].app.files[0].1, "codeunit 1 B { }");
    }

    #[test]
    fn a_workspace_app_satisfies_its_dependency_and_is_not_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        write_app(
            dir.path(),
            "base.app",
            "base",
            "Base",
            "1.0.0.0",
            &[],
            &[("src/B.al", "codeunit 1 B { }")],
        );
        let fixture = WorkspaceApp {
            json: AppJson {
                dependencies: vec![DepDecl {
                    id: "base".into(),
                    name: "Base".into(),
                    publisher: "P".into(),
                    min: Version(1, 0, 0, 0),
                    optional: false,
                }],
                ..project(&[])
            },
            origin: "fixture".into(),
        };
        let mut fixture = fixture;
        fixture.json.id = "fix".into();
        let loaded = load_dependencies(&Request {
            project: &project(&[("fix", "Proj", "1.0.0.0")]),
            workspace_apps: &[fixture],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap();
        let names: Vec<_> = loaded.apps.iter().map(|a| a.app.name.as_str()).collect();
        assert_eq!(names, ["Base"]);
    }

    #[test]
    fn a_dependency_with_no_package_is_an_error_naming_the_searched_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_dependencies(&Request {
            project: &project(&[("zzz", "Zed", "1.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap_err();
        assert!(
            err.contains("Zed") && err.contains(&dir.path().display().to_string()),
            "{err}"
        );
    }

    #[test]
    fn the_skip_policy_reports_what_is_missing_and_loads_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        write_app(
            dir.path(),
            "base.app",
            "base",
            "Base",
            "1.0.0.0",
            &[],
            &[("src/B.al", "codeunit 1 B { }")],
        );
        let loaded = load_dependencies(&Request {
            project: &project(&[("zzz", "Zed", "1.0.0.0"), ("base", "Base", "1.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Skip,
        })
        .unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert_eq!(loaded.skipped.len(), 1);
    }

    #[test]
    fn a_corrupt_package_is_reported_and_does_not_hide_the_others() {
        let dir = tempfile::tempdir().unwrap();
        write_app(
            dir.path(),
            "base.app",
            "base",
            "Base",
            "1.0.0.0",
            &[],
            &[("src/B.al", "codeunit 1 B { }")],
        );
        std::fs::write(dir.path().join("broken.app"), b"NAVX nonsense").unwrap();
        let loaded = load_dependencies(&Request {
            project: &project(&[("base", "Base", "1.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert!(
            loaded.notes.iter().any(|n| n.contains("broken.app")),
            "{:?}",
            loaded.notes
        );
    }

    #[test]
    fn a_package_without_source_is_noted_and_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        write_app(dir.path(), "sym.app", "sym", "Sym", "1.0.0.0", &[], &[]);
        let loaded = load_dependencies(&Request {
            project: &project(&[("sym", "Sym", "1.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap();
        assert!(loaded.apps.is_empty());
        assert!(
            loaded
                .notes
                .iter()
                .any(|n| n.contains("Sym") && n.contains("no AL source")),
            "{:?}",
            loaded.notes
        );
    }

    #[test]
    fn a_source_zip_in_a_search_directory_satisfies_a_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let app_json = r#"{"id":"zz","name":"Zed","publisher":"P","version":"2.0.0.0","dependencies":[]}"#;
        let zip = Builder::new()
            .add("app.json", app_json.as_bytes(), true)
            .add("Sub/Z.Codeunit.al", b"codeunit 9 Z { }", true)
            .finish();
        std::fs::write(dir.path().join("Zed.Source.zip"), zip).unwrap();
        std::fs::write(dir.path().join("notes.zip"), b"not an archive").unwrap();
        let loaded = load_dependencies(&Request {
            project: &project(&[("zz", "Zed", "2.0.0.0")]),
            workspace_apps: &[],
            search_dirs: &[dir.path().to_path_buf()],
            missing: MissingPolicy::Error,
        })
        .unwrap();
        assert_eq!(loaded.apps.len(), 1);
        assert_eq!(loaded.apps[0].app.files[0].0, "Zed/Sub/Z.Codeunit.al");
        // A stray .zip that is no package is ignored; only `*.Source.zip` files are reported when broken.
        assert!(
            loaded.notes.iter().all(|n| !n.contains("notes.zip")),
            "{:?}",
            loaded.notes
        );
    }

    #[test]
    fn a_broken_source_zip_is_reported_like_a_broken_app() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Broken.Source.zip"), b"not an archive").unwrap();
        let (found, problems) = scan_dir(dir.path());
        assert!(found.is_empty());
        assert!(problems.iter().any(|p| p.contains("Broken.Source.zip")), "{problems:?}");
    }
}
