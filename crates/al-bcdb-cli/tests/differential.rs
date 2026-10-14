//! Differential tests: the published bcdb (0.2.1, native AOT) and this port on the same
//! input must print the same bytes and exit with the same code.
//!
//! These need the reference binary, so they are `#[ignore]`d (Rust's way to report a test
//! as skipped rather than passed):
//!
//! ```text
//! cargo test -p al-bcdb-cli --test differential -- --ignored --test-threads 2
//! ```
//!
//! The reference is `$BCDB_REFERENCE`, else `bcdb` on the PATH, else the mise install. The
//! real-file tests read BC demo backups from the artifact cache (`~/.bcartifacts.cache`);
//! they fail loudly when a file is absent. A real CRONUS `.bacpac` does not ship in any
//! artifact: set `BCDB_BACPAC` to one to include it (it is read, never copied).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn reference() -> PathBuf {
    if let Ok(p) = std::env::var("BCDB_REFERENCE") {
        return p.into();
    }
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let p = Path::new(dir).join("bcdb");
        if p.is_file() {
            return p;
        }
    }
    let home = std::env::var("HOME").unwrap();
    let p = PathBuf::from(home).join(".local/share/mise/installs/dotnet-bcdb/latest/bin/bcdb");
    assert!(p.is_file(), "no reference bcdb: set BCDB_REFERENCE");
    p
}

fn port() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_al-bcdb"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../al-bcdb/tests/fixtures")
        .join(name)
}

