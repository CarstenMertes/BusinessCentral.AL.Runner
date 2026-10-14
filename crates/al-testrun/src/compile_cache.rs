//! The compile cache of the dependency apps (ADR-022): which key a run has, where the entry file
//! lives, how it is read, built and written. The format is `al-cache`; this is the OS side.

use al_cache::{AppKey, Entry, Key};
use al_sema::workspace::AppSource;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// How a run uses the compile cache.
#[derive(Debug, Clone)]
pub struct CompileCache {
    /// The folder of the entry files: `<cache root>/compile/`.
    pub dir: PathBuf,
    /// A miss is an error (CI: the measured run must be a hit).
    pub require: bool,
}

impl CompileCache {
    /// The folder under the cache root the download cache already uses.
    pub fn under(root: &Path) -> PathBuf {
        root.join("compile")
    }
}

/// What the cache did for a run (`--timings`, the job summary).
#[derive(Debug, Clone, Default)]
pub struct CacheReport {
    /// The entry file.
    pub file: PathBuf,
    /// The entry was found and used as it was.
    pub hit: bool,
    /// The entry was built by this run (a miss, or a file that was refused).
    pub built: bool,
    /// Making the key and reading and opening the file.
    pub open: Duration,
    /// Building and writing the entry (zero on a hit).
    pub build: Duration,
    pub bytes: usize,
    /// Dependency objects lowered again because the workspace changes what they embed.
    pub relowered: usize,
    /// The dependency objects in the entry.
    pub objects: usize,
    /// Why a file was not used, or why the cache was not used at all.
    pub notes: Vec<String>,
    /// Dependency files that did not parse when the entry was built: (path, why).
    pub parse_problems: Vec<(String, String)>,
    /// With lazily read dependency apps: where the scan of their sources came from, `read` (the index
    /// in the cache folder) or `built` (scanned now, index saved).
    pub source_index: Option<&'static str>,
}

/// A hash of the running executable: it changes with every change of the compiler or of the
/// builtins catalog. It cannot miss a crate, which a hash of source files can.
pub fn runtime_build() -> [u8; 32] {
    static HASH: OnceLock<[u8; 32]> = OnceLock::new();
    *HASH.get_or_init(|| {
        let bytes = std::env::current_exe().and_then(std::fs::read);
        match bytes {
            Ok(b) => Sha256::digest(&b).into(),
            // Without the executable the key cannot say which compiler made an entry: a hash of
            // the time would only ever miss. Use a value no file can match.
            Err(e) => Sha256::digest(format!("no executable: {e}").as_bytes()).into(),
        }
    })
}

/// A hash of the source of an app: every path and every byte.
pub fn source_hash(app: &AppSource) -> [u8; 32] {
    let per_file: Vec<[u8; 32]> = app
        .files
        .par_iter()
        .map(|(path, text)| {
            let mut h = Sha256::new();
            h.update((path.len() as u64).to_le_bytes());
            h.update(path.as_bytes());
            h.update((text.len() as u64).to_le_bytes());
            h.update(text.as_bytes());
            h.finalize().into()
        })
        .collect();
    let mut h = Sha256::new();
    h.update((per_file.len() as u64).to_le_bytes());
    for f in &per_file {
        h.update(f);
    }
    h.finalize().into()
}

/// The target as the key holds it: every field, written out.
fn target_text(t: &al_version::Target) -> String {
    // Destructured without `..`: a field added to the target must be written here to compile.
    let al_version::Target { bc, runtime } = t;
    let al_version::BcVersion { major, minor, build } = bc;
    let al_version::Runtime {
        major: rt_major,
        minor: rt_minor,
    } = runtime;
    format!(
        "bc {major}.{minor} build {} runtime {rt_major}.{rt_minor}",
        build.as_deref().unwrap_or("-")
    )
}

/// The key of the entry for the dependency `apps` (program order), their manifests, the
/// preprocessor symbols and the target.
pub fn key_for(
    apps: &[&AppSource],
    manifests: &[crate::runtime::Manifest],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
) -> Key {
    key_with(apps, manifests, symbols, target, &[])
}

