//! Hermetic end-to-end tests against fixtures/typeprobe.bacpac, a sqlpackage export of the
//! same `typeprobe` database as fixtures/typeprobe.bak. Expected values are the oracle
//! fixtures (SELECT output from a real SQL Server) that the .bak tests assert against, so
//! a bacpac read and a backup read of one database must produce identical output.
//! Port of BacpacEndToEndTests.cs.

mod common;
use al_bcdb::source::read_all;
use al_bcdb::value::Value;
use al_bcdb::{open, BcSource};
use common::*;

fn src() -> Box<dyn BcSource> {
    open(Box::new(fixture_bytes("typeprobe.bacpac"))).unwrap()
}

#[test]
fn all_nullable_types_match_oracle() {
    // The three tables hold identical data under no/row/page compression in the .bak;
    // a bacpac has no storage form at all, so all three must read the same.
    let s = src();
    for (t, f) in [
        ("probe", "typeprobe-probe.tsv"),
        ("probe_row", "typeprobe-probe-row.tsv"),
        ("probe_page", "typeprobe-probe-page.tsv"),
    ] {
        assert_eq!(fixture(f), read_table(&*s, t, PROBE_COLS), "{t}");
    }
}

#[test]
fn non_nullable_columns_match_oracle() {
    // Nullability decides the BCP prefix width: a non-nullable fixed-length column is
    // written raw with no prefix at all, so getting this wrong misaligns every later column.
    let cols = [
        "id", "n_tinyint", "n_smallint", "n_int", "n_bigint", "n_bit", "n_dec38_20", "n_dec18_2", "n_dec5_0",
        "n_datetime", "n_datetime2_7", "n_datetime2_0", "n_date", "n_time7", "n_time0", "n_guid", "n_nvarchar",
        "n_varchar", "n_nchar", "n_char", "n_binary", "n_varbinary", "n_vbmax", "n_nvmax", "n_ver",
    ];
    assert_eq!(fixture("typeprobe-probe-notnull.tsv"), read_table(&*src(), "probe_notnull", &cols));
}

#[test]
fn non_nullable_floats_decode_exactly() {
    let s = src();
    let t = table(&*s, "probe_notnull");
    let sel = pick(&s.columns(t).unwrap(), &["id", "n_real", "n_float"]);
    let rows = read_all(&*s, t, &sel).unwrap();
    let by_id = |id: i64| rows.iter().find(|r| r[0] == Value::Int(id)).unwrap().clone();
    assert_eq!((by_id(1)[1].clone(), by_id(1)[2].clone()), (Value::Real(0.0), Value::Float(0.0)));
    assert_eq!((by_id(2)[1].clone(), by_id(2)[2].clone()), (Value::Real(1.5), Value::Float(2.25)));
    assert_eq!((by_id(3)[1].clone(), by_id(3)[2].clone()), (Value::Real(-1.5), Value::Float(-2.25)));
}

#[test]
fn many_data_files_concatenate_in_order() {
    // probe_dense is written as seven TableData-NNN-00000.BCP files
    assert_eq!(
        fixture("typeprobe-probe-dense.tsv"),
        read_table(&*src(), "probe_dense", &["id", "grp", "amount", "posted", "note"])
    );
}

#[test]
fn legacy_and_max_lobs_are_inline() {
    // probe_lob row 4 is a 160,000-byte value
    assert_eq!(
        fixture("typeprobe-probe-lob.tsv"),
        read_table(&*src(), "probe_lob", &["id", "c_image", "c_text", "c_ntext", "c_vbmax", "c_nvmax"])
    );
}

#[test]
fn lobs_from_the_page_compressed_table() {
    assert_eq!(
        fixture("typeprobe-probe-lob-page.tsv"),
        read_table(&*src(), "probe_lob_page", &["id", "c_image", "c_vbmax", "c_nvmax"])
    );
}

#[test]
fn multi_link_lobs() {
    assert_eq!(fixture("typeprobe-probe-lob2.tsv"), read_table(&*src(), "probe_lob2", &["id", "c_image", "c_vbmax"]));
}

#[test]
fn nulled_and_updated_lobs() {
    assert_eq!(
        fixture("typeprobe-probe-lob-upd.tsv"),
        read_table(&*src(), "probe_lob_upd", &["id", "c_image", "c_text"])
    );
}

#[test]
fn ghost_rows_are_not_in_the_export() {
    assert_eq!(fixture("typeprobe-probe-ghost.tsv"), read_table(&*src(), "probe_ghost", &["id", "val", "amt"]));
}

#[test]
fn wide_table() {
    assert_eq!(
        fixture("typeprobe-probe-wide.tsv"),
        read_table(&*src(), "probe_wide", &["id", "c1", "c100", "c199", "c200", "wtext", "wdec"])
    );
}

