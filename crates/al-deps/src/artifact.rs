//! Dependency apps from Microsoft's artifact feed, as the fallback behind the project's
//! `.alpackages` and `--deps` (ADR-013 decisions 1 and 2, and the acceptance answer 2).
//!
//! The flow: resolve with what is on disk (plus the cache of the selected build). For every
//! dependency nobody supplies, pick the BC build (Layer A, [`crate::build`]), read the platform
//! artifact's central directory by range, fetch only the `<App>.Source.zip` entries of the missing
//! apps, check their CRC-32, save them in the cache, and resolve again. A second round picks up what
//! the first round's apps depend on (System Application Test Library needs Any).
//!
//! Which entry holds an app: the platform artifact keeps one `Applications/**/<App name>.Source.zip`
//! per app (and a precompiled `Microsoft_<App name>.app` for the test framework). The file name
//! is a hint; the `app.json` inside decides, by app id.

use crate::build::{BuildId, BuildQuery, Origin, Selection, parse_index, select_build};
use crate::cache::{BuildDir, Store, local_builds, local_platform_files};
use crate::load::{Loaded, Planned, Request, ScanCache, plan_dependencies, read_planned};
use crate::package::{Vars, read_info_with};
use crate::remote::{HttpSource, RangeSource, RemoteError, RemoteZip, fetch_all, http_agent};
use crate::select::{DepDecl, MICROSOFT_PLATFORM_APP_IDS, MissingPolicy, SelectError};
use crate::zip::Entry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Microsoft's public artifact CDN, sandbox channel (AL Runner's `ArtifactDownloader.CdnBase`).
pub const DEFAULT_CDN: &str = "https://bcartifacts-exdbf9fwegejdqak.b02.azurefd.net/sandbox";

/// Where the artifacts come from.
pub trait Cdn {
    type Src: RangeSource;
    /// Where the feed is, for messages.
    fn base_url(&self) -> String;
    /// The list of published builds (`indexes/w1.json`).
    fn index(&self) -> Result<Vec<u8>, RemoteError>;
    /// The platform artifact of a build: its URL, for messages, and a range source for it.
    fn platform(&self, build: &BuildId) -> (String, Self::Src);
    /// The country artifact of a build (`w1`): its URL, for messages, and a range source for it.
    /// It holds the demo database backup (ADR-020).
    fn country(&self, build: &BuildId, country: &str) -> (String, Self::Src);
}

pub struct HttpCdn {
    base: String,
    agent: ureq::Agent,
}

impl HttpCdn {
    pub fn new(base: &str) -> HttpCdn {
        HttpCdn {
            base: base.trim_end_matches('/').to_string(),
            agent: http_agent(),
        }
    }

    /// `ALRUNNER_ARTIFACT_URL` if set (a mirror the user runs), else Microsoft's CDN.
    pub fn from_env() -> HttpCdn {
        let base = std::env::var("ALRUNNER_ARTIFACT_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CDN.to_string());
        HttpCdn::new(&base)
    }
}

impl Cdn for HttpCdn {
    type Src = HttpSource;

    fn base_url(&self) -> String {
        self.base.clone()
    }

    fn index(&self) -> Result<Vec<u8>, RemoteError> {
        fetch_all(&self.agent, &format!("{}/indexes/w1.json", self.base))
    }

    fn platform(&self, build: &BuildId) -> (String, HttpSource) {
        let url = format!("{}/{build}/platform", self.base);
        (url.clone(), HttpSource::new(self.agent.clone(), url))
    }

    fn country(&self, build: &BuildId, country: &str) -> (String, HttpSource) {
        let url = format!("{}/{build}/{country}", self.base);
        (url.clone(), HttpSource::new(self.agent.clone(), url))
    }
}

/// How a run uses the feed.
pub struct Fetch {
    /// The cache root (see [`crate::cache::cache_root`]).
    pub cache_root: PathBuf,
    /// False for `--no-download`: only the cache is consulted.
    pub download: bool,
    pub query: BuildQuery,
    /// A folder of extracted artifacts, one folder per build (`<build>/platform/...`), as
    /// BcContainerHelper keeps them in `~/.bcartifacts.cache/sandbox`. A build the query names
    /// exactly is taken from there before the feed is asked, and any matching build there is the
    /// last resort when the feed has none or cannot be reached.
    pub local_artifacts: Option<PathBuf>,
}

/// The build a run took Microsoft's apps from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactUse {
    pub build: BuildId,
    pub origin: Origin,
    /// Entries fetched from the feed in this run (empty when the cache sufficed).
    pub fetched: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Provisioned {
    pub loaded: Loaded,
    /// Set when a build was consulted: some dependency was not in `.alpackages` or `--deps`.
    pub artifact: Option<ArtifactUse>,
    /// Lines for the run's log: the build chosen, what was fetched, what stays missing and why.
    pub log: Vec<String>,
}

/// The entries of the platform artifact that could hold `dep`, best first. Only Microsoft's apps
/// are in the artifact, so a dependency by another publisher has none.
pub fn find_entries<'a>(entries: &'a [Entry], dep: &DepDecl) -> Vec<&'a Entry> {
    rank_by_name(entries, |e| e.name.as_str(), dep)
}

/// [`find_entries`] over anything with an entry name: the archive's entries, or the files of an
/// extracted artifact.
fn rank_by_name<'a, T>(items: &'a [T], name_of: impl Fn(&T) -> &str, dep: &DepDecl) -> Vec<&'a T> {
    if !dep.publisher.is_empty() && !dep.publisher.eq_ignore_ascii_case("Microsoft") {
        return Vec::new();
    }
    let base = |t: &T| {
        let n = name_of(t);
        n.rsplit('/').next().unwrap_or(n).to_ascii_lowercase()
    };
    let name = dep.name.to_ascii_lowercase();
    let source = format!("{name}.source.zip");
    let compiled = format!("microsoft_{name}.app");
    let mut out: Vec<&T> = items.iter().filter(|e| base(e) == source).collect();
    out.extend(items.iter().filter(|e| base(e) == compiled));
    // The platform itself (`System`) is not an application: its package sits in the archive's
    // `ModernDev/pfiles/microsoft dynamics nav/<version>/al development environment/` folder.
    if name == "system" {
        out.extend(items.iter().filter(|e| {
            name_of(e)
                .to_ascii_lowercase()
                .ends_with("/al development environment/system.app")
        }));
    }
    out
}

fn dep_key(d: &DepDecl) -> String {
    if d.id.is_empty() {
        format!("{}/{}", d.publisher, d.name).to_ascii_lowercase()
    } else {
        d.id.to_ascii_lowercase()
    }
}

