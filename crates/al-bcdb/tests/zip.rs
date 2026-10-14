mod common;
use al_bcdb::zip::ZipIndex;
use al_bcdb::Error;
use common::*;

#[test]
fn reads_stored_entries_back() {
    let z = build_zip(&[("a.txt".into(), b"hello".to_vec()), ("dir/b.bin".into(), vec![1, 2, 3])]);
    let idx = ZipIndex::parse(&z).unwrap();
    assert_eq!(idx.entries.len(), 2);
    assert_eq!(idx.read(&z, "a.txt").unwrap(), b"hello");
    assert_eq!(idx.read(&z, "dir/b.bin").unwrap(), vec![1, 2, 3]);
}

#[test]
fn reads_deflated_entries_of_a_real_package() {
    let bytes = fixture_bytes("typeprobe.bacpac");
    let idx = ZipIndex::parse(&bytes).unwrap();
    assert_eq!(idx.entries.len(), 39);
    let dac = String::from_utf8(idx.read(&bytes, "DacMetadata.xml").unwrap()).unwrap();
    assert!(dac.contains("<Name>typeprobe</Name>"), "{dac}");
    let model = idx.read(&bytes, "model.xml").unwrap();
    assert_eq!(model.len() as u64, idx.find("model.xml").unwrap().size);
}

#[test]
fn something_that_is_not_a_zip_is_refused() {
    let e = ZipIndex::parse(&vec![7u8; 100]).unwrap_err();
    assert!(matches!(e, Error::InvalidData(_)));
    assert!(e.message().contains("not a zip"), "{e}");
}

#[test]
fn a_missing_entry_is_named() {
    let z = build_zip(&[("a".into(), vec![])]);
    let e = ZipIndex::parse(&z).unwrap().read(&z, "model.xml").unwrap_err();
    assert!(e.message().contains("model.xml"), "{e}");
}