#[test]
fn altered_tables() {
    // Dropped columns leave no trace in model.xml
    let s = src();
    assert_eq!(
        fixture("typeprobe-probe-altered.tsv"),
        read_table(&*s, "probe_altered", &["id", "b", "d", "b1", "b2", "e", "f", "b3", "g"])
    );
    assert_eq!(
        fixture("typeprobe-probe-altered-page.tsv"),
        read_table(&*s, "probe_altered_page", &["id", "b", "d", "b1", "b2", "e", "f", "b3"])
    );
}

#[test]
fn heap() {
    assert_eq!(fixture("typeprobe-probe-heap.tsv"), read_table(&*src(), "probe_heap", &["id", "txt", "amt"]));
}

#[test]
fn change_tracked_table() {
    // Change tracking's internal in-row version column is not a column of the logical table.
    assert_eq!(
        fixture("typeprobe-probe-tracked.tsv"),
        read_table(&*src(), "probe_tracked", &["id", "g", "txt", "amt"])
    );
}

#[test]
fn row_overflow() {
    assert_eq!(
        fixture("typeprobe-probe-overflow.tsv"),
        read_table(&*src(), "probe_overflow", &["id", "v1", "v2", "n1"])
    );
}

#[test]
fn table_list_matches_the_probe_database() {
    let s = src();
    let names: Vec<&str> = s.tables().unwrap().iter().map(|t| t.name.as_str()).collect();
    for n in ["probe", "probe_notnull", "$probe$platform", "TP$exttest$aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa$ext"] {
        assert!(names.contains(&n), "missing {n}");
    }
    // sqlpackage does not export change tracking's internal side tables
    assert!(!names.iter().any(|n| n.starts_with("MSchange_tracking")));
}

#[test]
fn row_counts_are_reported() {
    let s = src();
    assert_eq!(s.row_count(table(&*s, "probe_dense")).unwrap(), 4000);
    assert_eq!(s.row_count(table(&*s, "probe_ghost")).unwrap(), 334);
    assert_eq!(s.row_count(table(&*s, "probe_notnull")).unwrap(), 3);
}

#[test]
fn compression_column_reads_dash() {
    let s = src();
    assert_eq!(table(&*s, "probe").compression, "-");
}

#[test]
fn clustered_key_comes_from_the_primary_key() {
    let s = src();
    assert_eq!(s.row_key_columns(table(&*s, "probe_dense")).unwrap(), vec!["id".to_string()]);
}

#[test]
fn column_metadata_carries_sql_types_and_widths() {
    let s = src();
    let cols = s.columns(table(&*s, "probe")).unwrap();
    let c = |n: &str| cols.iter().find(|c| c.name == n).unwrap().clone();
    assert_eq!((c("c_nvarchar").xtype, c("c_nvarchar").max_length), (231, 200)); // nvarchar(100): bytes
    assert_eq!((c("c_dec38_20").xtype, c("c_dec38_20").precision, c("c_dec38_20").scale), (106, 38, 20));
    assert_eq!((c("c_time7").xtype, c("c_time7").scale), (41, 7));
    assert_eq!(c("c_varbinary").max_length, 100);
    let lob = s.columns(table(&*s, "probe_lob")).unwrap();
    assert_eq!(lob.iter().find(|c| c.name == "c_nvmax").unwrap().max_length, -1); // (max) is -1
    let nn = s.columns(table(&*s, "probe_notnull")).unwrap();
    assert_eq!(nn.iter().find(|c| c.name == "n_ver").unwrap().xtype, 189);
}

#[test]
fn selecting_one_column_still_decodes_it() {
    // Unselected columns are skipped without being decoded; a stream-position bug would show here first.
    let s = src();
    let t = table(&*s, "probe");
    let col = pick(&s.columns(t).unwrap(), &["c_nvarchar"]);
    let vals: Vec<String> = read_all(&*s, t, &col).unwrap().iter().map(|r| r[0].to_tsv()).collect();
    assert!(vals.contains(&"Hello World".to_string()));
    assert!(vals.contains(&"Ærøskøbing über café".to_string()));
    assert_eq!(vals.len(), 8);
}

#[test]
fn top_stops_the_read() {
    let s = src();
    let t = table(&*s, "probe_dense");
    let cols = pick(&s.columns(t).unwrap(), &["id"]);
    let mut n = 0;
    s.read_rows(t, &cols, &mut |_| {
        n += 1;
        Ok(n < 5)
    })
    .unwrap();
    assert_eq!(n, 5);
}

#[test]
fn banner_names_database_and_producer() {
    let b = src().banner().unwrap();
    assert!(b.contains("typeprobe") && b.contains("170.5.76.0") && b.contains("tables in model.xml"), "{b}");
}
