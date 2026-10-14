//! Serve mode: one file opened once, many requests answered over a line-based JSON
//! protocol. Hermetic, driven in-process against the typeprobe fixtures; values are the
//! oracle-known data the fixture tests use. Port of ServeTests.cs and RequestSurfaceTests.cs.

mod common;
use al_bcdb::open;
use al_bcdb::serve::serve;
use al_bcdb::symbols::SymbolStore;
use common::*;
use serde_json::Value as J;
use std::io::Cursor;

fn exttest_symbols() -> SymbolStore {
    SymbolStore::load([
        ("symbols-exttest-base.json", fixture_bytes("symbols-exttest-base.json")),
        ("symbols-exttest-ext.json", fixture_bytes("symbols-exttest-ext.json")),
    ])
    .unwrap()
}

fn run_on(fixture: &str, sym: Option<&SymbolStore>, requests: &[&str]) -> Vec<J> {
    let src = open(Box::new(fixture_bytes(fixture))).unwrap();
    let mut out = Vec::new();
    serve(&*src, sym, &mut Cursor::new(requests.join("\n").into_bytes()), &mut out).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not json ({e}): {l}")))
        .collect()
}

fn run(requests: &[&str]) -> Vec<J> {
    run_on("typeprobe.bak", None, requests)
}

fn one(fixture: &str, req: &str) -> J {
    let mut r = run_on(fixture, None, &[req]);
    assert_eq!(r.len(), 1);
    r.remove(0)
}

fn ok(r: &J) -> bool {
    r["ok"].as_bool().unwrap()
}

fn error(r: &J) -> String {
    assert!(!ok(r), "expected a refusal: {r}");
    r["error"].as_str().unwrap().to_string()
}

fn headers(r: &J) -> Vec<String> {
    r["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap().to_string())
        .collect()
}

fn rows_by_first(r: &J) -> std::collections::HashMap<i64, Vec<J>> {
    r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x[0].as_i64().unwrap(), x.as_array().unwrap().clone()))
        .collect()
}