/// Whether the app inside an entry is the one `dep` names (the file name is a hint, the manifest
/// decides). Says why not in `log`.
fn entry_is(bytes: &[u8], name: &str, dep: &DepDecl, build: &BuildId, log: &mut Vec<String>) -> bool {
    match read_info_with(bytes, &Vars::for_build(build.as_str()).unwrap_or_default()) {
        Ok((m, _)) if dep.id.is_empty() || m.id.eq_ignore_ascii_case(&dep.id) => true,
        Ok((m, _)) => {
            log.push(format!(
                "artifact: `{name}` holds {} ({}), not {} ({}); not used",
                m.name, m.id, dep.name, dep.id
            ));
            false
        }
        Err(e) => {
            log.push(format!("artifact: `{name}` is not a readable package: {e}; not used"));
            false
        }
    }
}

/// A build of the local artifacts that the query names, with its files.
struct LocalPlatform {
    files: Vec<(String, PathBuf)>,
}

/// The build of `root` that the query picks. With `exact_only`, only a query that names one whole
/// build (four numbers) picks, and it picks that build or none.
fn pick_local(root: &Path, q: &BuildQuery, exact_only: bool) -> Option<Selection> {
    let builds = local_builds(root);
    if exact_only && !q.exact().is_some_and(|e| builds.contains(&e)) {
        return None;
    }
    let mut sel = select_build(q, &builds, None, &root.display().to_string()).ok()?;
    sel.origin = Origin::Local;
    Some(sel)
}

/// The newest published build that satisfies the query, ignoring the cache. CI uses it to key its
/// artifact cache by build before anything is downloaded.
pub fn newest_published<C: Cdn>(cdn: &C, query: &BuildQuery) -> Result<Selection, String> {
    let published = published_builds(cdn)?;
    select_build(query, &[], Some(&published), "(none consulted)").map_err(|e| e.to_string())
}

fn published_builds<C: Cdn>(cdn: &C) -> Result<Vec<BuildId>, String> {
    let bytes = cdn.index().map_err(|e| format!("artifact index: {e}"))?;
    parse_index(&String::from_utf8_lossy(&bytes))
}

