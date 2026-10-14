//! Byte sequences taken from real row-compressed nvarchar cells of the typeprobe
//! database, values confirmed against SQL Server SELECT output (bcdb PROVENANCE.md
//! "Unicode compression is SCSU"). Port of ScsuTests.cs.

use al_bcdb::Error;
use al_bcdb::scsu;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn decodes_observed_sql_server_scsu() {
    let cases = [
        ("48656c6c6f20576f726c64", "Hello World"),
        ("c672f8736bf862696e6720fc62657220636166e910", "Ærøskøbing über café"),
        ("129ab8c0b8bbbbb8c6b020c2b5c1c2", "Кириллица тест"),
        (
            "1ffba5cbcbc7cdc9cabc20cac1c9200e4e2d0e65870e5b5720616e64200ed83c0edf8920656d6f6a69",
            "Ελληνικά και 中文字 and 🎉 emoji",
        ),
        ("e6f8e520c6d8c520e920e820fc20f620e420df", "æøå ÆØÅ é è ü ö ä ß"),
        ("616210", "ab"), // trailing 0x10 pad tag emits nothing
        ("", ""),
    ];
    for (h, expected) in cases {
        assert_eq!(scsu::decode(&hex(h)).unwrap(), expected, "input {h}");
    }
}

#[test]
fn rejects_truncated_tag_sequence() {
    // SQU needs 2 bytes
    assert!(matches!(scsu::decode(&[0x0E, 0x4E]), Err(Error::InvalidData(_))));
}

#[test]
fn rejects_reserved_window_index() {
    // SD0 with reserved index 0
    assert!(matches!(scsu::decode(&[0x18, 0x00, 0x41]), Err(Error::InvalidData(_))));
}
