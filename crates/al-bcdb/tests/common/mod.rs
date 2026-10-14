#![allow(dead_code)]
//! Shared helpers for the integration tests.

use al_bcdb::source::{read_all, BcSource, SourceTable};
use al_bcdb::types::SysColumn;
use std::path::PathBuf;

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// An oracle fixture: SELECT output, one row per line; a trailing `|#` sentinel (which keeps
/// trailing spaces in the last column alive in the export) is stripped. Sorted ordinally.
pub fn fixture(file: &str) -> Vec<String> {
    let text = String::from_utf8(fixture_bytes(file)).unwrap();
    let mut v: Vec<String> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.strip_suffix("|#").unwrap_or(l).to_string())
        .collect();
    v.sort();
    v
}

pub fn table<'a>(src: &'a dyn BcSource, name: &str) -> &'a SourceTable {
    src.tables().unwrap().iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no table {name}"))
}

pub fn pick(meta: &[SysColumn], names: &[&str]) -> Vec<SysColumn> {
    names
        .iter()
        .map(|n| meta.iter().find(|c| c.name == *n).unwrap_or_else(|| panic!("no column {n}")).clone())
        .collect()
}

/// A table's rows as `|`-joined text, sorted: the shape the oracle fixtures have.
pub fn read_table(src: &dyn BcSource, name: &str, cols: &[&str]) -> Vec<String> {
    let t = table(src, name);
    let meta = src.columns(t).unwrap();
    let sel = pick(&meta, cols);
    let mut rows: Vec<String> = read_all(src, t, &sel)
        .unwrap_or_else(|e| panic!("reading {name}: {e}"))
        .iter()
        .map(|r| r.iter().map(|v| v.to_tsv()).collect::<Vec<_>>().join("|"))
        .collect();
    rows.sort();
    rows
}

pub const PROBE_COLS: &[&str] = &[
    "id", "c_tinyint", "c_smallint", "c_int", "c_bigint", "c_bit", "c_dec38_20", "c_dec18_2", "c_dec5_0",
    "c_datetime", "c_datetime2_7", "c_datetime2_3", "c_datetime2_0", "c_date", "c_time7", "c_time0", "c_guid",
    "c_nvarchar", "c_varchar", "c_nchar", "c_char", "c_binary", "c_varbinary",
];

/// A stored (uncompressed) zip with the given entries, for tests that edit a package.
pub fn build_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        let mut local = Vec::new();
        local.extend_from_slice(&0x04034b50u32.to_le_bytes());
        local.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // version, flags, method 0, time, date
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(data.len() as u32).to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&local);
        out.extend_from_slice(data);
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 8]); // extra, comment, disk, internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
