//! The native-BCP row framing of a .bacpac data stream asserted on hand-written bytes, plus
//! the guards that must fire when a stream or package does not match the derived rules.
//! Every byte sequence was observed in fixtures/typeprobe.bacpac for a row whose value the
//! oracle states (bcdb PROVENANCE.md "bacpac: native BCP row framing"). Port of
//! BacpacFramingTests.cs.

mod common;
use al_bcdb::bacpac::BacpacSource;
use al_bcdb::bcp::{BacpacColumn, BcpRowReader};
use al_bcdb::value::Value;
use al_bcdb::zip::ZipIndex;
use al_bcdb::{Error, open};
use common::*;

fn col(name: &str, ty: &str) -> BacpacColumn {
    BacpacColumn::from_model(name, ty, true, false, 0, 0, 0)
}
fn notnull(name: &str, ty: &str) -> BacpacColumn {
    BacpacColumn::from_model(name, ty, false, false, 0, 0, 0)
}
fn sized(name: &str, ty: &str, nullable: bool, length: i32) -> BacpacColumn {
    BacpacColumn::from_model(name, ty, nullable, false, length, 0, 0)
}
fn dec(name: &str, p: u8, s: u8) -> BacpacColumn {
    BacpacColumn::from_model(name, "decimal", false, false, 0, p, s)
}
fn max(name: &str, ty: &str, nullable: bool) -> BacpacColumn {
    BacpacColumn::from_model(name, ty, nullable, true, 0, 0, 0)
}
fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn read(data: &[u8], cols: &[BacpacColumn]) -> al_bcdb::Result<Vec<Vec<Value>>> {
    let mut rows = Vec::new();
    BcpRowReader::new("t", cols).read(data, cols, &mut |r| {
        rows.push(r.to_vec());
        Ok(true)
    })?;
    Ok(rows)
}

#[test]
fn prefix_widths_follow_the_derived_rule() {
    let p = BcpRowReader::prefix_length;
    // Fixed-length types: no prefix when NOT NULL, a one-byte length when nullable.
    assert_eq!(p(&notnull("a", "int")), 0);
    assert_eq!(p(&col("a", "int")), 1);
    assert_eq!(p(&BacpacColumn::from_model("a", "datetime2", false, false, 0, 0, 7)), 0);
    // char(n) is the only character type that follows that rule.
    assert_eq!(p(&sized("a", "char", false, 10)), 0);
    assert_eq!(p(&sized("a", "char", true, 10)), 2);
    // bit, uniqueidentifier and decimal always carry a one-byte prefix.
    assert_eq!(p(&notnull("a", "bit")), 1);
    assert_eq!(p(&notnull("a", "uniqueidentifier")), 1);
    assert_eq!(p(&dec("a", 18, 2)), 1);
    // The variable-length family always carries two bytes, including nchar(n).
    assert_eq!(p(&sized("a", "nchar", false, 10)), 2);
    assert_eq!(p(&sized("a", "nvarchar", false, 50)), 2);
    assert_eq!(p(&sized("a", "varbinary", false, 50)), 2);
    assert_eq!(p(&notnull("a", "rowversion")), 2);
    // Legacy LOBs four bytes, (max) types eight.
    assert_eq!(p(&col("a", "image")), 4);
    assert_eq!(p(&col("a", "text")), 4);
    assert_eq!(p(&max("a", "varbinary", true)), 8);
    assert_eq!(p(&max("a", "nvarchar", false)), 8);
}

#[test]
fn platform_table_row() {
    // fixtures/typeprobe.bacpac, [$probe$platform] = (1, N'platform-one'), (2, NULL).
    let mut data = vec![0x01, 0, 0, 0, 0x18, 0];
    data.extend(utf16("platform-one"));
    data.extend([0x02, 0, 0, 0, 0xFF, 0xFF]);
    let rows = read(&data, &[notnull("id", "int"), sized("v", "nvarchar", true, 20)]).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], vec![Value::Int(1), Value::Text("platform-one".into())]);
    assert_eq!(rows[1], vec![Value::Int(2), Value::Null]);
}

