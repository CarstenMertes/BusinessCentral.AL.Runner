//! The backup provider on bcdb's synthetic `typeprobe` database (no Microsoft data): the same
//! tables as a `.bak` and as a `.bacpac`.

use al_bcdb_std::BackupProvider;
use al_host::testdata::{Cell, TableError, TableProvider, TableRequest};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../al-bcdb/tests/fixtures")
        .join(name)
}

fn request(name: &str, app_id: Option<&str>, per_company: bool) -> TableRequest {
    TableRequest {
        name: name.into(),
        app_id: app_id.map(str::to_string),
        per_company,
    }
}

const FILES: [&str; 2] = ["typeprobe.bak", "typeprobe.bacpac"];

#[test]
fn the_company_is_picked_by_name_and_a_wrong_name_lists_the_companies() {
    for f in FILES {
        let p = BackupProvider::open(&fixture(f), Some("tp")).unwrap();
        assert_eq!(p.company(), "TP", "{f}");
        assert_eq!(p.source(), f);
        let err = BackupProvider::open(&fixture(f), Some("Nope")).err().unwrap();
        assert!(err.contains("'ProbeCo'") && err.contains("'TP'"), "{err}");
        let err = BackupProvider::open(&fixture(f), None).err().unwrap();
        assert!(
            err.contains("2 companies") && err.contains("--test-data-company"),
            "{err}"
        );
    }
}

#[test]
fn a_table_extension_companion_is_merged_into_the_base_rows() {
    for f in FILES {
        let p = BackupProvider::open(&fixture(f), Some("TP")).unwrap();
        let t = p.read(&request("exttest", None, true)).unwrap();
        let ext = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        assert_eq!(
            t.columns,
            ["id", "own", &format!("extra${ext}"), &format!("num${ext}")],
            "{f}"
        );
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[0][1], Cell::Text("base-one".into()));
        assert_eq!(t.rows[0][2], Cell::Text("ext-one".into()));
        assert_eq!(t.rows[0][3], Cell::Int(11));
        assert_eq!(
            t.rows[2][2],
            Cell::Null,
            "a base row without a companion row reads NULL"
        );
    }
}

#[test]
fn a_table_without_a_company_is_read_only_when_the_table_is_not_per_company() {
    for f in FILES {
        let p = BackupProvider::open(&fixture(f), Some("TP")).unwrap();
        let t = p.read(&request("probe", None, false)).unwrap();
        assert_eq!(t.rows.len(), 8, "{f}");
        let col = |n: &str| t.columns.iter().position(|c| c == n).unwrap();
        assert_eq!(t.rows[1][col("c_int")], Cell::Int(2147483647));
        assert_eq!(t.rows[1][col("c_bit")], Cell::Bool(true));
        assert_eq!(t.rows[5][col("c_int")], Cell::Null);
        assert!(
            matches!(
                t.rows[0][col("c_dec38_20")],
                Cell::Decimal {
                    negative: false,
                    magnitude: 0,
                    scale: 20
                }
            ),
            "{f}"
        );
        assert!(matches!(t.rows[3][col("c_guid")], Cell::Guid(_)));
        assert_eq!(p.read(&request("probe", None, true)).err(), Some(TableError::Absent));
    }
}

#[test]
fn two_tables_of_one_name_are_told_apart_by_the_declaring_apps_id() {
    for f in FILES {
        let p = BackupProvider::open(&fixture(f), Some("ProbeCo")).unwrap();
        let one = p
            .read(&request("ambig", Some("11111111-1111-1111-1111-111111111111"), true))
            .unwrap();
        let two = p
            .read(&request("ambig", Some("22222222-2222-2222-2222-222222222222"), true))
            .unwrap();
        assert_eq!((one.rows.len(), two.rows.len()), (1, 1), "{f}");
        assert_ne!(one.rows, two.rows);
        for app in [None, Some("33333333-3333-3333-3333-333333333333")] {
            match p.read(&request("ambig", app, true)).err() {
                Some(TableError::Ambiguous(why)) => assert!(why.contains("2 tables"), "{why}"),
                other => panic!("{f} {app:?}: {other:?}"),
            }
        }
    }
}

#[test]
fn a_table_the_reader_cannot_join_is_a_reader_error_with_its_text_and_other_tables_still_read() {
    for f in FILES {
        let p = BackupProvider::open(&fixture(f), Some("TP")).unwrap();
        match p.read(&request("exttest3", None, true)).err() {
            Some(TableError::Reader(why)) => assert!(why.contains("stranger"), "{f}: {why}"),
            other => panic!("{f}: {other:?}"),
        }
        assert!(p.read(&request("exttest", None, true)).is_ok());
        assert_eq!(
            p.read(&request("No Such Table", None, true)).err(),
            Some(TableError::Absent)
        );
    }
}

/// A cache with the given `<version>/<country>/BusinessCentral-<COUNTRY>.bak` files.
fn cache(files: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for f in files {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }
    d
}

#[test]
fn the_cached_artifact_is_the_apps_build_else_the_newest_of_the_version() {
    use al_bcdb_std::default_backup_in;
    let c = cache(&[
        "27.5.46862.48827/w1/BusinessCentral-W1.bak",
        "27.5.46862.48827/platform/x.txt",
        "28.1.49838.50621/w1/BusinessCentral-W1.bak",
        "29.0.54011.55816/w1/BusinessCentral-W1.bak",
        "24.5.23489.42600/us/BusinessCentral-US.bak",
    ]);
    let pick = |builds: &[&str], major, minor| {
        let builds: Vec<String> = builds.iter().map(|b| b.to_string()).collect();
        default_backup_in(c.path(), &builds, major, minor)
            .map(|p| p.strip_prefix(c.path()).unwrap().to_string_lossy().into_owned())
    };
    // the build of the loaded apps wins over the target's version
    assert_eq!(
        pick(&["1.0.0.0", "27.5.46862.48827"], 29, Some(0)).unwrap(),
        "27.5.46862.48827/w1/BusinessCentral-W1.bak"
    );
    // else major.minor, else the major
    assert_eq!(
        pick(&[], 28, Some(1)).unwrap(),
        "28.1.49838.50621/w1/BusinessCentral-W1.bak"
    );
    assert_eq!(
        pick(&[], 27, Some(0)).unwrap(),
        "27.5.46862.48827/w1/BusinessCentral-W1.bak"
    );
    assert_eq!(
        pick(&[], 24, None).unwrap(),
        "24.5.23489.42600/us/BusinessCentral-US.bak"
    );
    let err = pick(&["x"], 26, None).unwrap_err();
    assert!(
        err.contains("no cached BC artifact for BC 26") && err.contains("--test-data PATH.bak"),
        "{err}"
    );
}