/// Resolves the dependencies of `req.project`, downloading from the feed what no directory supplies.
///
/// `req.missing` is the caller's policy for what is still missing at the end: `Error` fails the
/// run, `Skip` records it in `Loaded::skipped`.
pub fn load_with_artifacts<C: Cdn>(req: &Request<'_>, fetch: &Fetch, cdn: &C) -> Result<Provisioned, String> {
    /// Each round fetches at least one app or ends the loop; the chain of test libraries is short.
    const ROUNDS: usize = 8;
    let store = Store::new(&fetch.cache_root);
    let mut log: Vec<String> = Vec::new();
    let mut chosen: Option<(BuildDir, Selection)> = None;
    let mut remote: Option<(String, RemoteZip<C::Src>)> = None;
    // Set when the chosen build is read from the local artifacts instead of the feed.
    let mut local: Option<LocalPlatform> = None;
    let mut local_files: Vec<PathBuf> = Vec::new();
    let mut fetched: Vec<String> = Vec::new();
    let mut tried: HashSet<String> = HashSet::new();
    let mut scans = ScanCache::new();
    let mut planned: Option<Planned> = None;
    for _ in 0..ROUNDS {
        let mut dirs = req.search_dirs.to_vec();
        if let Some((bd, _)) = &chosen {
            if local.is_none() {
                let (_, notes) = bd.verify();
                log.extend(notes);
                dirs.push(bd.apps_dir());
            }
        }
        dirs.extend(local_files.iter().cloned());
        // Once a build is chosen, Microsoft's per-build apps of another major in the search directories (the corpus's
        // `.alpackages` holds the BC 27.5 ones) do not stand in for the chosen build's: a run for another major
        // loads that major's Base Application, System Application and so on from the build, so that what the apps
        // answer (`Application System Constants`) is the major's own. Same major: the project's package stays.
        if let Some((_, sel)) = &chosen {
            let major = sel.build.major();
            for dir in &dirs {
                if let Some((found, _)) = scans.get_mut(dir) {
                    found.retain(|f| {
                        let per_build = MICROSOFT_PLATFORM_APP_IDS
                            .iter()
                            .any(|id| id.eq_ignore_ascii_case(&f.manifest.id));
                        !(per_build
                            && f.manifest.publisher.eq_ignore_ascii_case("Microsoft")
                            && f.manifest.version.0 != major)
                    });
                }
            }
        }
        // Only the choice is made here; the sources are read once, after the last round.
        let round = plan_dependencies(
            &Request {
                project: req.project,
                workspace_apps: req.workspace_apps,
                search_dirs: &dirs,
                missing: MissingPolicy::Skip,
                source_build: req.source_build,
                application_implies_dependencies: req.application_implies_dependencies,
                lazy_sources: req.lazy_sources,
            },
            &mut scans,
        )?;
        let required: Vec<DepDecl> = round
            .skipped
            .iter()
            .filter_map(|e| match e {
                SelectError::Missing { dep, .. } => Some(dep.clone()),
                _ => None,
            })
            .filter(|d| !tried.contains(&dep_key(d)))
            // The feed holds only Microsoft's apps: do not select a build for anything else.
            .filter(|d| d.publisher.is_empty() || d.publisher.eq_ignore_ascii_case("Microsoft"))
            .collect();
        // When a build is needed anyway, the optional roots (the platform's `System`, the `Application` floor) come from it
        // too if no directory holds them: without `System` the platform tables are missing, and a run whose cache another
        // run had filled would see something else than one on a fresh machine. Nothing is requested for them alone.
        let mut seen_ids: HashSet<String> = required.iter().map(|d| d.id.to_ascii_lowercase()).collect();
        let optional: Vec<DepDecl> = round
            .optional_missing
            .iter()
            .filter(|_| !required.is_empty())
            .filter(|d| !tried.contains(&dep_key(d)))
            .filter(|d| d.publisher.is_empty() || d.publisher.eq_ignore_ascii_case("Microsoft"))
            .filter(|d| seen_ids.insert(d.id.to_ascii_lowercase()))
            .cloned()
            .collect();
        let todo: Vec<DepDecl> = required.into_iter().chain(optional).collect();
        planned = Some(round);
        if todo.is_empty() {
            break;
        }
        let cache_dir = store.dir().display().to_string();
        let local_root = fetch.local_artifacts.as_deref();
        let choose_local = |sel: Selection, local: &mut Option<LocalPlatform>, root: &Path, log: &mut Vec<String>| {
            log.extend(sel.warnings.iter().map(|w| format!("warning: {w}")));
            *local = Some(LocalPlatform {
                files: local_platform_files(root, &sel.build),
            });
            Some((store.build(&sel.build), sel))
        };
        if chosen.is_none() {
            // Layer A, first look: the cache. A cached build needs no network to be chosen.
            let cached = store.list_builds();
            // The build of the tests, when the query names one, must itself be cached: another
            // cached build waits until the build of the tests is looked for on disk and on the feed.
            let usable = fetch.query.exact().is_none_or(|e| cached.contains(&e));
            if let Some(sel) = usable
                .then(|| select_build(&fetch.query, &cached, None, &cache_dir).ok())
                .flatten()
            {
                log.extend(sel.warnings.iter().map(|w| format!("warning: {w}")));
                chosen = Some((store.build(&sel.build), sel));
                continue; // resolve again with the cached build's apps in the pool
            }
            // A build named exactly and found on disk is used without asking the feed.
            if let Some((root, sel)) = local_root.and_then(|r| pick_local(r, &fetch.query, true).map(|s| (r, s))) {
                chosen = choose_local(sel, &mut local, root, &mut log);
            }
        }
        if chosen.is_none() && !fetch.download {
            // Offline: any cached build, else any matching build on disk.
            if let Ok(sel) = select_build(&fetch.query, &store.list_builds(), None, &cache_dir) {
                log.extend(sel.warnings.iter().map(|w| format!("warning: {w}")));
                chosen = Some((store.build(&sel.build), sel));
                continue;
            }
            if let Some((root, sel)) = local_root.and_then(|r| pick_local(r, &fetch.query, false).map(|s| (r, s))) {
                chosen = choose_local(sel, &mut local, root, &mut log);
            }
        }
        if local.is_none() && !fetch.download {
            break;
        }
        if chosen.is_none() {
            let picked = published_builds(cdn).and_then(|published| {
                select_build(&fetch.query, &store.list_builds(), Some(&published), &cache_dir)
                    .map_err(|e| e.to_string())
            });
            match picked {
                Ok(sel) => {
                    log.extend(sel.warnings.iter().map(|w| format!("warning: {w}")));
                    chosen = Some((store.build(&sel.build), sel));
                }
                // The feed has no such build (it keeps recent ones) or cannot be reached: a
                // matching build already on disk still answers.
                Err(e) => match local_root.and_then(|r| pick_local(r, &fetch.query, false).map(|s| (r, s))) {
                    Some((root, sel)) => {
                        log.push(format!("artifact: {e}; using the local artifacts instead"));
                        chosen = choose_local(sel, &mut local, root, &mut log);
                    }
                    None => return Err(e),
                },
            }
        }
        if let Some(l) = &local {
            let (_, sel) = chosen.as_ref().expect("a build is chosen");
            let mut progress = false;
            for dep in &todo {
                tried.insert(dep_key(dep));
                let mut found = false;
                for (name, path) in rank_by_name(&l.files, |f| f.0.as_str(), dep) {
                    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    if !entry_is(&bytes, name, dep, &sel.build, &mut log) {
                        continue;
                    }
                    log.push(format!("artifact: using {name} from {}", path.display()));
                    fetched.push(name.clone());
                    local_files.push(path.clone());
                    found = true;
                    break;
                }
                if found {
                    progress = true;
                } else {
                    log.push(format!(
                        "artifact: BC build {} has no entry for `{}` ({})",
                        sel.build, dep.name, dep.id
                    ));
                }
            }
            if !progress {
                break;
            }
            continue;
        }
        let (bd, sel) = chosen.as_ref().expect("a build is chosen");
        if remote.is_none() {
            let (url, src) = cdn.platform(&sel.build);
            let zip = match bd.load_tail() {
                Some((total, tail)) => RemoteZip::from_tail(src, total, tail).map_err(|e| format!("{url}: {e}"))?,
                None => {
                    let zip = RemoteZip::open(src).map_err(|e| format!("{url}: {e}"))?;
                    let (total, tail) = zip.tail();
                    bd.save_tail(&url, total, tail)?;
                    zip
                }
            };
            remote = Some((url, zip));
        }
        let (url, zip) = remote.as_ref().expect("the archive is open");
        let mut progress = false;
        for dep in &todo {
            tried.insert(dep_key(dep));
            let mut stored = false;
            for entry in find_entries(&zip.entries, dep) {
                let bytes = zip
                    .read(entry)
                    .map_err(|e| format!("{url}: entry `{}`: {e}", entry.name))?;
                if !entry_is(&bytes, &entry.name, dep, &sel.build, &mut log) {
                    continue;
                }
                bd.put_entry(&entry.name, url, &bytes)?;
                // The directory has a file more than the last look found.
                scans.remove(&bd.apps_dir());
                log.push(format!(
                    "artifact: fetched {} ({} bytes) from {url}",
                    entry.name,
                    bytes.len()
                ));
                fetched.push(entry.name.clone());
                stored = true;
                break;
            }
            if stored {
                progress = true;
            } else {
                log.push(format!(
                    "artifact: BC build {} has no entry for `{}` ({})",
                    sel.build, dep.name, dep.id
                ));
            }
        }
        if !progress {
            break;
        }
    }
    let final_req = Request {
        project: req.project,
        workspace_apps: req.workspace_apps,
        search_dirs: &[],
        missing: MissingPolicy::Skip,
        source_build: req.source_build,
        application_implies_dependencies: req.application_implies_dependencies,
        lazy_sources: req.lazy_sources,
    };
    let loaded = read_planned(planned.expect("at least one round ran"), &final_req)?;
    let used = chosen.map(|(_, sel)| {
        let how = match sel.origin {
            Origin::Cache => "cache",
            Origin::Published => "downloaded",
            Origin::Local => "local artifacts",
        };
        log.insert(0, format!("artifact: BC build {} ({how})", sel.build));
        ArtifactUse {
            build: sel.build,
            origin: sel.origin,
            fetched,
        }
    });
    if !loaded.skipped.is_empty() {
        // Name what was missing and where it would have come from (ADR-013 decision 1).
        let from = match &used {
            Some(u) => format!("BC build {} at {}/{}/platform", u.build, cdn.base_url(), u.build),
            None => format!("a BC build matching {} at {}", fetch.query.prefix, cdn.base_url()),
        };
        let missing: Vec<String> = loaded
            .skipped
            .iter()
            .map(|e| {
                if fetch.download {
                    e.to_string()
                } else {
                    format!("{e}; downloading is off (--no-download); Microsoft's apps would come from {from}")
                }
            })
            .collect();
        if req.missing == MissingPolicy::Error {
            return Err(missing.join("\n"));
        }
        if !fetch.download {
            log.extend(missing.into_iter().map(|m| format!("artifact: {m}")));
        }
    }
    Ok(Provisioned {
        loaded,
        artifact: used,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::{AppJson, Request};
    use crate::remote::FileSource;
    use crate::version::Version;
    use crate::zip::testutil::{Builder, navx};
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::path::Path;

    const SA: &str = "63ca2fa4-4f03-4f2b-a480-172fef340d3f";
    const SATL: &str = "9856ae4f-d1a7-46ef-89bb-6ef056398228";
    const MOCK: &str = "40860557-a18d-42ad-aecb-22b7dd80dc80";
    const ANY: &str = "e7320ebb-08b3-4406-b1ec-b4927d3e280b";
    const SYS: &str = "8874ed3a-0643-4247-9ced-7a7002f7135d";
    const SYSTEM_APP_ENTRY: &str = "ModernDev/pfiles/microsoft dynamics nav/270/al development environment/System.app";

    /// The platform's `System.app`: a package with a manifest and a little source.
    fn system_app() -> Vec<u8> {
        let manifest = format!(
            r#"<Package><App Id="{SYS}" Name="System" Publisher="Microsoft" Version="27.0.0.0" /><Dependencies/></Package>"#
        );
        navx(
            &Builder::new()
                .add("NavxManifest.xml", manifest.as_bytes(), true)
                .add("src/Sys.al", b"table 2000000000 \"Sys\" { }", true)
                .finish(),
        )
    }

    fn app_json(id: &str, name: &str, deps: &[(&str, &str)]) -> String {
        let deps: Vec<String> = deps
            .iter()
            .map(|(i, n)| format!(r#"{{"id":"{i}","name":"{n}","publisher":"Microsoft","version":"27.5.0.0"}}"#))
            .collect();
        format!(
            r#"{{"id":"{id}","name":"{name}","publisher":"Microsoft","version":"27.5.46862.56249","platform":"27.0.0.0","dependencies":[{}]}}"#,
            deps.join(",")
        )
    }

    fn source_zip(json: &str, al: &[(&str, &str)]) -> Vec<u8> {
        let mut b = Builder::new().add("app.json", json.as_bytes(), true);
        for (p, t) in al {
            b = b.add(p, t.as_bytes(), true);
        }
        b.finish()
    }

    /// A platform artifact in miniature: the entries the corpus needs, under the paths the real one
    /// uses, around filler that must never be fetched.
    fn artifact_bytes(corrupt_mock: bool, wrong_id_for_any: bool) -> Vec<u8> {
        let satl = source_zip(
            &app_json(
                SATL,
                "System Application Test Library",
                &[(SA, "System Application"), (ANY, "Any")],
            ),
            &[(
                "Azure AD Plan/Plan.Codeunit.al",
                "codeunit 132916 \"Azure AD Plan Test Library\" { }",
            )],
        );
        let any_id = if wrong_id_for_any {
            "ffffffff-0000-0000-0000-000000000000"
        } else {
            ANY
        };
        let any = source_zip(
            &app_json(any_id, "Any", &[]),
            &[("Any.Codeunit.al", "codeunit 130500 Any { }")],
        );
        let mock = source_zip(
            &app_json(MOCK, "Permissions Mock", &[]),
            &[("M.Codeunit.al", "codeunit 131006 \"Permissions Mock\" { }")],
        );
        let sa_src = source_zip(
            &app_json(SA, "System Application", &[]),
            &[("S.Codeunit.al", "codeunit 1 S { }")],
        );
        let base_json = format!(
            r#"{{"id":"437dbf0e-84ff-417a-965d-ed2bb9650972","name":"Base Application","publisher":"Microsoft",
            "version":"$(app_currentVersion)","platform":"$(app_platformVersion)",
            "dependencies":[{{"id":"{SA}","name":"System Application","publisher":"Microsoft","version":"$(app_minimumVersion)"}}]}}"#
        );
        let base = source_zip(&base_json, &[("T.Table.al", "table 18 Customer { }")]);
        let mut b = Builder::new();
        b = b.add(SYSTEM_APP_ENTRY, &system_app(), false);
        b = b.add("Applications/BaseApp/Source/Base Application.Source.zip", &base, false);
        for i in 0..40 {
            b = b.add(&format!("filler/{i}.bin"), &vec![i as u8; 4000], false);
        }
        b = b
            .add(
                "Applications/System Application/Test/System Application Test Library.Source.zip",
                &satl,
                false,
            )
            .add(
                "Applications/TestFramework/TestLibraries/Any/Any.Source.zip",
                &any,
                false,
            )
            .add(
                "Applications/System Application/Source/System Application.Source.zip",
                &sa_src,
                false,
            );
        if corrupt_mock {
            b = b.add_with_crc(
                "Applications/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip",
                &mock,
                7,
            );
        } else {
            b = b.add(
                "Applications/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip",
                &mock,
                true,
            );
        }
        b.finish()
    }

    struct FixtureCdn {
        index: String,
        platform: HashMap<String, PathBuf>,
        index_calls: Cell<u32>,
        platform_calls: Cell<u32>,
    }

    impl FixtureCdn {
        fn new(dir: &Path, builds: &[&str]) -> FixtureCdn {
            let index = format!(
                "[{}]",
                builds
                    .iter()
                    .map(|b| format!(r#"{{"Version":"{b}","CreationTime":"2026-10-01T00:00:00Z"}}"#))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut platform = HashMap::new();
            for b in builds {
                let p = dir.join(format!("platform-{b}.zip"));
                std::fs::write(&p, artifact_bytes(false, false)).unwrap();
                platform.insert(b.to_string(), p);
            }
            FixtureCdn {
                index,
                platform,
                index_calls: Cell::new(0),
                platform_calls: Cell::new(0),
            }
        }
    }

    impl Cdn for FixtureCdn {
        type Src = FileSource;
        fn base_url(&self) -> String {
            "https://cdn.test".into()
        }
        fn index(&self) -> Result<Vec<u8>, RemoteError> {
            self.index_calls.set(self.index_calls.get() + 1);
            Ok(self.index.clone().into_bytes())
        }
        fn platform(&self, build: &BuildId) -> (String, FileSource) {
            self.platform_calls.set(self.platform_calls.get() + 1);
            let p = self
                .platform
                .get(build.as_str())
                .cloned()
                .unwrap_or_else(|| PathBuf::from("/nonexistent/platform.zip"));
            (format!("https://cdn.test/{build}/platform"), FileSource(p))
        }
        fn country(&self, _: &BuildId, _: &str) -> (String, FileSource) {
            panic!("the country artifact was requested")
        }
    }

    /// A feed nobody may call.
    struct NoNetwork;
    impl Cdn for NoNetwork {
        type Src = FileSource;
        fn base_url(&self) -> String {
            "https://cdn.test".into()
        }
        fn index(&self) -> Result<Vec<u8>, RemoteError> {
            panic!("the feed index was requested")
        }
        fn platform(&self, _: &BuildId) -> (String, FileSource) {
            panic!("the platform artifact was requested")
        }
        fn country(&self, _: &BuildId, _: &str) -> (String, FileSource) {
            panic!("the country artifact was requested")
        }
    }

    fn dep(id: &str, name: &str) -> DepDecl {
        DepDecl {
            id: id.into(),
            name: name.into(),
            publisher: "Microsoft".into(),
            min: Version(27, 0, 0, 0),
            optional: false,
        }
    }

    /// The corpus's shape: System Application is in `.alpackages`; the two test libraries are not.
    fn project() -> AppJson {
        AppJson {
            id: "p".into(),
            name: "Proj".into(),
            publisher: "P".into(),
            version: Version(1, 0, 0, 0),
            platform: Some(Version(27, 0, 0, 0)),
            application: None,
            dependencies: vec![
                dep(SA, "System Application"),
                dep(SATL, "System Application Test Library"),
                dep(MOCK, "Permissions Mock"),
            ],
        }
    }

    fn alpackages(dir: &Path) -> PathBuf {
        let d = dir.join(".alpackages");
        std::fs::create_dir_all(&d).unwrap();
        let manifest = format!(
            r#"<Package><App Id="{SA}" Name="System Application" Publisher="Microsoft" Version="27.5.46862.48827" /><Dependencies/></Package>"#
        );
        let zip = Builder::new()
            .add("NavxManifest.xml", manifest.as_bytes(), true)
            .add("src/S.al", b"codeunit 1 S { }", true)
            .finish();
        std::fs::write(d.join("System Application.app"), navx(&zip)).unwrap();
        d
    }

    fn fetch(cache: &Path, download: bool, requested: Option<&str>) -> Fetch {
        Fetch {
            cache_root: cache.to_path_buf(),
            download,
            query: BuildQuery::new(requested, 27, Some((27, 0))),
            local_artifacts: None,
        }
    }

    fn run<C: Cdn>(
        alpkg: &Path,
        f: &Fetch,
        cdn: &C,
        missing: MissingPolicy,
        project: &AppJson,
    ) -> Result<Provisioned, String> {
        load_with_artifacts(
            &Request {
                project,
                workspace_apps: &[],
                search_dirs: &[alpkg.to_path_buf()],
                missing,
                source_build: None,
                application_implies_dependencies: false,
                lazy_sources: false,
            },
            f,
            cdn,
        )
    }

    fn names(p: &Provisioned) -> Vec<&str> {
        p.loaded.apps.iter().map(|a| a.app.name.as_str()).collect()
    }

    /// Owner decision 2026-10-15: a symbol package in `.alpackages` (what VS Code downloads) does not
    /// count as found. The search goes on to the feed, for the same app.
    #[test]
    fn a_symbol_package_in_alpackages_does_not_stop_the_search_the_feed_supplies_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpkg).unwrap();
        let manifest = format!(
            r#"<Package><App Id="{SA}" Name="System Application" Publisher="Microsoft" Version="27.5.46862.48827" /><Dependencies/></Package>"#
        );
        // What VS Code downloads: the manifest and the symbols, no AL source.
        let zip = Builder::new()
            .add("NavxManifest.xml", manifest.as_bytes(), true)
            .add("SymbolReference.json", b"{}", true)
            .finish();
        std::fs::write(alpkg.join("System Application.app"), navx(&zip)).unwrap();
        let p = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Skip,
            &project(),
        )
        .unwrap();
        let sa: Vec<_> = p
            .loaded
            .apps
            .iter()
            .filter(|a| a.app.name == "System Application")
            .collect();
        assert_eq!(sa.len(), 1, "{:?}", names(&p));
        assert!(
            !sa[0].path.starts_with(&alpkg),
            "the source came from the feed, not the symbol package"
        );
        assert!(
            sa[0].app.files.iter().any(|(_, t)| t.contains("codeunit")),
            "it carries source"
        );
        assert!(p.loaded.skipped.is_empty(), "{:?}", p.loaded.skipped);
    }

    /// Offline, the symbol package alone leaves the dependency missing, with the reason.
    #[test]
    fn without_the_feed_a_symbol_package_leaves_the_dependency_missing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpkg).unwrap();
        let manifest = format!(
            r#"<Package><App Id="{SA}" Name="System Application" Publisher="Microsoft" Version="27.5.46862.48827" /><Dependencies/></Package>"#
        );
        let zip = Builder::new()
            .add("NavxManifest.xml", manifest.as_bytes(), true)
            .add("SymbolReference.json", b"{}", true)
            .finish();
        std::fs::write(alpkg.join("System Application.app"), navx(&zip)).unwrap();
        let p = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), false, Some("27.5")),
            &NoNetwork,
            MissingPolicy::Skip,
            &project(),
        )
        .unwrap();
        assert!(!names(&p).contains(&"System Application"));
        let text: Vec<String> = p.loaded.skipped.iter().map(|e| e.to_string()).collect();
        assert!(
            text.iter()
                .any(|t| t.contains("System Application") && t.contains("without AL source")),
            "{text:?}"
        );
    }

    #[test]
    fn missing_test_libraries_are_fetched_by_range_cached_and_resolved_with_their_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(
            dir.path(),
            &["27.4.1.1", "27.5.46862.55978", "27.5.46862.56249", "28.1.1.1"],
        );
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        let p = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        // Dependencies first. `Any` was not in the project's list: the second round found it.
        assert_eq!(
            names(&p),
            [
                "System",
                "System Application",
                "Any",
                "System Application Test Library",
                "Permissions Mock"
            ]
        );
        let used = p.artifact.as_ref().unwrap();
        assert_eq!(used.build.to_string(), "27.5.46862.56249", "the newest 27.5 build");
        assert_eq!(used.origin, Origin::Published);
        assert_eq!(used.fetched.len(), 4, "the three libraries and the platform's System");
        // Source comes from the downloaded archives with the app name as the path prefix.
        let satl = p
            .loaded
            .apps
            .iter()
            .find(|a| a.app.name.starts_with("System Application T"))
            .unwrap();
        assert_eq!(
            satl.app.files[0].0,
            "System Application Test Library/Azure AD Plan/Plan.Codeunit.al"
        );
        // The cache holds the tail and the three entries, with records.
        let b = cache.join("artifacts/27.5.46862.56249");
        assert!(b.join("platform.tail").is_file());
        for f in [
            "Any.Source.zip",
            "Permissions Mock.Source.zip",
            "System Application Test Library.Source.zip",
        ] {
            assert!(b.join("apps").join(f).is_file(), "{f}");
            assert!(b.join("apps").join(format!("{f}.json")).is_file(), "{f}.json");
        }
        // System Application was in .alpackages and was not downloaded.
        assert!(!b.join("apps/System Application.Source.zip").exists());
        assert!(
            p.log
                .iter()
                .any(|l| l.contains("27.5.46862.56249") && l.contains("downloaded")),
            "{:?}",
            p.log
        );
    }

    #[test]
    fn a_source_archive_with_build_variables_is_fetched_and_filled_from_the_selected_build() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let mut p = project();
        p.dependencies = vec![dep("437dbf0e-84ff-417a-965d-ed2bb9650972", "Base Application")];
        let cache = dir.path().join("cache");
        let out = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &p,
        )
        .unwrap();
        // The platform's `System` is an optional root of the project: with a build chosen anyway, it comes from it.
        assert_eq!(names(&out), ["System", "System Application", "Base Application"]);
        assert_eq!(out.loaded.apps[2].version.to_string(), "27.5.46862.56249");
        // Warm: the record beside the cached file names the build, so the variables fill again.
        let again = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &NoNetwork,
            MissingPolicy::Error,
            &p,
        )
        .unwrap();
        assert_eq!(names(&again), names(&out));
    }

    #[test]
    fn a_second_run_uses_the_cache_and_never_touches_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        let first = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        let again = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &NoNetwork,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!(names(&first), names(&again));
        let used = again.artifact.unwrap();
        assert_eq!((used.origin, used.fetched.len()), (Origin::Cache, 0));
    }

    #[test]
    fn the_platform_system_app_is_fetched_from_the_modern_dev_folder_of_the_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let mut p = project();
        p.dependencies = vec![dep(SA, "System Application"), dep(SYS, "System")];
        let out = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &p,
        )
        .unwrap();
        assert_eq!(names(&out), ["System", "System Application"]);
        let used = out.artifact.unwrap();
        assert_eq!(used.fetched, [SYSTEM_APP_ENTRY]);
        // Warm: the second run finds it in the cache and asks nobody.
        let again = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, Some("27.5")),
            &NoNetwork,
            MissingPolicy::Error,
            &p,
        )
        .unwrap();
        assert_eq!(names(&again), ["System", "System Application"]);
    }

    /// An extracted artifact as BcContainerHelper keeps it: `<sandbox>/<build>/platform/<entry>`.
    fn extracted_artifact(sandbox: &Path, build: &str) {
        let platform = sandbox.join(build).join("platform");
        let put = |entry: &str, bytes: &[u8]| {
            let p = platform.join(entry);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let any = source_zip(
            &app_json(ANY, "Any", &[]),
            &[("Any.Codeunit.al", "codeunit 130500 Any { }")],
        );
        put(
            "Applications/TestFramework/TestLibraries/Any/Source/Any.Source.zip",
            &any,
        );
        let mock = source_zip(
            &app_json(MOCK, "Permissions Mock", &[]),
            &[("M.Codeunit.al", "codeunit 131006 \"Permissions Mock\" { }")],
        );
        put(
            "Applications/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip",
            &mock,
        );
        let satl = source_zip(
            &app_json(
                SATL,
                "System Application Test Library",
                &[(SA, "System Application"), (ANY, "Any")],
            ),
            &[("P.Codeunit.al", "codeunit 132916 \"Azure AD Plan Test Library\" { }")],
        );
        put(
            "Applications/System Application/Test/System Application Test Library.Source.zip",
            &satl,
        );
        put(SYSTEM_APP_ENTRY, &system_app());
        // Filler a walk must not need to read.
        put("ServiceTier/big.dll", &[0u8; 100]);
    }

    #[test]
    fn an_exact_build_that_the_feed_no_longer_lists_is_taken_from_the_local_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        extracted_artifact(&sandbox, "27.5.46862.48827");
        // The feed lists only newer builds.
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249", "28.1.1.1"]);
        let alpkg = alpackages(dir.path());
        let mut f = fetch(&dir.path().join("cache"), true, Some("27.5.46862.48827"));
        f.local_artifacts = Some(sandbox);
        let out = run(&alpkg, &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(
            names(&out),
            [
                "System",
                "System Application",
                "Any",
                "System Application Test Library",
                "Permissions Mock"
            ]
        );
        let used = out.artifact.unwrap();
        assert_eq!(used.build.to_string(), "27.5.46862.48827");
        assert_eq!(used.origin, Origin::Local);
        assert_eq!(cdn.index_calls.get(), 0, "an exact local build needs no feed");
        assert_eq!(cdn.platform_calls.get(), 0);
        // The source of an app read from there fills Microsoft's build variables with that build.
        assert!(
            out.log
                .iter()
                .any(|l| l.contains("27.5.46862.48827") && l.contains("local")),
            "{:?}",
            out.log
        );
    }

    #[test]
    fn local_artifacts_are_the_last_resort_for_a_prefix_the_feed_cannot_answer() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        extracted_artifact(&sandbox, "27.5.46862.48827");
        extracted_artifact(&sandbox, "28.1.49838.50621");
        let alpkg = alpackages(dir.path());
        // Offline: the local build of the major is used.
        let mut f = fetch(&dir.path().join("cache"), false, Some("27"));
        f.local_artifacts = Some(sandbox.clone());
        let out = run(&alpkg, &f, &NoNetwork, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(out.artifact.unwrap().build.to_string(), "27.5.46862.48827");
        // Online and the feed has a match: the feed's newest wins over the local one.
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let mut f = fetch(&dir.path().join("cache2"), true, Some("27"));
        f.local_artifacts = Some(sandbox);
        let out = run(&alpkg, &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        let used = out.artifact.unwrap();
        assert_eq!(
            (used.build.to_string().as_str(), used.origin),
            ("27.5.46862.56249", Origin::Published)
        );
    }

    #[test]
    fn the_build_of_the_tests_is_taken_from_the_local_artifacts_before_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        extracted_artifact(&sandbox, "27.5.46862.48827");
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let mut f = fetch(&dir.path().join("cache"), true, None);
        f.query = f.query.with_preferred(BuildId::parse("27.5.46862.48827").ok());
        f.local_artifacts = Some(sandbox);
        let out = run(&alpackages(dir.path()), &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        let used = out.artifact.unwrap();
        assert_eq!(
            (used.build.to_string().as_str(), used.origin),
            ("27.5.46862.48827", Origin::Local)
        );
        assert_eq!(cdn.index_calls.get(), 0);
    }

    #[test]
    fn the_build_of_the_tests_is_taken_from_the_feed_when_it_is_still_listed() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.48827", "27.5.46862.56249"]);
        let mut f = fetch(&dir.path().join("cache"), true, None);
        f.query = f.query.with_preferred(BuildId::parse("27.5.46862.48827").ok());
        let out = run(&alpackages(dir.path()), &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(out.artifact.unwrap().build.to_string(), "27.5.46862.48827");
    }

    #[test]
    fn a_build_of_the_tests_that_is_gone_is_replaced_by_the_newest_and_the_log_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let mut f = fetch(&dir.path().join("cache"), true, None);
        f.query = f.query.with_preferred(BuildId::parse("27.5.46862.48827").ok());
        let out = run(&alpackages(dir.path()), &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(out.artifact.unwrap().build.to_string(), "27.5.46862.56249");
        assert!(
            out.log
                .iter()
                .any(|l| l.contains("27.5.46862.48827") && l.contains("56249")),
            "{:?}",
            out.log
        );
    }

    #[test]
    fn the_build_of_the_tests_is_taken_from_the_local_artifacts_even_when_another_build_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        extracted_artifact(&sandbox, "27.5.46862.48827");
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        // A first run without the tests' build caches the newest published one.
        let cache = dir.path().join("cache");
        let first = run(
            &alpkg,
            &fetch(&cache, true, None),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!(first.artifact.unwrap().build.to_string(), "27.5.46862.56249");
        let mut f = fetch(&cache, true, None);
        f.query = f.query.with_preferred(BuildId::parse("27.5.46862.48827").ok());
        f.local_artifacts = Some(sandbox);
        let out = run(&alpkg, &f, &NoNetwork, MissingPolicy::Error, &project()).unwrap();
        let used = out.artifact.unwrap();
        assert_eq!(
            (used.build.to_string().as_str(), used.origin),
            ("27.5.46862.48827", Origin::Local)
        );
    }

    #[test]
    fn offline_a_cached_build_is_used_when_the_tests_build_is_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        run(
            &alpkg,
            &fetch(&cache, true, None),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        let mut f = fetch(&cache, false, None);
        f.query = f.query.with_preferred(BuildId::parse("27.5.46862.48827").ok());
        let out = run(&alpkg, &f, &NoNetwork, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(out.artifact.unwrap().build.to_string(), "27.5.46862.56249");
    }

    #[test]
    fn a_build_that_is_nowhere_still_names_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        extracted_artifact(&sandbox, "27.5.46862.48827");
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let mut f = fetch(&dir.path().join("cache"), true, Some("27.5.1.1"));
        f.local_artifacts = Some(sandbox);
        let e = run(&alpackages(dir.path()), &f, &cdn, MissingPolicy::Error, &project()).unwrap_err();
        assert!(e.contains("27.5.1.1"), "{e}");
    }

    #[test]
    fn a_cached_build_wins_over_a_newer_published_one() {
        let dir = tempfile::tempdir().unwrap();
        let old = FixtureCdn::new(dir.path(), &["27.5.46862.55978"]);
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &old,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        // A newer 27.5 is published now; the next run stays on the cached build and does not even ask.
        let p = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &NoNetwork,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!(p.artifact.unwrap().build.to_string(), "27.5.46862.55978");
    }

    #[test]
    fn nothing_missing_means_no_build_is_chosen_and_no_request_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        let mut only_sa = project();
        only_sa.dependencies.truncate(1);
        let p = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, None),
            &NoNetwork,
            MissingPolicy::Error,
            &only_sa,
        )
        .unwrap();
        assert_eq!(names(&p), ["System Application"]);
        assert!(p.artifact.is_none());
        assert!(!dir.path().join("cache").exists(), "nothing was written either");
    }

    #[test]
    fn the_default_build_is_the_newest_of_the_floors_major() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249", "28.1.1.1"]);
        let alpkg = alpackages(dir.path());
        let f = Fetch {
            cache_root: dir.path().join("cache"),
            download: true,
            query: BuildQuery::new(None, 27, Some((27, 0))),
            local_artifacts: None,
        };
        let p = run(&alpkg, &f, &cdn, MissingPolicy::Error, &project()).unwrap();
        assert_eq!(p.artifact.unwrap().build.to_string(), "27.5.46862.56249");
        // A floor above everything published is an error naming the request, not a quiet downgrade.
        let f = Fetch {
            cache_root: dir.path().join("cache2"),
            download: true,
            query: BuildQuery::new(None, 27, Some((27, 9))),
            local_artifacts: None,
        };
        let e = run(&alpkg, &f, &cdn, MissingPolicy::Error, &project()).unwrap_err();
        assert!(e.contains("floor 27.9") && e.contains("27.5.46862.56249"), "{e}");
    }

    #[test]
    fn no_download_names_the_missing_apps_and_the_error_policy_fails_on_them() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        let f = fetch(&dir.path().join("cache"), false, Some("27.5"));
        let skip = run(&alpkg, &f, &NoNetwork, MissingPolicy::Skip, &project()).unwrap();
        assert_eq!(names(&skip), ["System Application"]);
        assert_eq!(skip.loaded.skipped.len(), 2);
        let e = run(&alpkg, &f, &NoNetwork, MissingPolicy::Error, &project()).unwrap_err();
        assert!(
            e.contains("System Application Test Library") && e.contains("--no-download"),
            "{e}"
        );
    }

    #[test]
    fn an_app_by_another_publisher_is_never_looked_for_in_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        let mut p = project();
        p.dependencies = vec![DepDecl {
            publisher: "Contoso".into(),
            ..dep("c0", "Contoso Lib")
        }];
        // The feed is not even asked for its index: nothing in it could be Contoso's.
        let out = run(
            &alpkg,
            &fetch(&dir.path().join("c"), true, None),
            &NoNetwork,
            MissingPolicy::Skip,
            &p,
        )
        .unwrap();
        assert_eq!(out.loaded.skipped.len(), 1);
        assert!(out.artifact.is_none());
    }

    #[test]
    fn an_app_the_feed_lacks_stays_missing_and_is_reported_with_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let mut p = project();
        p.dependencies.push(dep("aa", "No Such Microsoft App"));
        let out = run(
            &alpkg,
            &fetch(&dir.path().join("c"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Skip,
            &p,
        )
        .unwrap();
        assert_eq!(out.loaded.skipped.len(), 1);
        assert!(
            out.log
                .iter()
                .any(|l| l.contains("No Such Microsoft App") && l.contains("27.5.46862.56249")),
            "{:?}",
            out.log
        );
        let e = run(
            &alpkg,
            &fetch(&dir.path().join("c"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &p,
        )
        .unwrap_err();
        assert!(e.contains("No Such Microsoft App"), "{e}");
    }

    #[test]
    fn a_crc_mismatch_in_a_downloaded_entry_is_an_error_and_nothing_bad_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        std::fs::write(&cdn.platform["27.5.46862.56249"], artifact_bytes(true, false)).unwrap();
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        let e = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap_err();
        assert!(e.contains("Permissions Mock") && e.contains("CRC-32"), "{e}");
        assert!(
            e.contains("https://cdn.test/27.5.46862.56249/platform"),
            "names the URL: {e}"
        );
        assert!(
            !cache
                .join("artifacts/27.5.46862.56249/apps/Permissions Mock.Source.zip")
                .exists()
        );
    }

    #[test]
    fn an_entry_whose_app_id_is_not_the_one_asked_for_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        std::fs::write(&cdn.platform["27.5.46862.56249"], artifact_bytes(false, true)).unwrap();
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        let out = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Skip,
            &project(),
        )
        .unwrap();
        // Any is not the app that satisfies the dependency of the test library, so it stays missing.
        assert!(
            out.loaded.skipped.iter().any(|e| e.to_string().contains("`Any`")),
            "{:?}",
            out.loaded.skipped
        );
        assert!(!cache.join("artifacts/27.5.46862.56249/apps/Any.Source.zip").exists());
    }

    #[test]
    fn a_damaged_cache_entry_is_dropped_and_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        let victim = cache.join("artifacts/27.5.46862.56249/apps/Any.Source.zip");
        std::fs::write(&victim, b"garbage").unwrap();
        let p = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert!(
            p.log
                .iter()
                .any(|l| l.contains("Any.Source.zip") && l.contains("removed")),
            "{:?}",
            p.log
        );
        assert_eq!(
            p.artifact.unwrap().fetched,
            ["Applications/TestFramework/TestLibraries/Any/Any.Source.zip"]
        );
        assert_ne!(std::fs::read(&victim).unwrap(), b"garbage");
    }

    #[test]
    fn the_central_directory_is_fetched_once_per_build() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.56249"]);
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!((cdn.index_calls.get(), cdn.platform_calls.get()), (1, 1));
        // A different project on the same build needs another app: the saved directory is reused
        // and the index is not asked for again.
        std::fs::remove_file(cache.join("artifacts/27.5.46862.56249/apps/Any.Source.zip")).unwrap();
        std::fs::remove_file(cache.join("artifacts/27.5.46862.56249/apps/Any.Source.zip.json")).unwrap();
        run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!(cdn.index_calls.get(), 1);
    }

    #[test]
    fn newest_published_ignores_the_cache_and_asks_only_for_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let cdn = FixtureCdn::new(dir.path(), &["27.5.46862.55978", "27.5.46862.56249", "28.1.1.1"]);
        let s = newest_published(&cdn, &BuildQuery::new(Some("27.5"), 27, None)).unwrap();
        assert_eq!(s.build.to_string(), "27.5.46862.56249");
        assert_eq!(cdn.platform_calls.get(), 0);
    }

    #[test]
    fn entries_are_found_by_app_name_in_any_folder() {
        use crate::zip::{find_end_record, parse_central_directory};
        let bytes = artifact_bytes(false, false);
        let end = find_end_record(&bytes).unwrap();
        let entries = parse_central_directory(&bytes[end.cd_offset..end.at], end.count).unwrap();
        let names = |d: &DepDecl| {
            find_entries(&entries, d)
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&dep(MOCK, "Permissions Mock")),
            ["Applications/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip"]
        );
        assert_eq!(names(&dep(ANY, "any")).len(), 1, "names compare without case");
        assert!(names(&dep(ANY, "Any Other")).is_empty());
        let other = DepDecl {
            publisher: "Contoso".into(),
            ..dep(ANY, "Any")
        };
        assert!(names(&other).is_empty());
    }

    /// A loopback HTTP server over the fixture artifact, with `Range` support. It stands in for
    /// Microsoft's CDN so that the HTTP code is tested without a network.
    mod loopback {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        pub struct Server {
            pub base: String,
            pub served: Arc<AtomicU64>,
            pub requests: Arc<AtomicU64>,
            stop: Arc<AtomicBool>,
            addr: std::net::SocketAddr,
        }

        impl Drop for Server {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::SeqCst);
                let _ = TcpStream::connect(self.addr);
            }
        }

        #[derive(Clone, Copy, PartialEq)]
        pub enum Mode {
            Normal,
            /// Answers every request with 200 and the whole file, as a server without ranges would.
            IgnoreRange,
            /// The index lists the build but its artifact has been pruned.
            Gone,
        }

        pub fn serve(build: &str, artifact: Vec<u8>, mode: Mode) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let served = Arc::new(AtomicU64::new(0));
            let requests = Arc::new(AtomicU64::new(0));
            let (stop2, served2, requests2) = (stop.clone(), served.clone(), requests.clone());
            let platform_path = format!("/{build}/platform");
            let index = format!(r#"[{{"Version":"{build}","CreationTime":"2026-10-01T00:00:00Z"}}]"#).into_bytes();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    requests2.fetch_add(1, Ordering::SeqCst);
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let mut range = None;
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        if h.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                            range = Some(v.trim().to_string());
                        }
                    }
                    let (status, extra, body): (&str, String, Vec<u8>) = if path == "/indexes/w1.json" {
                        ("200 OK", String::new(), index.clone())
                    } else if path == platform_path && mode != Mode::Gone {
                        let n = artifact.len();
                        match (range, mode) {
                            (Some(r), Mode::Normal) => {
                                let (a, b) = r.split_once('-').unwrap();
                                let (start, end) = if a.is_empty() {
                                    (n - b.parse::<usize>().unwrap().min(n), n - 1)
                                } else {
                                    (a.parse().unwrap(), b.parse::<usize>().unwrap().min(n - 1))
                                };
                                (
                                    "206 Partial Content",
                                    format!("Content-Range: bytes {start}-{end}/{n}\r\n"),
                                    artifact[start..=end].to_vec(),
                                )
                            }
                            _ => ("200 OK", String::new(), artifact.clone()),
                        }
                    } else {
                        ("404 Not Found", String::new(), b"<Error>BlobNotFound</Error>".to_vec())
                    };
                    served2.fetch_add(body.len() as u64, Ordering::SeqCst);
                    let head = format!(
                        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                    let mut sink = Vec::new();
                    let _ = reader.get_mut().take(0).read_to_end(&mut sink);
                }
            });
            Server {
                base: format!("http://{addr}"),
                served,
                requests,
                stop,
                addr,
            }
        }
    }

    #[test]
    fn over_http_only_ranges_are_requested_and_the_second_run_needs_no_server() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        let cache = dir.path().join("cache");
        let artifact = artifact_bytes(false, false);
        let total = artifact.len() as u64;
        let build = "27.5.46862.56249";
        {
            let server = loopback::serve(build, artifact, loopback::Mode::Normal);
            let cdn = HttpCdn::new(&server.base);
            let p = run(
                &alpkg,
                &fetch(&cache, true, Some("27.5")),
                &cdn,
                MissingPolicy::Error,
                &project(),
            )
            .unwrap();
            assert_eq!(p.artifact.unwrap().fetched.len(), 4);
            let served = server.served.load(std::sync::atomic::Ordering::SeqCst);
            // The index, the directory, four entries: not the whole artifact.
            assert!(served < total, "served {served} of {total}");
            // index + tail + (directory is inside the tail: tiny fixture) + 4 entries
            assert_eq!(server.requests.load(std::sync::atomic::Ordering::SeqCst), 1 + 1 + 4);
        } // the server is gone
        let cdn = HttpCdn::new("http://127.0.0.1:9");
        let again = run(
            &alpkg,
            &fetch(&cache, true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap();
        assert_eq!(again.artifact.unwrap().origin, Origin::Cache);
    }

    #[test]
    fn a_server_that_ignores_range_is_refused_not_downloaded_whole() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        let mut big = artifact_bytes(false, false);
        // Bigger than the 64 KiB end search, so a 200 answer cannot pass as a short file.
        let pad = vec![0u8; 200_000];
        big.splice(0..0, pad);
        let server = loopback::serve("27.5.46862.56249", big, loopback::Mode::IgnoreRange);
        let cdn = HttpCdn::new(&server.base);
        let e = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap_err();
        assert!(e.contains("range"), "{e}");
    }

    #[test]
    fn a_build_the_feed_no_longer_has_is_a_404_naming_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let alpkg = alpackages(dir.path());
        // The index lists the build but the artifact is gone: the feed prunes old builds.
        let server = loopback::serve("27.5.46862.56249", Vec::new(), loopback::Mode::Gone);
        let cdn = HttpCdn::new(&server.base);
        let e = run(
            &alpkg,
            &fetch(&dir.path().join("cache"), true, Some("27.5")),
            &cdn,
            MissingPolicy::Error,
            &project(),
        )
        .unwrap_err();
        assert!(e.contains("404") && e.contains("/27.5.46862.56249/platform"), "{e}");
    }
}