/// [`key_for`] where `identities[i]`, when set, stands for the source of app `i`: the hash of the zip
/// directory of a package read lazily, which needs no read of the files (and is the hash the app has
/// whether or not its files were read since). An app without one is hashed over every path and byte.
pub fn key_with(
    apps: &[&AppSource],
    manifests: &[crate::runtime::Manifest],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    identities: &[Option<[u8; 32]>],
) -> Key {
    let apps = apps
        .iter()
        .zip(manifests)
        .enumerate()
        .map(|(i, (a, m))| AppKey {
            id: a.id.clone(),
            name: a.name.clone(),
            dependencies: a.dependencies.clone(),
            publisher: m.publisher.clone(),
            version: m.version.clone(),
            source: identities.get(i).copied().flatten().unwrap_or_else(|| source_hash(a)),
        })
        .collect();
    Key::new(
        runtime_build(),
        // `AL_UNPINNED_BASE_ACCESS` changes what the lowering produces (ADR-023), so it is an input.
        format!(
            "{}{}",
            target.map(target_text).unwrap_or_default(),
            std::env::var("AL_UNPINNED_BASE_ACCESS").map_or(String::new(), |v| format!(" base-access {v}"))
        ),
        symbols.iter().cloned().collect(),
        apps,
    )
}

/// Reads the entry for `key` from `dir`. `Ok(None)`: there is no file. A file that cannot be used
/// (damaged, truncated, made for other inputs) is removed and reported in `Err`; the caller builds
/// a new one. Nothing in a refused file is used.
pub fn read_entry(dir: &Path, key: &Key) -> Result<Option<(Entry, PathBuf)>, String> {
    let path = dir.join(key.file_name());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match Entry::open(bytes, key) {
        Ok(e) => Ok(Some((e, path))),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(format!("{} ignored and removed: {e}", path.display()))
        }
    }
}

/// Writes `bytes` as the entry for `key`: to a temporary file in the same folder, then renamed, so
/// a reader sees the whole old file or the whole new one.
pub fn write_entry(dir: &Path, key: &Key, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(key.file_name());
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        key.file_name(),
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    Ok(path)
}

/// Builds the entry for `apps` (dependency apps in program order) and encodes it.
pub fn build_entry(
    apps: Vec<AppSource>,
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    key: &Key,
) -> Vec<u8> {
    let set = al_sema::lower::lower_for_cache(apps, symbols, target);
    al_cache::build(&set, key)
}

/// How long an entry that nothing used stays in the folder.
pub const KEEP_DAYS: u64 = 30;

/// Marks the entry as used now, so that [`prune`] keeps what runs still use. Best effort.
pub fn touch(path: &Path) {
    let _ = std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(std::time::SystemTime::now()));
}

/// Removes the entries in `dir`, except `keep`, that nothing used for `KEEP_DAYS` days: an app
/// that changes with every run would otherwise leave 13 MB behind each time. Best effort.
pub fn prune(dir: &Path, keep: &Path) -> Vec<PathBuf> {
    let limit = std::time::Duration::from_secs(KEEP_DAYS * 24 * 3600);
    let mut removed = Vec::new();
    let indexes = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "alci"));
    for path in list(dir).into_iter().map(|(p, _)| p).chain(indexes) {
        if path == keep {
            continue;
        }
        let old = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        if old && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// The entry files in `dir` and their sizes.
pub fn list(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, u64)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "alcc"))
        .map(|e| (e.path(), e.metadata().map(|m| m.len()).unwrap_or(0)))
        .collect();
    out.sort();
    out
}

/// The entry for the dependency `apps`: the file in the cache folder, or one built now and saved.
/// A file that cannot be used is reported in the notes and replaced. With `cc.require` a missing
/// or unusable file is an error.
pub fn open_or_build(
    cc: &CompileCache,
    apps: &[AppSource],
    manifests: &[crate::runtime::Manifest],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
) -> Result<(Entry, CacheReport), String> {
    let mut apps = apps.to_vec();
    open_or_build_with(cc, &mut apps, manifests, symbols, target, &[], &mut |_| {})
}

