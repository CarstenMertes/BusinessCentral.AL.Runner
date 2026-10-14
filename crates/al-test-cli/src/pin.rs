//! The per-version corpus pin check (ADR-007, "Decided without owner review (2026-10-13/14)").
//!
//! The `corpus` submodule is pinned to one commit. An oracle normally comes from the CI run of
//! that commit. A version whose oracle was recorded for an older commit X is still sound when
//! the corpus diff X..pinned only adds files under `tests/`: no existing test changed, so the
//! oracle's results still describe the same tests. Cases that exist only in the newer corpus are
//! not in that oracle and do not count for that version.

use std::path::Path;
use std::process::Command;

/// Accepts `recorded` as the commit of an oracle when the corpus pin is `pinned`.
pub fn check(repo: &Path, recorded: &str, pinned: &str) -> Result<(), String> {
    if recorded == pinned {
        return Ok(());
    }
    let short = |c: &str| c.chars().take(12).collect::<String>();
    // `--no-renames`: a rename is a deletion plus an addition, and the deletion is a change.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--name-status",
            "--no-renames",
            recorded,
            pinned,
            "--",
            "tests/",
            // The corpus's compile log: not a test, rewritten by every compile.
            ":(exclude,glob)tests/**/.dev/**",
        ])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot compare the corpus commits {} and {} ({}): the oracle is recorded for the older one, so its history is needed; run `git -C {} fetch --unshallow`",
            short(recorded),
            short(pinned),
            String::from_utf8_lossy(&out.stderr).trim(),
            repo.display()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let changed: Vec<&str> = text.lines().filter(|l| !l.starts_with('A')).collect();
    if changed.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "the oracle is recorded for corpus commit {}, but the pin {} changes or removes existing test files, so the oracle no longer describes them; record the oracle again from the CI run of the pin:",
        short(recorded),
        short(pinned)
    );
    for l in changed.iter().take(10) {
        msg.push_str(&format!("\n  {}", l.replace('\t', " ")));
    }
    if changed.len() > 10 {
        msg.push_str(&format!("\n  ... and {} more", changed.len() - 10));
    }
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn put(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A repo with one commit holding `tests/a.al` and `workflow.yml`; returns the commit.
    fn repo() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]);
        put(d.path(), "tests/a.al", "one");
        put(d.path(), "workflow.yml", "one");
        git(d.path(), &["add", "."]);
        git(d.path(), &["commit", "-q", "-m", "first"]);
        let c = git(d.path(), &["rev-parse", "HEAD"]);
        (d, c)
    }

    fn commit(d: &Path) -> String {
        git(d, &["add", "-A"]);
        git(d, &["commit", "-q", "-m", "next"]);
        git(d, &["rev-parse", "HEAD"])
    }

    #[test]
    fn the_same_commit_is_accepted() {
        let (d, c) = repo();
        assert_eq!(check(d.path(), &c, &c), Ok(()));
    }

    #[test]
    fn added_test_files_are_accepted() {
        let (d, old) = repo();
        put(d.path(), "tests/b.al", "new");
        let new = commit(d.path());
        assert_eq!(check(d.path(), &old, &new), Ok(()));
    }

    #[test]
    fn changes_outside_tests_are_ignored() {
        let (d, old) = repo();
        put(d.path(), "workflow.yml", "two");
        let new = commit(d.path());
        assert_eq!(check(d.path(), &old, &new), Ok(()));
    }

    /// `tests/al-language/.dev/compile-errors.log` is the corpus's own compile log. It is not a test
    /// and it changes with every compile, so it never makes an older oracle stale.
    #[test]
    fn changes_to_the_dev_log_directory_are_ignored() {
        let (d, old) = repo();
        put(d.path(), "tests/app/.dev/compile-errors.log", "one");
        let mid = commit(d.path());
        put(d.path(), "tests/app/.dev/compile-errors.log", "two");
        let new = commit(d.path());
        assert_eq!(check(d.path(), &mid, &new), Ok(()));
        assert_eq!(check(d.path(), &old, &new), Ok(()));
    }

    #[test]
    fn a_modified_test_file_is_refused_and_named() {
        let (d, old) = repo();
        put(d.path(), "tests/b.al", "new");
        put(d.path(), "tests/a.al", "changed");
        let new = commit(d.path());
        let e = check(d.path(), &old, &new).unwrap_err();
        assert!(e.contains("tests/a.al"), "{e}");
        assert!(!e.contains("tests/b.al"), "an added file is not a change: {e}");
        assert!(e.contains(&old[..12]) && e.contains(&new[..12]), "{e}");
    }

    #[test]
    fn a_deleted_test_file_is_refused() {
        let (d, old) = repo();
        std::fs::remove_file(d.path().join("tests/a.al")).unwrap();
        let new = commit(d.path());
        assert!(check(d.path(), &old, &new).unwrap_err().contains("tests/a.al"));
    }

    #[test]
    fn a_renamed_test_file_is_refused() {
        let (d, old) = repo();
        std::fs::rename(d.path().join("tests/a.al"), d.path().join("tests/z.al")).unwrap();
        let new = commit(d.path());
        assert!(check(d.path(), &old, &new).is_err());
    }

    #[test]
    fn an_unknown_commit_says_how_to_get_the_history() {
        let (d, c) = repo();
        let e = check(d.path(), "0123456789012345678901234567890123456789", &c).unwrap_err();
        assert!(e.contains("fetch"), "{e}");
    }
}
