//! `release/install.sh` against a local folder that looks like a release (ALRUNNER_DOWNLOAD_BASE). The binary
//! is a fake shell script; nothing is downloaded. Linux x64 and macOS arm64 only, as the script.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

fn script(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A release folder with one archive `<exe>-<label>-<target>.tar.xz` holding a fake `<exe>`, and SHA256SUMS.
fn release(dir: &Path, exe: &str, label: &str, target: &str) {
    let stage = dir.join("stage");
    script(&stage.join(exe), &format!("echo '{exe} 3.0.0-alpha.9 (abc)'"));
    let archive = format!("{exe}-{label}-{target}.tar.xz");
    let ok = Command::new("tar")
        .arg("-cJf")
        .arg(dir.join(&archive))
        .arg("-C")
        .arg(&stage)
        .arg(exe)
        .status()
        .unwrap();
    assert!(ok.success());
    let sum = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd '{}' && (sha256sum '{archive}' 2>/dev/null || shasum -a 256 '{archive}') > SHA256SUMS",
            dir.display()
        ))
        .status()
        .unwrap();
    assert!(sum.success());
}

fn install(base: &Path, bin: &Path, home: &Path, path_first: &[PathBuf], tag: &str) -> Output {
    let install_sh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/install.sh");
    let mut path: Vec<PathBuf> = path_first.to_vec();
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    Command::new("sh")
        .arg(install_sh)
        .arg(tag)
        .env("ALRUNNER_DOWNLOAD_BASE", format!("file://{}", base.display()))
        .env("ALRUNNER_INSTALL_DIR", bin)
        .env("HOME", home)
        .env("PATH", std::env::join_paths(path).unwrap())
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn install_sh_installs_al_runner_and_points_out_v2_and_an_old_alrunner() {
    let Some(target) = target() else { return };
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("rel");
    release(&base, "al-runner", "edge", target);
    let bin = d.path().join("bin");
    let home = d.path().join("home");
    let tools = home.join(".dotnet").join("tools");
    script(&tools.join("al-runner"), "echo 'al-runner v2.12.0'");
    // An install from before the rename.
    script(&bin.join("alrunner"), "echo 'alrunner 3.0.0-alpha.2 (x)'");
    // v2's folder comes first on PATH, then the install folder.
    let o = install(&base, &bin, &home, &[tools.clone(), bin.clone()], "edge");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(bin.join("al-runner").is_file(), "{t}");
    assert!(t.contains("al-runner 3.0.0-alpha.9 (abc)"), "prints the version: {t}");
    assert!(
        t.contains(&format!("{}/alrunner", bin.display())) && t.contains("old name"),
        "{t}"
    );
    assert!(
        t.contains(&format!("AL Runner v2 is installed at {}/al-runner", tools.display())),
        "{t}"
    );
    assert!(t.contains("dotnet tool uninstall -g MSDyn365BC.AL.Runner"), "{t}");
    assert!(
        t.contains(&format!("put {} before {} on PATH", bin.display(), tools.display())),
        "{t}"
    );
}

#[test]
fn install_sh_without_v2_or_an_old_binary_prints_no_note() {
    let Some(target) = target() else { return };
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("rel");
    release(&base, "al-runner", "edge", target);
    let bin = d.path().join("bin");
    let home = d.path().join("home");
    let o = install(&base, &bin, &home, std::slice::from_ref(&bin), "edge");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(!t.contains("note:"), "{t}");
}

#[test]
fn install_sh_still_installs_a_release_from_before_the_rename() {
    let Some(target) = target() else { return };
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("rel");
    release(&base, "alrunner", "3.0.0-alpha.2", target);
    let bin = d.path().join("bin");
    let home = d.path().join("home");
    let o = install(&base, &bin, &home, std::slice::from_ref(&bin), "v3.0.0-alpha.2");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(bin.join("alrunner").is_file(), "an old release keeps its old name: {t}");
    assert!(!bin.join("al-runner").exists(), "{t}");
}
