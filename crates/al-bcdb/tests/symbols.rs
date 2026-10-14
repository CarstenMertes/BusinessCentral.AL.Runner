//! Port of SymbolTests.cs.

mod common;
use al_bcdb::symbols::{normalize_sql_name, SymbolStore};
use al_bcdb::Error;
use common::build_zip;

const SAMPLE: &str = r##"
{
  "AppId": "11111111-2222-3333-4444-555555555555",
  "Name": "Test App",
  "Publisher": "T",
  "Tables": [
    { "Id": 50100, "Name": "Top Level",
      "Fields": [
        { "Id": 1, "Name": "No.", "TypeDefinition": { "Name": "Code[20]" } },
        { "Id": 2, "Name": "Sum", "TypeDefinition": { "Name": "Decimal" },
          "Properties": [ { "Name": "FieldClass", "Value": "FlowField" }, { "Name": "Other", "Value": 5 } ] }
      ] }
  ],
  "Namespaces": [
    { "Name": "Inner",
      "Tables": [
        { "Id": 50101, "Name": "Nested Table",
          "Fields": [
            { "Id": 1, "Name": "Type", "TypeDefinition": { "Name": "Enum", "Subtype": { "Name": "My Enum", "Id": 50102 } } }
          ] }
      ],
      "TableExtensions": [
        { "Id": 50200, "Name": "Cust Ext", "TargetObject": "#437dbf0e84ff417a965ded2bb9650972#Customer",
          "Fields": [ { "Id": 50000, "Name": "My Field.", "TypeDefinition": { "Name": "Text[30]" } } ] }
      ],
      "Namespaces": [
        { "Name": "Deep", "Tables": [ { "Id": 50103, "Name": "Deep/One", "Fields": [] } ] }
      ]
    }
  ]
}"##;

fn from_json(json: &str) -> al_bcdb::Result<SymbolStore> {
    SymbolStore::load([("sample.json", json.as_bytes().to_vec())])
}

#[test]
fn walks_namespaces_recursively() {
    let s = from_json(SAMPLE).unwrap();
    assert_eq!(s.tables.len(), 3);
    assert_eq!(s.find("Top Level").len(), 1);
    assert_eq!(s.find("Nested Table").len(), 1);
    assert_eq!(s.find("Deep/One").len(), 1);
}

#[test]
fn maps_al_names_and_types() {
    let s = from_json(SAMPLE).unwrap();
    let t = s.find("Top Level")[0];
    assert_eq!((t.id, t.app_name.as_str()), (50100, "Test App"));
    assert_eq!(t.fields[0].type_name, "Code[20]");
    assert_eq!(t.fields[1].field_class, "FlowField");
    assert_eq!(s.find("Nested Table")[0].fields[0].type_name, "Enum \"My Enum\"");
}

#[test]
fn find_for_sql_table_uses_bc_normalization() {
    let s = from_json(SAMPLE).unwrap();
    // '/' becomes '_' in SQL object names
    assert!(s.find_for_sql_table("Deep_One", Some("11111111-2222-3333-4444-555555555555")).is_some());
    assert!(s.find_for_sql_table("Deep_One", Some("99999999-0000-0000-0000-000000000000")).is_none());
    assert!(s.find_for_sql_table("deep_one", None).is_some());
}

#[test]
fn sql_name_normalization() {
    assert_eq!(normalize_sql_name("No."), "No_");
    assert_eq!(normalize_sql_name("G/L Account"), "G_L Account");
    assert_eq!(normalize_sql_name("A.B\"C\\D/E'F%G[H"), "A_B_C_D_E_F_G_H");
}

#[test]
fn table_extensions_carry_a_qualified_target() {
    let s = from_json(SAMPLE).unwrap();
    let e = &s.table_extensions[0];
    assert_eq!(e.target_table, "Customer");
    assert_eq!(e.target_app_id.as_deref(), Some("437dbf0e-84ff-417a-965d-ed2bb9650972"));
    let (ext, f) = s
        .find_extension_field("11111111-2222-3333-4444-555555555555", "Customer", Some("437dbf0e-84ff-417a-965d-ed2bb9650972"), "My Field_")
        .unwrap();
    assert_eq!((ext.id, f.id), (50200, 50000));
    assert!(s.find_extension_field("11111111-2222-3333-4444-555555555555", "Vendor", None, "My Field_").is_none());
}

#[test]
fn loads_a_navx_packaged_app() {
    // 40-byte header (magic + header length), then a zip containing SymbolReference.json
    let zip = build_zip(&[("SymbolReference.json".into(), SAMPLE.as_bytes().to_vec())]);
    let mut navx = vec![0u8; 40];
    navx[..4].copy_from_slice(b"NAVX");
    navx[4..8].copy_from_slice(&40i32.to_le_bytes());
    navx.extend(zip);
    let s = SymbolStore::load([("x.app", navx)]).unwrap();
    assert_eq!(s.tables.len(), 3);
}

#[test]
fn loads_the_runtime_wrapper_app() {
    let inner_zip = build_zip(&[("SymbolReference.json".into(), SAMPLE.as_bytes().to_vec())]);
    let mut inner = vec![0u8; 40];
    inner[..4].copy_from_slice(b"NAVX");
    inner[4..8].copy_from_slice(&40i32.to_le_bytes());
    inner.extend(inner_zip);
    let wrapper = build_zip(&[("readytorunappmanifest.json".into(), b"{}".to_vec()), ("abc_1.0_28.app".into(), inner)]);
    let mut outer = vec![0u8; 40];
    outer[..4].copy_from_slice(b"NAVX");
    outer[4..8].copy_from_slice(&40i32.to_le_bytes());
    outer.extend(wrapper);
    assert_eq!(SymbolStore::load([("x.app", outer)]).unwrap().tables.len(), 3);
}

#[test]
fn missing_app_id_throws() {
    let e = from_json(r#"{ "Name": "x", "Tables": [] }"#).err().unwrap();
    assert!(matches!(e, Error::InvalidData(_)));
    assert!(e.message().contains("AppId"), "{e}");
}

#[test]
fn something_that_is_not_a_package_is_refused() {
    let e = SymbolStore::load([("junk.app", vec![1, 2, 3, 4, 5, 6, 7, 8, 9])]).err().unwrap();
    assert!(e.message().contains("not a NAVX/zip"), "{e}");
}

/// Needs the BC 28.1 artifact in the artifact cache (never in the repo):
/// `cargo test -p al-bcdb --test symbols -- --ignored`.
#[test]
#[ignore = "needs ~/.bcartifacts.cache/sandbox/28.1.49838.50621 (Microsoft artifact, not in the repo)"]
fn loads_the_shipped_base_application() {
    let home = std::env::var("HOME").unwrap();
    let app = format!("{home}/.bcartifacts.cache/sandbox/28.1.49838.50621/w1/Extensions/Microsoft_Base Application_28.1.49838.50621.app");
    let bytes = std::fs::read(&app).unwrap_or_else(|e| panic!("{app}: {e}"));
    let s = SymbolStore::load([(app.as_str(), bytes)]).unwrap();
    let cust = s.find("Customer");
    assert_eq!(cust.len(), 1);
    assert_eq!(cust[0].id, 18);
    assert_eq!(cust[0].app_id, "437dbf0e-84ff-417a-965d-ed2bb9650972");
    assert_eq!(cust[0].fields.iter().find(|f| f.id == 1).unwrap().type_name, "Code[20]");
}