struct Ran {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

fn run(bin: &Path, args: &[&str]) -> Ran {
    let o = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}", bin.display()));
    Ran {
        code: o.status.code(),
        stdout: o.stdout,
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// Same stdout bytes and exit code; and, for failures, the same message.
fn same(args: &[&str]) {
    let (a, b) = (run(&reference(), args), run(&port(), args));
    assert_eq!(
        a.code, b.code,
        "exit code of {args:?}\nbcdb: {}\nport: {}",
        a.stderr, b.stderr
    );
    if a.stdout != b.stdout {
        let (x, y) = (String::from_utf8_lossy(&a.stdout), String::from_utf8_lossy(&b.stdout));
        let first = x.lines().zip(y.lines()).position(|(p, q)| p != q).unwrap_or(0);
        panic!(
            "stdout of {args:?} differs at line {first}\nbcdb: {:?}\nport: {:?}",
            x.lines().nth(first),
            y.lines().nth(first)
        );
    }
    assert_eq!(a.stderr, b.stderr, "stderr of {args:?}");
}

#[test]
#[ignore = "needs the published bcdb binary"]
fn fixtures_read_every_table_in_both_formats() {
    let syms = format!(
        "{},{}",
        fixture("symbols-exttest-base.json").display(),
        fixture("symbols-exttest-ext.json").display()
    );
    for f in ["typeprobe.bak", "typeprobe.bacpac"] {
        let file = fixture(f);
        let file = file.to_str().unwrap();
        same(&["companies", file]);
        same(&["tables", file]);
        same(&["tables", file, "--symbols", &syms]);
        // every table of the file, by SQL name, all columns
        let listing = run(&reference(), &["tables", file]);
        for line in listing.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            let line = String::from_utf8_lossy(line);
            let (head, name) = line.split_once('\t').unwrap();
            let company = head.split_whitespace().last().unwrap();
            let mut base = vec!["read", file, "--table", name];
            if company != "-" {
                base.extend(["--company", company]);
            }
            for fmt in ["tsv", "json"] {
                let mut a = base.clone();
                a.extend(["--format", fmt, "--symbols", &syms]);
                same(&a);
                let mut b = base.clone();
                b.extend(["--format", fmt]);
                same(&b);
            }
        }
    }
}

#[test]
#[ignore = "needs the published bcdb binary"]
fn fixtures_options_and_refusals() {
    let syms = format!(
        "{},{}",
        fixture("symbols-exttest-base.json").display(),
        fixture("symbols-exttest-ext.json").display()
    );
    for f in ["typeprobe.bak", "typeprobe.bacpac"] {
        let file = fixture(f);
        let file = file.to_str().unwrap();
        for args in [
            vec!["read", file, "--table", "probe", "--top", "3"],
            vec!["read", file, "--table", "probe", "--top", "0"],
            vec!["read", file, "--table", "probe", "--top", "abc"],
            vec!["read", file, "--table", "probe", "--top", "-1"],
            vec![
                "read",
                file,
                "--table",
                "probe",
                "--select",
                "id,c_nvarchar",
                "--format",
                "json",
            ],
            vec!["read", file, "--table", "probe_oddnames", "--select", "id,pad "],
            vec!["read", file, "--table", "probe_oddnames", "--select", "id, pad"],
            vec!["read", file, "--table", "probe", "--select", "c_nosuch"],
            vec![
                "read",
                file,
                "--table",
                "probe_lob",
                "--select",
                "id,c_vbmax",
                "--sha256",
                "c_vbmax",
            ],
            vec![
                "read",
                file,
                "--table",
                "probe_lob",
                "--select",
                "id",
                "--sha256",
                "c_vbmax",
            ],
            vec![
                "read",
                file,
                "--table",
                "probe_lob",
                "--select",
                "id,c_vbmax",
                "--sha256",
                "nope",
            ],
            vec!["read", file, "--table", "probe", "--sha256", "id"],
            vec!["read", file, "--table", "ambig"],
            vec!["read", file, "--table", "ambig", "--app", "2222"],
            vec!["read", file, "--table", "ambig", "--app", "9999"],
            vec!["read", file, "--table", "no_such_table"],
            vec!["read", file, "--table", "probe", "--compayn", "x"],
            vec!["read", file, "--table", "probe", "--format", "yaml"],
            vec!["read", file, "--table", "probe", "--company"],
            vec!["read", file, "probe"],
            vec!["read", file, "--table", "exttest", "--merge-extensions"],
            vec![
                "read",
                file,
                "--table",
                "exttest",
                "--merge-extensions",
                "--symbols",
                &syms,
            ],
            vec![
                "read",
                file,
                "--table",
                "exttest2",
                "--merge-extensions",
                "--symbols",
                &syms,
            ],
            vec!["read", file, "--table", "exttest3", "--merge-extensions"],
            vec!["read", file, "--table", "exttest4", "--symbols", &syms],
            vec![
                "read",
                file,
                "--table",
                "exttest4",
                "--merge-extensions",
                "--symbols",
                &syms,
            ],
            vec!["read", file, "--table", "probe", "--merge-extensions", "--select", "id"],
            vec!["describe", file, "--table", "exttest", "--symbols", &syms],
            vec!["describe", file, "--table", "exttest4", "--symbols", &syms],
            vec!["describe", file, "--table", "probe", "--symbols", &syms],
            vec!["describe", file, "--table", "exttest"],
        ] {
            same(&args);
        }
    }
    same(&["read", "/no/such/file", "--table", "x"]);
    // the usage text names the program, so only the exit code and the empty stdout are held equal
    let (a, b) = (run(&reference(), &["bogus", "x"]), run(&port(), &["bogus", "x"]));
    assert_eq!((a.code, a.stdout.len()), (b.code, b.stdout.len()));
}

#[test]
#[ignore = "needs the published bcdb binary"]
fn fixtures_verify_command() {
    for f in ["typeprobe.bak", "typeprobe.bacpac"] {
        let file = fixture(f);
        let fix = fixture("typeprobe-probe-dense.tsv");
        same(&[
            "verify",
            file.to_str().unwrap(),
            "--fixture",
            fix.to_str().unwrap(),
            "--table",
            "probe_dense",
            "--select",
            "id,grp,amount,posted,note",
        ]);
        same(&[
            "verify",
            file.to_str().unwrap(),
            "--fixture",
            fix.to_str().unwrap(),
            "--table",
            "probe_dense",
            "--select",
            "id,grp",
        ]);
    }
}

#[test]
#[ignore = "needs the published bcdb binary"]
fn fixtures_check_command() {
    let file = fixture("typeprobe.bak");
    same(&["check", file.to_str().unwrap()]);
    same(&["check", fixture("typeprobe.bacpac").to_str().unwrap()]);
}

/// Every request answered by both servers, byte for byte.
fn serve_all(file: &str, symbols: Option<&str>, show_content: bool) {
    let tables = run(&port(), &["tables", file]);
    let mut reqs = String::new();
    let mut n = 0;
    for line in String::from_utf8_lossy(&tables.stdout).lines() {
        let Some((head, name)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = head.split_whitespace();
        let rows: i64 = parts.next().unwrap().parse().unwrap();
        let company = parts.last().unwrap();
        if rows == 0 {
            continue;
        }
        let name = name.split('\t').next().unwrap();
        let esc = |s: &str| serde_json::to_string(s).unwrap();
        if company == "-" {
            reqs.push_str(&format!(
                "{{\"id\": {n}, \"cmd\": \"read\", \"table\": {}}}\n",
                esc(name)
            ));
        } else {
            reqs.push_str(&format!(
                "{{\"id\": {n}, \"cmd\": \"read\", \"table\": {}, \"company\": {}}}\n",
                esc(name),
                esc(company)
            ));
        }
        n += 1;
    }
    reqs.push_str("{\"cmd\": \"tables\"}\n{\"cmd\": \"companies\"}\n");
    let dir = tempfile::tempdir().unwrap();
    let req_path = dir.path().join("requests.jsonl");
    std::fs::write(&req_path, reqs).unwrap();
    let mut outs = Vec::new();
    for (name, bin) in [("bcdb", reference()), ("port", port())] {
        let out = dir.path().join(format!("{name}.out"));
        let mut cmd = Command::new(&bin);
        cmd.args(["serve", file]);
        if let Some(s) = symbols {
            cmd.args(["--symbols", s]);
        }
        let st = cmd
            .stdin(File::open(&req_path).unwrap())
            .stdout(File::create(&out).unwrap())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "{name} serve failed");
        outs.push(out);
    }
    let (mut a, mut b) = (
        BufReader::new(File::open(&outs[0]).unwrap()),
        BufReader::new(File::open(&outs[1]).unwrap()),
    );
    let (mut la, mut lb) = (Vec::new(), Vec::new());
    let mut line_no = 0;
    loop {
        la.clear();
        lb.clear();
        let (x, y) = (
            a.read_until(b'\n', &mut la).unwrap(),
            b.read_until(b'\n', &mut lb).unwrap(),
        );
        if x == 0 && y == 0 {
            break;
        }
        line_no += 1;
        if la != lb {
            // a customer's export must not end up in a log: say where and how long, not what
            let head = |v: &[u8]| {
                if show_content {
                    String::from_utf8_lossy(&v[..v.len().min(300)]).into_owned()
                } else {
                    format!("<{} bytes>", v.len())
                }
            };
            // the reply to a table listing may differ only in ordering; report the first difference
            panic!(
                "serve reply {line_no} of {file} differs\nbcdb: {}\nport: {}",
                head(&la),
                head(&lb)
            );
        }
    }
    assert!(line_no as usize >= n, "only {line_no} replies for {n} requests");
}

fn artifact(version: &str, name: &str) -> String {
    let home = std::env::var("HOME").unwrap();
    for sub in ["w1", "us"] {
        let p = format!("{home}/.bcartifacts.cache/sandbox/{version}/{sub}/{name}");
        if Path::new(&p).is_file() {
            return p;
        }
    }
    panic!("artifact {version}/{name} is not in ~/.bcartifacts.cache");
}

#[test]
#[ignore = "needs the published bcdb binary and the BC 28.1 artifact in ~/.bcartifacts.cache"]
fn bc281_tables_and_companies_are_identical() {
    let f = artifact("28.1.49838.50621", "BusinessCentral-W1.bak");
    same(&["tables", &f]);
    same(&["companies", &f]);
    same(&["check", &f]);
}

#[test]
#[ignore = "needs the published bcdb binary and the BC 28.1 artifact in ~/.bcartifacts.cache"]
fn bc281_every_table_through_serve_is_identical() {
    serve_all(&artifact("28.1.49838.50621", "BusinessCentral-W1.bak"), None, true);
}

#[test]
#[ignore = "needs the published bcdb binary and the BC 28.1 artifact in ~/.bcartifacts.cache"]
fn bc281_describe_and_ar_names_with_the_shipped_base_application() {
    let bak = artifact("28.1.49838.50621", "BusinessCentral-W1.bak");
    let app = artifact(
        "28.1.49838.50621",
        "Extensions/Microsoft_Base Application_28.1.49838.50621.app",
    );
    for table in [
        "Customer",
        "No. Series",
        "G/L Entry",
        "Dimension Set Entry",
        "Posted Gen. Journal Line",
        "Sales Header",
        "Item",
    ] {
        for company in ["CRONUS", "My Company"] {
            same(&[
                "describe",
                &bak,
                "--table",
                table,
                "--company",
                company,
                "--symbols",
                &app,
            ]);
            same(&[
                "read",
                &bak,
                "--table",
                table,
                "--company",
                company,
                "--symbols",
                &app,
                "--merge-extensions",
                "--top",
                "50",
                "--format",
                "json",
            ]);
        }
    }
    same(&["tables", &bak, "--symbols", &app]);
}

#[test]
#[ignore = "needs the published bcdb binary and the BC 27.5, 29.0 and 24.5 (US) artifacts in ~/.bcartifacts.cache"]
fn other_versions_tables_and_a_few_tables_are_identical() {
    for (v, n) in [
        ("27.5.46862.48827", "BusinessCentral-W1.bak"),
        ("29.0.54011.55816", "BusinessCentral-W1.bak"),
        ("24.5.23489.42600", "BusinessCentral-US.bak"),
    ] {
        let f = artifact(v, n);
        same(&["tables", &f]);
        same(&["companies", &f]);
        same(&[
            "read",
            &f,
            "--table",
            "Customer",
            "--company",
            "CRONUS",
            "--format",
            "json",
        ]);
        same(&["read", &f, "--table", "G/L Entry", "--company", "CRONUS"]);
    }
}

#[test]
#[ignore = "needs the published bcdb binary and the BC 27.5, 29.0 and 24.5 (US) artifacts in ~/.bcartifacts.cache"]
fn other_versions_every_table_through_serve_is_identical() {
    for (v, n) in [
        ("27.5.46862.48827", "BusinessCentral-W1.bak"),
        ("29.0.54011.55816", "BusinessCentral-W1.bak"),
        ("24.5.23489.42600", "BusinessCentral-US.bak"),
    ] {
        serve_all(&artifact(v, n), None, true);
    }
}

#[test]
#[ignore = "needs the published bcdb binary and a real .bacpac in $BCDB_BACPAC"]
fn real_bacpac_every_table_through_serve_is_identical() {
    let f = std::env::var("BCDB_BACPAC").expect("set BCDB_BACPAC to a real .bacpac");
    same(&["tables", &f]);
    same(&["companies", &f]);
    serve_all(&f, None, false);
}