/// [`open_or_build`] with the identities of [`key_with`].
/// `prepare` is called with the apps just before an entry is built, which needs their text: a caller
/// that left it unread reads it then. A hit never calls it.
pub fn open_or_build_with(
    cc: &CompileCache,
    apps: &mut [AppSource],
    manifests: &[crate::runtime::Manifest],
    symbols: &std::collections::BTreeSet<String>,
    target: Option<&al_version::Target>,
    identities: &[Option<[u8; 32]>],
    prepare: &mut dyn FnMut(&mut [AppSource]),
) -> Result<(Entry, CacheReport), String> {
    let opened = std::time::Instant::now();
    let key = {
        let dep_refs: Vec<&AppSource> = apps.iter().collect();
        key_with(&dep_refs, manifests, symbols, target, identities)
    };
    let mut report = CacheReport {
        file: cc.dir.join(key.file_name()),
        ..Default::default()
    };
    let entry = match read_entry(&cc.dir, &key) {
        Ok(Some((e, _))) => {
            report.hit = true;
            Some(e)
        }
        Ok(None) => None,
        Err(note) => {
            eprintln!("compile cache: {note}");
            report.notes.push(note);
            None
        }
    };
    report.open = opened.elapsed();
    if let Some(e) = entry {
        report.bytes = e.size();
        touch(&report.file);
        return Ok((e, report));
    }
    if cc.require {
        return Err(format!(
            "--require-compile-cache: there is no usable entry {}{}",
            report.file.display(),
            report.notes.iter().map(|n| format!(" ({n})")).collect::<String>()
        ));
    }
    eprintln!(
        "compile cache: building the entry for {} dependency apps once ({})",
        apps.len(),
        report.file.display()
    );
    prepare(apps);
    let started = std::time::Instant::now();
    let bytes = build_entry(apps.to_vec(), symbols, target, &key);
    report.bytes = bytes.len();
    if let Err(e) = write_entry(&cc.dir, &key, &bytes) {
        let note = format!("the entry was not saved: {e}");
        eprintln!("compile cache: {note}");
        report.notes.push(note);
    }
    report.built = true;
    prune(&cc.dir, &report.file);
    let entry = Entry::open(bytes, &key).map_err(|e| format!("a fresh compile cache entry does not open: {e}"))?;
    report.build = started.elapsed();
    Ok((entry, report))
}

/// The index entry of the dependency apps: what the scan of their sources found (`al_cache::index`).
/// Its key is the key of the apps without a target: the scan depends on the sources, the preprocessor
/// symbols and the compiler, not on the BC version.
pub fn index_key(
    apps: &[&AppSource],
    manifests: &[crate::runtime::Manifest],
    symbols: &std::collections::BTreeSet<String>,
    identities: &[Option<[u8; 32]>],
) -> Key {
    key_with(apps, manifests, symbols, None, identities)
}

/// Reads the index for `key` from `dir`: `None` when there is no file or it cannot be used (damaged,
/// cut, made for other inputs; such a file is removed). The run scans the sources instead.
pub fn read_index(dir: &Path, key: &Key) -> Option<Vec<Vec<al_sema::FileIndex>>> {
    let path = dir.join(al_cache::index_file_name(key));
    let bytes = std::fs::read(&path).ok()?;
    match al_cache::open_index(&bytes, key) {
        Ok(index) => {
            touch(&path);
            Some(index)
        }
        Err(e) => {
            eprintln!("compile cache: {} ignored and removed: {e}", path.display());
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Writes the index for `key` (a temporary file, then a rename). Best effort: a run that cannot
/// save it is not worse off than before.
pub fn write_index(dir: &Path, key: &Key, index: &[Vec<al_sema::FileIndex>]) {
    let bytes = al_cache::build_index(key, index);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(al_cache::index_file_name(key));
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        al_cache::index_file_name(key),
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    if std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    } else {
        prune(dir, &path);
    }
}