#[test]
fn nullable_fixed_length_null_is_all_ones() {
    let rows = read(&[0xFF, 0x04, 0x2A, 0, 0, 0], &[col("a", "int"), col("b", "int")]).unwrap();
    assert_eq!(rows[0], vec![Value::Null, Value::Int(42)]);
}

#[test]
fn empty_is_not_null() {
    let rows = read(
        &[0, 0, 0xFF, 0xFF],
        &[sized("a", "nvarchar", true, 10), sized("b", "nvarchar", true, 10)],
    )
    .unwrap();
    assert_eq!(rows[0], vec![Value::Text(String::new()), Value::Null]);
}

#[test]
fn decimal_carries_its_own_precision_and_scale() {
    // [len 19][precision][scale][sign: 1 = positive][16-byte magnitude, little-endian].
    let mut body = vec![19, 18, 2, 1];
    body.extend([0u8; 16]);
    body[4] = 0x7D; // 125 -> 1.25 at scale 2
    assert_eq!(read(&body, &[dec("d", 18, 2)]).unwrap()[0][0].to_string(), "1.25");
    body[3] = 0;
    body[2] = 2;
    assert_eq!(read(&body, &[dec("d", 18, 2)]).unwrap()[0][0].to_string(), "-1.25");
}

#[test]
fn time_and_datetime2_are_always_full_width_hundred_nanosecond_units() {
    let t0: Vec<u8> = 863_990_000_000i64.to_le_bytes()[..5].to_vec(); // 23:59:59
    assert_eq!(
        read(&t0, &[notnull("t", "time")]).unwrap()[0][0].to_string(),
        "23:59:59"
    );
    let mut dt2 = t0.clone();
    dt2.extend([0x5B, 0x95, 0x0A]); // 1900-01-01
    assert_eq!(
        read(&dt2, &[notnull("d", "datetime2")]).unwrap()[0][0].to_string(),
        "1900-01-01 23:59:59"
    );
    let c7 = BacpacColumn::from_model("d", "datetime2", false, false, 0, 0, 7);
    assert_eq!(
        read(&dt2, &[c7]).unwrap()[0][0].to_string(),
        "1900-01-01 23:59:59.0000000"
    );
}

#[test]
fn datetime_halves_are_swapped_relative_to_storage() {
    // BCP: [i32 days since 1900][u32 ticks]. 1980-06-15 18:45:30.123: days 29385, ticks 5,636,437 -> .123
    let days = 29_385i32;
    let ticks = ((18 * 3600 + 45 * 60 + 30) * 300 + 37) as u32; // 37/300 s = 123.3 ms -> .123
    let mut d = days.to_le_bytes().to_vec();
    d.extend(ticks.to_le_bytes());
    assert_eq!(
        read(&d, &[notnull("d", "datetime")]).unwrap()[0][0].to_string(),
        "1980-06-15 18:45:30.123"
    );
}

#[test]
fn non_unicode_text_is_exported_as_utf16() {
    let mut data = vec![0x0A, 0];
    data.extend(utf16("Hello"));
    assert_eq!(
        read(&data, &[sized("v", "varchar", true, 100)]).unwrap()[0][0].to_string(),
        "Hello"
    );
    let ch = utf16("xyz       "); // char(10), padded by the server
    assert_eq!(
        read(&ch, &[sized("c", "char", false, 10)]).unwrap()[0][0].to_string(),
        "xyz       "
    );
}

#[test]
fn rowversion_is_eight_bytes_big_endian() {
    let data = [8, 0, 0, 0, 0, 0, 0, 0, 0x07, 0xE9];
    assert_eq!(
        read(&data, &[notnull("v", "rowversion")]).unwrap()[0][0].to_string(),
        "0x00000000000007E9"
    );
}

// ---- guards -------------------------------------------------------------------

