//! The host the command line gives a session (ADR-006): the real wall clock, and AL Runner's
//! identity. The core crates read both through `al-host`; only this crate touches the OS.

use al_host::{AlRunnerIdentity, Clock, FileSystem, Host};
use std::path::{Component, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The system clock, read on every call.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc_millis(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            // A clock set before 1970: the same distance on the other side.
            Err(e) => -(e.duration().as_millis() as i64),
        }
    }
}

/// The directory `--files-dir` named, if any. Set once at start-up.
static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Makes the `File` data type use a real directory instead of memory (`--files-dir DIR`).
pub fn set_files_dir(dir: PathBuf) {
    let _ = FILES_DIR.set(dir);
}

/// The real clock and AL Runner's identity, on the file system the command line chose: memory,
/// unless `--files-dir` named a directory.
pub fn system_host() -> Host {
    let host = Host::new(SystemClock, AlRunnerIdentity);
    match FILES_DIR.get() {
        Some(dir) => host.with_files(DirFileSystem::new(dir.clone())),
        None => host,
    }
}

/// Files in one directory. AL paths are taken as relative to it, with `/` or `\` as separators;
/// a path that is absolute or climbs out with `..` names no file (the run cannot touch anything
/// outside the directory the user opted into).
pub struct DirFileSystem {
    root: PathBuf,
}

impl DirFileSystem {
    pub fn new(root: PathBuf) -> DirFileSystem {
        DirFileSystem { root }
    }

    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let rel = PathBuf::from(path.replace('\\', "/"));
        let mut out = self.root.clone();
        for c in rel.components() {
            match c {
                Component::Normal(p) => out.push(p),
                Component::CurDir => {}
                _ => return None,
            }
        }
        (out != self.root).then_some(out)
    }
}

impl FileSystem for DirFileSystem {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.resolve(path)?).ok()
    }
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let p = self
            .resolve(path)
            .ok_or_else(|| format!("`{path}` is outside the files directory"))?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, bytes).map_err(|e| e.to_string())
    }
    fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|p| p.is_file())
    }
    fn remove(&mut self, path: &str) -> bool {
        self.resolve(path).is_some_and(|p| std::fs::remove_file(p).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_file_system_keeps_files_under_its_root_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut fs = DirFileSystem::new(dir.path().to_path_buf());
        fs.write("a/b.txt", b"hi").unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/b.txt")).unwrap(), b"hi");
        assert_eq!(fs.read("a\\b.txt").unwrap(), b"hi");
        assert!(fs.exists("a/b.txt") && !fs.exists("a"));
        assert!(fs.write("../escape.txt", b"x").is_err());
        assert!(fs.write("/abs.txt", b"x").is_err());
        assert!(!fs.exists("../escape.txt"));
        assert!(fs.remove("a/b.txt") && !fs.remove("a/b.txt"));
    }

    #[test]
    fn the_system_clock_is_after_2024_and_does_not_run_backwards() {
        let c = SystemClock;
        let (a, b) = (c.now_utc_millis(), c.now_utc_millis());
        assert!(a > 1_704_067_200_000, "{a}");
        assert!(b >= a);
    }
}