#[test]
fn read_returns_oracle_known_values() {
    let res = run(&[r#"{"id": 7, "cmd": "read", "table": "probe", "select": "id,c_bigint,c_nvarchar"}"#]);
    assert_eq!(res.len(), 1);
    let r = &res[0];
    assert_eq!(r["id"], 7);
    assert!(ok(r));
    assert_eq!(headers(r), ["id", "c_bigint", "c_nvarchar"]);
    let rows = rows_by_first(r);
    assert_eq!(rows[&2][1].as_i64(), Some(i64::MAX));
    assert_eq!(rows[&2][2], "Hello World");
    assert_eq!(rows[&3][1].as_i64(), Some(i64::MIN));
    assert_eq!(rows[&3][2], "Ærøskøbing über café");
    assert!(rows[&6][1].is_null());
}

#[test]
fn error_keeps_the_session_alive() {
    let res = run(&[
        r#"{"id": 1, "cmd": "read", "table": "no_such_table"}"#,
        r#"{"id": 2, "cmd": "read", "table": "probe_ghost", "select": "id,val", "top": 1}"#,
    ]);
    assert_eq!(res.len(), 2);
    assert!(error(&res[0]).contains("no_such_table"));
    assert!(ok(&res[1]));
    assert_eq!(res[1]["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn tables_and_companies_and_quit() {
    let res = run(&[
        r#"{"cmd": "tables"}"#,
        r#"{"cmd": "companies"}"#,
        r#"{"cmd": "quit"}"#,
        r#"{"cmd": "read", "table": "probe"}"#, // after quit: must not be answered
    ]);
    assert_eq!(res.len(), 3);
    let t = |name: &str| {
        res[0]["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(t("probe")["rows"], 8);
    assert_eq!(t("probe_ghost")["compression"], "page");
    assert!(t("probe")["company"].is_null());
    assert!(ok(&res[1]) && ok(&res[2]));
}

#[test]
fn dollar_prefixed_platform_table_keeps_its_raw_name() {
    // "$probe$platform".split('$') matches no <company>$<table>$<appid> shape; the listing
    // must fall back to the raw object name so platform tables ($ndo$... in real
    // databases) stay discoverable.
    let res = run(&[
        r#"{"cmd": "tables"}"#,
        r#"{"cmd": "read", "table": "$probe$platform", "select": "id,v"}"#,
    ]);
    let tables = res[0]["tables"].as_array().unwrap();
    let t = tables.iter().find(|t| t["name"] == "$probe$platform").expect("listed");
    assert_eq!(t["rows"], 2);
    assert!(!tables.iter().any(|t| t["name"] == ""));
    assert_eq!(res[1]["rows"][0][1], "platform-one");
}

#[test]
fn app_selector_disambiguates_same_named_tables() {
    // Two apps define "ambig" in the same company (legal via AL namespaces; the BC demo
    // database ships Dimension Set Entry twice).
    let res = run(&[
        r#"{"id": 1, "cmd": "read", "table": "ambig"}"#,
        r#"{"id": 2, "cmd": "read", "table": "ambig", "app": "2222", "select": "id,v"}"#,
    ]);
    assert!(error(&res[0]).contains("--app"));
    assert!(ok(&res[1]));
    assert_eq!(res[1]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(res[1]["rows"][0][1], "from-app-two");
}

#[test]
fn describe_lists_table_extension_fields() {
    let sym = exttest_symbols();
    let res = run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "describe", "table": "exttest"}"#],
    );
    let r = &res[0];
    assert!(ok(r), "{r}");
    let f = |name: &str| {
        r["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["sqlColumn"].is_string() && f["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(f("own")["id"], 2);
    assert_eq!(f("extra")["id"], 50120);
    assert_eq!(f("extra")["type"], "Text");
    assert_eq!(f("extra")["sqlColumn"], "extra$bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
    assert_eq!(f("num")["id"], 50121);
}

#[test]
fn merged_read_joins_extension_companion() {
    // One AL record = base row + $ext companion row joined on the key. Base row 3 has no
    // companion row: its extension fields are NULL.
    let sym = exttest_symbols();
    let res = run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "read", "table": "exttest", "merge-extensions": true}"#],
    );
    let r = &res[0];
    assert!(ok(r), "{r}");
    assert_eq!(headers(r), ["id", "own", "extra", "num"]);
    let rows = rows_by_first(r);
    assert_eq!(rows[&1][1], "base-one");
    assert_eq!(rows[&1][2], "ext-one");
    assert_eq!(rows[&1][3], 11);
    assert!(rows[&2][2].is_null());
    assert_eq!(rows[&2][3], 22);
    assert_eq!(rows[&3][1], "base-three");
    assert!(rows[&3][2].is_null() && rows[&3][3].is_null());
    assert!(r.get("note").is_none());
}

#[test]
fn read_maps_base_table_extension_columns_to_al_names() {
    // exttest4 is the BC 29 shape: extension fields are columns of the base table itself,
    // "<field>$<extending app id>", and there is no $ext companion.
    let sym = exttest_symbols();
    let r = &run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "read", "table": "exttest4"}"#],
    )[0];
    assert!(ok(r), "{r}");
    assert_eq!(headers(r), ["id", "own", "extra", "num"]);
    let rows = rows_by_first(r);
    assert_eq!(rows[&1][2], "ext-one");
    assert_eq!(rows[&1][3], 11);
    assert!(rows[&2][2].is_null());
    assert_eq!(rows[&2][3], 22);
    assert!(r.get("note").is_none());
    let sel = &run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "read", "table": "exttest4", "select": "id,extra"}"#],
    )[0];
    assert_eq!(headers(sel), ["id", "extra"]);
}

#[test]
fn describe_lists_base_table_extension_fields() {
    let sym = exttest_symbols();
    let r = &run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "describe", "table": "exttest4"}"#],
    )[0];
    assert!(ok(r), "{r}");
    let f = |name: &str| {
        r["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["sqlColumn"].is_string() && f["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(f("extra")["id"], 50122);
    assert_eq!(f("extra")["sqlColumn"], "extra$bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
    assert_eq!(f("num")["id"], 50123);
    assert_eq!(f("num")["extension"]["appName"], "ExtTest Ext");
}

#[test]
fn merge_extensions_without_a_companion_says_so_instead_of_silently_doing_nothing() {
    let sym = exttest_symbols();
    let r = &run_on(
        "typeprobe.bak",
        Some(&sym),
        &[r#"{"cmd": "read", "table": "exttest4", "merge-extensions": true}"#],
    )[0];
    assert!(ok(r), "{r}");
    let note = r["note"].as_str().unwrap();
    assert!(
        note.contains("no $ext companion") && note.contains("base table"),
        "{note}"
    );
    assert_eq!(headers(r), ["id", "own", "extra", "num"]);
    let plain = &run(&[r#"{"cmd": "read", "table": "probe", "select": "id", "merge-extensions": true}"#])[0];
    assert!(plain["note"].as_str().unwrap().contains("nothing to merge"));
}

fn assert_exttest2_merged(r: &J) {
    // exttest2 carries Posted Gen. Journal Line's shape (table 181): the base table's
    // primary key is (id) alone, but its CLUSTERED index is (tmpl, batch, id), and BC keys
    // the $ext companion on the primary key. A merged read joins on the companion's own key.
    assert!(ok(r), "{r}");
    assert_eq!(
        headers(r),
        [
            "tmpl",
            "id",
            "own",
            "batch",
            "extra$bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "num$bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        ]
    );
    let rows: std::collections::HashMap<i64, Vec<J>> = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x[1].as_i64().unwrap(), x.as_array().unwrap().clone()))
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[&1][0], "GENERAL");
    assert_eq!(rows[&1][2], "base-one");
    assert_eq!(rows[&1][3], "DEFAULT");
    assert_eq!(rows[&1][4], "ext-one");
    assert_eq!(rows[&1][5], 11);
    assert_eq!(rows[&2][3], "DAILY");
    assert!(rows[&2][4].is_null() && rows[&2][5].is_null()); // id 2 has no companion row
    assert_eq!(rows[&3][0], "SALES");
    assert!(rows[&3][4].is_null()); // id 3 has one, with a NULL text
    assert_eq!(rows[&3][5], 33);
}

#[test]
fn merged_read_joins_on_the_companion_key_not_the_base_clustered_key() {
    assert_exttest2_merged(&one(
        "typeprobe.bak",
        r#"{"cmd": "read", "table": "exttest2", "merge-extensions": true}"#,
    ));
}

#[test]
fn bacpac_merged_read_joins_on_the_same_companion_key() {
    assert_exttest2_merged(&one(
        "typeprobe.bacpac",
        r#"{"cmd": "read", "table": "exttest2", "merge-extensions": true}"#,
    ));
}

#[test]
fn merged_read_refuses_when_the_companion_key_is_not_in_the_base_table() {
    // exttest3's companion is keyed on "stranger", which its base table does not have.
    let e = error(&one(
        "typeprobe.bak",
        r#"{"cmd": "read", "table": "exttest3", "merge-extensions": true}"#,
    ));
    for needle in [
        "stranger",
        "TP$exttest3$aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa$ext",
        "refusing to guess",
    ] {
        assert!(e.contains(needle), "{e}");
    }
}

#[test]
fn malformed_request_reports_error_and_continues() {
    let res = run(&[
        "this is not json",
        r#"{"id": "x", "cmd": "read", "table": "probe", "select": "id", "top": 2}"#,
    ]);
    assert_eq!(res.len(), 2);
    assert!(!ok(&res[0]));
    assert!(ok(&res[1]));
    assert_eq!(res[1]["id"], "x");
    assert_eq!(res[1]["rows"].as_array().unwrap().len(), 2);
}

#[test]
fn bacpac_serve_answers_the_same_values() {
    let r = one(
        "typeprobe.bacpac",
        r#"{"id": 7, "cmd": "read", "table": "probe", "select": "id,c_bigint,c_nvarchar"}"#,
    );
    assert!(ok(&r));
    let rows = rows_by_first(&r);
    assert_eq!(rows[&2][1].as_i64(), Some(i64::MAX));
    assert_eq!(rows[&2][2], "Hello World");
    assert_eq!(rows[&3][2], "Ærøskøbing über café");
    assert!(rows[&6][1].is_null());
}

#[test]
fn bacpac_serve_merges_extension_companions() {
    let sym = exttest_symbols();
    let r = &run_on(
        "typeprobe.bacpac",
        Some(&sym),
        &[r#"{"id": 1, "cmd": "read", "table": "exttest", "merge-extensions": true, "select": "id,own,extra,num"}"#],
    )[0];
    assert!(ok(r), "{r}");
    let rows = rows_by_first(r);
    assert_eq!(rows[&1][2], "ext-one");
    assert_eq!(rows[&1][3], 11);
    assert!(rows[&2][2].is_null());
    assert!(rows[&3][3].is_null());
}

#[test]
fn bacpac_serve_lists_tables_and_reports_no_storage_compression() {
    let r = one("typeprobe.bacpac", r#"{"id": 1, "cmd": "tables"}"#);
    let dense = r["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "probe_dense")
        .unwrap()
        .clone();
    assert_eq!(dense["rows"], 4000);
    assert_eq!(dense["compression"], "-");
}

// ---- the request surface (RequestSurfaceTests.cs) --------------------------------------

const BOTH: [&str; 2] = ["typeprobe.bak", "typeprobe.bacpac"];

#[test]
fn column_name_with_a_trailing_space_can_be_selected() {
    for f in BOTH {
        // probe_oddnames has the columns [pad ] and [ pad]. A token is matched as written
        // before it is trimmed, so "pad " reaches the trailing-space column.
        let r = one(
            f,
            r#"{"id":1,"cmd":"read","table":"probe_oddnames","select":"id,pad "}"#,
        );
        assert!(ok(&r), "{f}: {r}");
        let rows = rows_by_first(&r);
        assert_eq!(rows[&1][1], "trailing-one", "{f}");
        assert!(rows[&2][1].is_null(), "{f}");
    }
}

#[test]
fn column_name_with_a_leading_space_can_be_selected() {
    for f in BOTH {
        let r = one(
            f,
            r#"{"id":1,"cmd":"read","table":"probe_oddnames","select":"id, pad"}"#,
        );
        assert!(ok(&r), "{f}: {r}");
        let rows = rows_by_first(&r);
        assert_eq!(rows[&1][1], "leading-one");
        assert_eq!(rows[&2][1], "leading-two");
    }
}

#[test]
fn space_after_a_comma_still_means_nothing() {
    for f in BOTH {
        for sel in ["id,amt", "id, amt"] {
            let r = one(
                f,
                &format!(r#"{{"id":1,"cmd":"read","table":"probe_oddnames","select":"{sel}"}}"#),
            );
            assert!(ok(&r), "{f}: {r}");
            assert_eq!(headers(&r), ["id", "amt"]);
        }
    }
}

#[test]
fn unknown_column_still_reports_the_name_as_written() {
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"probe","select":"c_nosuch"}"#,
    ));
    assert!(e.contains("c_nosuch") && e.contains("not found"), "{e}");
}

#[test]
fn sha256_naming_no_selected_column_is_refused() {
    for f in BOTH {
        let e = error(&one(
            f,
            r#"{"id":1,"cmd":"read","table":"probe_lob","select":"id,c_vbmax","sha256":"NoSuchColumn"}"#,
        ));
        assert!(e.contains("NoSuchColumn") && e.contains("sha256"), "{e}");
    }
}

#[test]
fn sha256_naming_a_column_that_is_not_selected_is_refused() {
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"probe_lob","select":"id","sha256":"c_vbmax"}"#,
    ));
    assert!(e.contains("c_vbmax"), "{e}");
}

#[test]
fn sha256_still_hashes_the_column_it_names() {
    for f in BOTH {
        let r = one(
            f,
            r#"{"id":1,"cmd":"read","table":"probe_lob","select":"id,c_vbmax","sha256":"c_vbmax"}"#,
        );
        assert!(ok(&r), "{r}");
        // oracle-known: row 1 is the empty value, so its SHA-256 is the empty-input hash
        assert_eq!(
            rows_by_first(&r)[&1][1],
            "sha256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
    }
}

#[test]
fn unknown_request_key_is_refused() {
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"probe","select":"id","totallyBogusKey":"xyz"}"#,
    ));
    assert!(e.contains("totallyBogusKey") && e.contains("select"), "{e}"); // lists what is accepted
}

