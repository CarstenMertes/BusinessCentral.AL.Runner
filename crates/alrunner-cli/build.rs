//! Puts the commit into `alrunner --version`: `ALRUNNER_COMMIT` is the short hash, or "unknown"
//! when the source has no git history (a source archive). CI may set `ALRUNNER_COMMIT` itself.
//! `ALRUNNER_VERSION` is the Cargo version plus the optional `ALRUNNER_VERSION_SUFFIX` (an edge
//! build sets `+edge.YYYYMMDD`). A suffix that is not empty and does not start with `+` is ignored.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ALRUNNER_COMMIT");
    println!("cargo:rerun-if-env-changed=ALRUNNER_VERSION_SUFFIX");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    let commit = std::env::var("ALRUNNER_COMMIT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().chars().take(9).collect::<String>())
        .or_else(|| {
            let out = Command::new("git")
                .args(["rev-parse", "--short=9", "HEAD"])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ALRUNNER_COMMIT={commit}");
    let suffix = std::env::var("ALRUNNER_VERSION_SUFFIX").unwrap_or_default();
    let suffix = suffix.trim();
    let suffix = if suffix.starts_with('+') && suffix.len() > 1 {
        suffix
    } else {
        ""
    };
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    println!("cargo:rustc-env=ALRUNNER_VERSION={version}{suffix}");
}
