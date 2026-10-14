//! The command line's own contract (port of the CLI half of RequestSurfaceTests.cs): an
//! option a command does not accept fails the command, naming it, and a mistyped option is
//! never a plausible wrong answer with exit 0. Hermetic: runs the built binary on the
//! typeprobe fixtures.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../al-bcdb/tests/fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Out {
    let o = Command::new(env!("CARGO_BIN_EXE_al-bcdb")).args(args).output().unwrap();
    Out {
        code: o.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn refused(args: &[&str]) -> String {
    let o = run(args);
    assert_eq!(o.code, 1, "{args:?} should fail: {}", o.stdout);
    assert!(o.stdout.is_empty(), "a refused command prints nothing on stdout");
    o.stderr
}

#[test]
fn unknown_flag_is_refused_and_the_accepted_ones_are_named() {
    let bak = fixture("typeprobe.bak");
    let e = refused(&["read", &bak, "--table", "probe", "--totallyBogusFlag"]);
    for needle in ["totallyBogusFlag", "--table", "--merge-extensions"] {
        assert!(e.contains(needle), "{e}");
    }
}

#[test]
fn misspelled_flags_are_refused_rather_than_dropped() {
    let bak = fixture("typeprobe.bak");
    for (flag, value) in [
        ("compayn", Some("CRONUS")),
        ("tpo", Some("2")),
        ("mergeExtensions", None),
    ] {
        let mut a = vec!["read", bak.as_str(), "--table", "probe"];
        let f = format!("--{flag}");
        a.push(&f);
        if let Some(v) = value {
            a.push(v);
        }
        assert!(refused(&a).contains(flag));
    }
}

#[test]
fn a_flag_is_only_accepted_by_the_commands_that_use_it() {
    let bak = fixture("typeprobe.bak");
    assert!(refused(&["read", &bak, "--table", "probe", "--fixture", "f.tsv"]).contains("fixture"));
    assert!(refused(&["read", &bak, "--table", "probe", "--against", "x.mdf"]).contains("against"));
    assert!(refused(&["tables", &bak, "--select", "id"]).contains("select"));
    // and verify accepts its own flag plus the read options it passes through
    let fix = fixture("typeprobe-probe-dense.tsv");
    let ok = run(&[
        "verify",
        &bak,
        "--fixture",
        &fix,
        "--table",
        "probe_dense",
        "--select",
        "id,grp,amount,posted,note",
    ]);
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert!(
        ok.stdout
            .starts_with("OK: 4000 rows match fixture typeprobe-probe-dense.tsv"),
        "{}",
        ok.stdout
    );
}

#[test]
fn a_value_taking_flag_without_a_value_is_refused() {
    let bak = fixture("typeprobe.bak");
    // at the end of the line, and immediately followed by another flag
    assert!(refused(&["read", &bak, "--table", "probe", "--company"]).contains("--company"));
    assert!(refused(&["read", &bak, "--company", "--top", "2"]).contains("--company"));
}

#[test]
fn valueless_flags_take_no_value_and_positional_arguments_are_refused() {
    let bak = fixture("typeprobe.bak");
    let ok = run(&[
        "read",
        &bak,
        "--merge-extensions",
        "--table",
        "probe",
        "--select",
        "id",
        "--top",
        "1",
    ]);
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert!(refused(&["read", &bak, "probe"]).contains("probe"));
}

#[test]
fn format_refuses_a_value_it_does_not_understand() {
    let bak = fixture("typeprobe.bak");
    let e = refused(&["read", &bak, "--table", "probe", "--format", "yaml"]);
    assert!(e.contains("yaml") && e.contains("json"), "{e}");
}

#[test]
fn good_command_succeeds_and_the_same_with_an_unknown_flag_fails() {
    let bak = fixture("typeprobe.bak");
    let good = ["read", bak.as_str(), "--table", "probe", "--select", "id", "--top", "1"];
    assert_eq!(run(&good).code, 0);
    let mut bad = good.to_vec();
    bad.push("--totallyBogusFlag");
    assert_eq!(run(&bad).code, 1);
}

#[test]
fn exit_codes_follow_bcdb() {
    assert_eq!(run(&["--version"]).code, 0);
    assert_eq!(run(&["bogus", "x"]).code, 64);
    assert_eq!(run(&["tables"]).code, 64);
    assert_eq!(run(&["tables", "/no/such/file"]).code, 2);
    // check and validate are page-map commands: a bacpac has none
    assert_eq!(run(&["check", &fixture("typeprobe.bacpac")]).code, 2);
}

#[test]
fn restore_is_not_ported_and_says_why() {
    let e = refused(&["restore", &fixture("typeprobe.bak"), "--to", "x"]);
    assert!(e.contains("restore is not ported"), "{e}");
}

#[test]
fn read_json_and_tsv_shapes() {
    let bak = fixture("typeprobe.bak");
    let t = run(&[
        "read",
        &bak,
        "--table",
        "probe_notnull",
        "--select",
        "id,n_bit,n_nvarchar",
        "--format",
        "tsv",
    ]);
    let mut lines = t.stdout.lines();
    assert_eq!(lines.next(), Some("id|n_bit|n_nvarchar"));
    assert_eq!(lines.count(), 3);
    let j = run(&[
        "read",
        &bak,
        "--table",
        "probe_notnull",
        "--select",
        "id,n_bit",
        "--format",
        "json",
    ]);
    assert!(j.stdout.starts_with("[\n  {\"id\": "), "{}", j.stdout);
    assert!(j.stdout.ends_with("}\n]\n"), "{}", j.stdout);
    assert!(j.stdout.contains("\"n_bit\": true") || j.stdout.contains("\"n_bit\": false"));
}

#[test]
fn export_reads_every_table_with_rows() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().to_str().unwrap();
    for f in ["typeprobe.bak", "typeprobe.bacpac"] {
        let o = run(&["export", &fixture(f), "--out", out, "--jobs", "2"]);
        assert_eq!(o.code, 0, "{}", o.stderr);
        assert!(o.stderr.contains("0 failed"), "{}", o.stderr);
        let dense = std::fs::read_to_string(dir.path().join("probe_dense.tsv")).unwrap();
        assert_eq!(dense.lines().count(), 4001, "header plus 4000 rows from {f}");
    }
}