#[test]
fn misspelled_option_is_refused_rather_than_dropped() {
    // "tpo" for "top" used to answer ok:true with every row.
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"probe","tpo":2}"#,
    ));
    assert!(e.contains("tpo"), "{e}");
}

#[test]
fn misspelled_company_key_is_refused_rather_than_reading_every_company() {
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"ambig","compayn":"ProbeCo"}"#,
    ));
    assert!(e.contains("compayn"), "{e}");
}

#[test]
fn camel_case_merge_extensions_is_refused_and_the_real_key_is_named() {
    let e = error(&one(
        "typeprobe.bak",
        r#"{"id":1,"cmd":"read","table":"exttest","mergeExtensions":true}"#,
    ));
    assert!(e.contains("mergeExtensions") && e.contains("merge-extensions"), "{e}");
}

#[test]
fn key_that_is_real_but_meaningless_for_the_command_is_refused() {
    let e = error(&one("typeprobe.bak", r#"{"id":1,"cmd":"tables","table":"probe"}"#));
    assert!(e.contains("table") && e.contains("tables"), "{e}");
}

#[test]
fn accepted_keys_still_work_and_the_session_survives_a_refusal() {
    let res = run(&[
        r#"{"id":1,"cmd":"read","table":"probe","select":"id","nope":1}"#,
        r#"{"id":2,"cmd":"read","table":"probe","select":"id","top":2}"#,
        r#"{"id":3,"cmd":"tables"}"#,
        r#"{"id":4,"cmd":"companies"}"#,
    ]);
    assert_eq!(res.len(), 4);
    assert!(!ok(&res[0]));
    assert!(ok(&res[1]));
    assert_eq!(res[1]["rows"].as_array().unwrap().len(), 2);
    assert!(ok(&res[2]) && ok(&res[3]));
}

#[test]
fn id_and_cmd_are_always_accepted() {
    let r = one("typeprobe.bak", r#"{"id":"abc","cmd":"companies"}"#);
    assert!(ok(&r));
    assert_eq!(r["id"], "abc");
}

#[test]
fn quit_takes_no_other_keys() {
    let res = run(&[r#"{"id":1,"cmd":"quit","table":"probe"}"#]);
    assert!(error(&res[0]).contains("table"));
}

#[test]
fn top_refuses_a_value_that_is_not_a_row_count() {
    for f in BOTH {
        let e = error(&one(
            f,
            r#"{"id":1,"cmd":"read","table":"probe","select":"id","top":"abc"}"#,
        ));
        assert!(e.contains("top") && e.contains("abc"), "{e}");
    }
}