#[test]
fn prefix_that_disagrees_with_the_type_width_throws() {
    let e = read(&[0x02, 0, 0], &[col("a", "int")]).unwrap_err();
    assert!(matches!(e, Error::InvalidData(_)));
    for needle in ["a", "int", "2"] {
        assert!(e.message().contains(needle), "{e}");
    }
}

#[test]
fn decimal_whose_precision_disagrees_with_the_model_throws() {
    let mut body = vec![19, 38, 2, 1];
    body.extend([0u8; 16]);
    let e = read(&body, &[dec("d", 18, 2)]).unwrap_err();
    assert!(e.message().contains('d') && e.message().contains("38"), "{e}");
}

#[test]
fn truncated_stream_throws_naming_the_table() {
    let e = read(&[0x01, 0x00], &[notnull("id", "int")]).unwrap_err();
    assert!(matches!(e, Error::InvalidData(_)));
    assert!(e.message().contains('t') && e.message().contains("id"), "{e}");
}

#[test]
fn chunked_max_value_is_refused_rather_than_guessed() {
    // 0xFFFFFFFFFFFFFFFE is SQL Server's "length unknown, chunks follow" form.
    let e = read(
        &[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        &[max("b", "varbinary", true)],
    )
    .unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(
        e.message().contains('b') && e.message().contains("refusing to guess"),
        "{e}"
    );
}

#[test]
fn type_whose_framing_is_not_derived_throws_naming_it() {
    let e = read(&[0u8; 8], &[notnull("m", "money")]).unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(e.message().contains('m') && e.message().contains("money"), "{e}");
}

// ---- container guards ---------------------------------------------------------

/// The fixture package with one entry's text edited, as a stored zip.
fn rewrite(entry: &str, edit: impl Fn(&str) -> String) -> Vec<u8> {
    let bytes = fixture_bytes("typeprobe.bacpac");
    let idx = ZipIndex::parse(&bytes).unwrap();
    let entries: Vec<(String, Vec<u8>)> = idx
        .entries
        .iter()
        .map(|e| {
            let data = idx.read(&bytes, &e.name).unwrap();
            if e.name == entry {
                // keep a leading byte-order mark out of the edit
                let text = String::from_utf8(data).unwrap();
                (e.name.clone(), edit(&text).into_bytes())
            } else {
                (e.name.clone(), data)
            }
        })
        .collect();
    build_zip(&entries)
}

#[test]
fn model_checksum_mismatch_throws() {
    // model.xml is parsed on first use rather than at open, so the guard fires on the
    // first query, before any row is returned.
    let z = rewrite("model.xml", |s| s.replace("probe_dense", "probe_dxnse"));
    let src = open(Box::new(z)).unwrap();
    let e = src.tables().unwrap_err();
    assert!(matches!(e, Error::InvalidData(_)));
    assert!(
        e.message().contains("model.xml") && e.message().contains("checksum"),
        "{e}"
    );
}

#[test]
fn unknown_data_stream_version_throws() {
    let z = rewrite("Origin.xml", |s| s.replace("\"Data\">2.0.0.0<", "\"Data\">3.0.0.0<"));
    let e = BacpacSource::open(Box::new(z)).err().expect("must refuse");
    assert!(matches!(e, Error::InvalidData(_)));
    assert!(
        e.message().contains("3.0.0.0") && e.message().contains("refusing to guess"),
        "{e}"
    );
}

#[test]
fn a_data_stream_with_no_table_in_the_model_throws() {
    let bytes = fixture_bytes("typeprobe.bacpac");
    let idx = ZipIndex::parse(&bytes).unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = idx
        .entries
        .iter()
        .map(|e| (e.name.clone(), idx.read(&bytes, &e.name).unwrap()))
        .collect();
    entries.push(("Data/dbo.ghost_table/TableData-000-00000.BCP".into(), vec![1, 2, 3]));
    let src = open(Box::new(build_zip(&entries))).unwrap();
    let e = src.tables().unwrap_err();
    assert!(
        e.message().contains("ghost_table") && e.message().contains("refusing to guess"),
        "{e}"
    );
}
