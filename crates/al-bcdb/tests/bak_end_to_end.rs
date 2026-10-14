//! Hermetic end-to-end tests against fixtures/typeprobe.bak, a real SQL Server backup of a
//! small scratch database (bcdb tools/typeprobe.sql). Expected values are oracle fixtures
//! exported from SELECT on that same database. Port of TypeprobeEndToEndTests.cs and
//! NullabilityTests.cs. These run everywhere with no SQL Server and no BC artifacts.

mod common;
use al_bcdb::bak::BakSource;
use al_bcdb::catalog::Catalog;
use al_bcdb::pagefile::PageFile;
use al_bcdb::source::read_all;
use al_bcdb::{BcSource, Error, open};
use common::*;
use std::sync::atomic::Ordering;

fn src() -> Box<dyn BcSource> {
    open(Box::new(fixture_bytes("typeprobe.bak"))).unwrap()
}

#[test]
fn all_types_match_oracle_uncompressed_row_and_page_compressed() {
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
fn compression_is_reported_per_table() {
    let s = src();
    assert_eq!(table(&*s, "probe").compression, "none");
    assert_eq!(table(&*s, "probe_row").compression, "row");
    assert_eq!(table(&*s, "probe_page").compression, "page");
}

#[test]
fn page_compression_with_anchors_and_dictionary() {
    assert_eq!(
        fixture("typeprobe-probe-dense.tsv"),
        read_table(&*src(), "probe_dense", &["id", "grp", "amount", "posted", "note"])
    );
}

#[test]
fn legacy_and_max_lobs() {
    assert_eq!(
        fixture("typeprobe-probe-lob.tsv"),
        read_table(
            &*src(),
            "probe_lob",
            &["id", "c_image", "c_text", "c_ntext", "c_vbmax", "c_nvmax"]
        )
    );
}

#[test]
fn lobs_under_page_compression() {
    assert_eq!(
        fixture("typeprobe-probe-lob-page.tsv"),
        read_table(&*src(), "probe_lob_page", &["id", "c_image", "c_vbmax", "c_nvmax"])
    );
}

#[test]
fn multi_link_lob_roots() {
    assert_eq!(
        fixture("typeprobe-probe-lob2.tsv"),
        read_table(&*src(), "probe_lob2", &["id", "c_image", "c_vbmax"])
    );
}

#[test]
fn ghost_records_in_compressed_pages_are_skipped() {
    // 500 rows inserted, 166 deleted right before BACKUP: the page carries 166 ghost CD
    // records that SELECT does not return, and neither must the reader.
    assert_eq!(
        fixture("typeprobe-probe-ghost.tsv"),
        read_table(&*src(), "probe_ghost", &["id", "val", "amt"])
    );
}

#[test]
fn wide_table_uses_two_byte_column_count() {
    // 203 columns: the CD column count takes the two-byte form (BC tables reach 216 columns).
    assert_eq!(
        fixture("typeprobe-probe-wide.tsv"),
        read_table(
            &*src(),
            "probe_wide",
            &["id", "c1", "c100", "c199", "c200", "wtext", "wdec"]
        )
    );
}

#[test]
fn altered_tables_decode_by_physical_layout() {
    // Columns dropped and added after rows existed: physical order, offsets, null bits and
    // the record column count all diverge from declaration order. The sysrscols leaf layout
    // is the only correct source (an upgraded production database first exposed this).
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
fn heap_with_empty_slots_and_churn() {
    // 99 slot-array entries are 0 (empty slots) and must be skipped the way SQL Server's
    // own scan skips them; a production heap first exposed this as phantom all-NULL rows.
    assert_eq!(
        fixture("typeprobe-probe-heap.tsv"),
        read_table(&*src(), "probe_heap", &["id", "txt", "amt"])
    );
}

#[test]
fn change_tracked_table_skips_internal_version_column() {
    // Change tracking adds an internal in-row bigint version column whose sysrscols rscolid
    // carries flag 0x08000000; its masked low bits collide with a real column id.
    assert_eq!(
        fixture("typeprobe-probe-tracked.tsv"),
        read_table(&*src(), "probe_tracked", &["id", "g", "txt", "amt"])
    );
}

#[test]
fn updated_and_nulled_legacy_lobs() {
    assert_eq!(
        fixture("typeprobe-probe-lob-upd.tsv"),
        read_table(&*src(), "probe_lob_upd", &["id", "c_image", "c_text"])
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
fn non_nullable_columns() {
    let cols = [
        "id",
        "n_tinyint",
        "n_smallint",
        "n_int",
        "n_bigint",
        "n_bit",
        "n_dec38_20",
        "n_dec18_2",
        "n_dec5_0",
        "n_datetime",
        "n_datetime2_7",
        "n_datetime2_0",
        "n_date",
        "n_time7",
        "n_time0",
        "n_guid",
        "n_nvarchar",
        "n_varchar",
        "n_nchar",
        "n_char",
        "n_binary",
        "n_varbinary",
        "n_vbmax",
        "n_nvmax",
        "n_ver",
    ];
    assert_eq!(
        fixture("typeprobe-probe-notnull.tsv"),
        read_table(&*src(), "probe_notnull", &cols)
    );
}

#[test]
fn a_bak_and_a_bacpac_of_one_database_read_identically() {
    let bak = src();
    let bacpac = open(Box::new(fixture_bytes("typeprobe.bacpac"))).unwrap();
    for t in ["probe_dense", "probe_lob", "probe_overflow", "probe_notnull"] {
        let cols: Vec<String> = bacpac
            .columns(table(&*bacpac, t))
            .unwrap()
            .iter()
            .map(|c| c.name.clone())
            .filter(|n| n != "n_ver")
            .collect();
        let names: Vec<&str> = cols.iter().map(|s| s.as_str()).collect();
        assert_eq!(read_table(&*bak, t, &names), read_table(&*bacpac, t, &names), "{t}");
    }
}

#[test]
fn row_counts_come_from_the_catalog() {
    let s = src();
    assert_eq!(s.row_count(table(&*s, "probe_dense")).unwrap(), 4000);
    assert_eq!(s.row_count(table(&*s, "probe_notnull")).unwrap(), 3);
}

#[test]
fn clustered_key_comes_from_the_clustered_index() {
    let s = src();
    assert_eq!(
        s.row_key_columns(table(&*s, "probe_dense")).unwrap(),
        vec!["id".to_string()]
    );
}

#[test]
fn top_stops_the_read() {
    let s = src();
    let t = table(&*s, "probe_dense");
    let cols = pick(&s.columns(t).unwrap(), &["id"]);
    let mut n = 0;
    s.read_rows(t, &cols, &mut |_| {
        n += 1;
        Ok(n < 7)
    })
    .unwrap();
    assert_eq!(n, 7);
}

#[test]
fn a_column_asked_for_twice_or_not_stored_is_refused() {
    let s = src();
    let t = table(&*s, "probe");
    let id = pick(&s.columns(t).unwrap(), &["id"]);
    let twice = [id[0].clone(), id[0].clone()];
    assert!(matches!(read_all(&*s, t, &twice), Err(Error::Argument(_))));
}

// ---- nullability (NullabilityTests.cs) -----------------------------------------------

fn nullability(s: &dyn BcSource) -> Vec<String> {
    let mut lines = Vec::new();
    for t in s.tables().unwrap() {
        for c in s.columns(t).unwrap() {
            lines.push(format!("{}|{}|{}", t.name, c.name, i32::from(c.is_nullable)));
        }
    }
    lines.sort();
    lines
}

#[test]
fn bak_nullability_matches_the_oracle() {
    let expected = fixture("typeprobe-nullability.tsv");
    assert_eq!(expected.len(), 387);
    assert_eq!(expected, nullability(&*src()));
}

#[test]
fn bacpac_nullability_matches_the_oracle() {
    // sqlpackage does not export change tracking's internal side tables, so the bacpac
    // carries a subset; every column it does carry must agree.
    let expected: std::collections::HashSet<String> = fixture("typeprobe-nullability.tsv").into_iter().collect();
    let actual = nullability(&*open(Box::new(fixture_bytes("typeprobe.bacpac"))).unwrap());
    assert!(!actual.is_empty());
    let wrong: Vec<_> = actual.iter().filter(|a| !expected.contains(*a)).collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

#[test]
fn not_null_and_nullable_are_both_represented() {
    let actual = nullability(&*src());
    assert_eq!(actual.iter().filter(|a| a.ends_with("|0")).count(), 65);
    assert_eq!(actual.iter().filter(|a| a.ends_with("|1")).count(), 322);
}

// ---- structure -------------------------------------------------------------------------

fn pagefile() -> PageFile {
    PageFile::open(Box::new(fixture_bytes("typeprobe.bak"))).unwrap()
}

#[test]
fn structural_map_self_check() {
    let pf = pagefile();
    let cc = pf.cross_check().unwrap();
    assert!(cc.agree > 300, "only {} self-identifying blocks agree", cc.agree);
    assert!(cc.last_wins_disagreements.is_empty());
}

#[test]
fn derived_file_length_covers_every_mapped_page() {
    // The header's Size is a lower bound on the file, not the end of the copied data (bcdb
    // FileSizeBoundTests): the derived length must cover every page the map placed.
    let pf = pagefile();
    assert!(pf.page_count() > 0);
    assert!(
        pf.file_pages() >= pf.page_count(),
        "{} < {}",
        pf.file_pages(),
        pf.page_count()
    );
    assert_eq!(pf.file_pages() % 8, 0);
}

#[test]
fn catalog_enumerates_probe_tables_only_user_tables() {
    let pf = pagefile();
    let cat = Catalog::open(&pf).unwrap();
    let names: Vec<&str> = cat.objects.iter().map(|o| o.name.as_str()).collect();
    for n in ["probe", "probe_dense", "probe_lob"] {
        assert!(names.contains(&n), "{n}");
    }
    // sysschobjs lists every object, but only type "U " rows are consumed ("UQ" is a unique
    // constraint, not a table).
    assert!(cat.total_object_count > cat.objects.len());
}

#[test]
fn clustered_key_seek_agrees_with_the_full_scan_for_every_object() {
    // syscolpars and sysiscols are clustered on the object id, so one object's columns are
    // found by descending the index. Only worth having if it returns exactly what the scan
    // returns, for every object in the database.
    let pf = pagefile();
    let scanned = Catalog::open(&pf).unwrap().column_metadata_by_scan(&pf).unwrap();
    assert!(!scanned.is_empty());
    let seeking = Catalog::open(&pf).unwrap();
    for (obj, want) in &scanned {
        let got = seeking.load_column_metadata(&pf, *obj).unwrap();
        assert_eq!(&want.columns, &got.columns, "columns of {obj}");
        assert_eq!(&want.index_columns, &got.index_columns, "index columns of {obj}");
    }
    assert_eq!(
        seeking.clustered_seek_declines.load(Ordering::Relaxed),
        0,
        "a seek fell back to a scan"
    );
    assert!(seeking.clustered_seeks.load(Ordering::Relaxed) > 0);
}

#[test]
fn rowset_layout_seek_agrees_with_the_full_scan() {
    let pf = pagefile();
    let cat = Catalog::open(&pf).unwrap();
    let mut compared = 0;
    for o in &cat.objects {
        let Ok(rs) = cat.rowset_for(o.object_id, &[1, 0]) else {
            continue;
        };
        assert_eq!(
            cat.rowset_columns_by_scan(&pf, rs.rowset_id).unwrap(),
            *cat.rowset_columns(&pf, rs.rowset_id).unwrap(),
            "rowset of {}",
            o.name
        );
        compared += 1;
    }
    assert!(compared > 10, "only {compared} rowsets compared");
    assert!(cat.clustered_seeks.load(Ordering::Relaxed) > 0);
}

#[test]
fn seek_on_a_missing_key_returns_nothing_rather_than_the_neighbouring_rows() {
    // Descending lands on the leaf page where the key would be, which for an absent key
    // holds the neighbouring object's rows. Returning those would attach one table's
    // columns to another.
    let pf = pagefile();
    let cat = Catalog::open(&pf).unwrap();
    let unused = cat.objects.iter().map(|o| o.object_id).max().unwrap() + 12345;
    let m = cat.load_column_metadata(&pf, unused).unwrap();
    assert!(m.columns.is_empty() && m.index_columns.is_empty());
}

#[test]
fn alloc_unit_pages_cover_every_page_the_iam_chain_yields() {
    // The prefetch page list is derived without reading each page, so it is a superset of
    // the data pages the table reader ends up using.
    let bak = BakSource::open(Box::new(fixture_bytes("typeprobe.bak"))).unwrap();
    let cat = bak.catalog();
    let obj = cat.objects.iter().find(|o| o.name == "probe_dense").unwrap();
    let au = cat
        .au_for_rowset(cat.rowset_for(obj.object_id, &[1, 0]).unwrap().rowset_id)
        .unwrap();
    let candidates: std::collections::HashSet<i32> =
        cat.alloc_unit_pages(bak.page_file(), au).unwrap().into_iter().collect();
    let mut seen = 0;
    bak.for_each_data_page(au, &mut |pid, _| {
        assert!(
            candidates.contains(&pid),
            "data page {pid} missing from the alloc-unit page list"
        );
        seen += 1;
        Ok(true)
    })
    .unwrap();
    assert!(seen > 0);
}

#[test]
fn something_that_is_not_a_backup_is_refused() {
    let e = open(Box::new(vec![0u8; 4096])).err().expect("must refuse");
    assert!(matches!(e, Error::InvalidData(_)), "{e}");
    assert!(
        e.message().contains("not a SQL Server native backup") || e.message().contains("MQDA"),
        "{e}"
    );
}

#[test]
fn banner_counts_objects_and_pages() {
    let b = src().banner().unwrap();
    assert!(b.contains("objects") && b.contains("pages superseded"), "{b}");
}
