//! ADR-032 step 2: `src/builtin_ids.rs` is generated and checked in; it must be current.

use al_builtins::builtin_id::{BuiltinId, render_table};

#[test]
fn the_checked_in_table_is_what_the_generator_writes() {
    let names = al_builtins::native::all_builtin_names();
    let want = render_table(&names);
    let have = include_str!("../src/builtin_ids.rs");
    assert!(
        want == have,
        "src/builtin_ids.rs is stale: run `cargo run -p al-builtins --example gen_builtin_ids`"
    );
}

#[test]
fn every_catalog_overload_and_every_registered_native_has_an_id() {
    let cat = al_builtins::catalog::Catalog::shared();
    for s in &cat.signatures {
        let name = cat.id(s);
        let id = BuiltinId::of(&name).unwrap_or_else(|| panic!("{name} has no BuiltinId"));
        assert_eq!(id.name(), name);
    }
    for name in al_builtins::native::registered_ids() {
        assert!(BuiltinId::of(name).is_some(), "{name} has no BuiltinId");
    }
}

#[test]
fn names_are_sorted_unique_and_round_trip_through_both_serialized_forms() {
    let all: Vec<BuiltinId> = BuiltinId::all().collect();
    assert!(all.windows(2).all(|w| w[0].name() < w[1].name()));
    let id = BuiltinId::of("Record.Insert#2").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"Record.Insert#2\"");
    assert_eq!(serde_json::from_str::<BuiltinId>("\"Record.Insert#2\"").unwrap(), id);
    assert!(serde_json::from_str::<BuiltinId>("\"No.Such#1\"").is_err());
}

#[test]
fn the_array_registry_agrees_with_the_name_lookup() {
    for id in BuiltinId::all() {
        assert_eq!(
            al_builtins::native::by_id(id).map(|f| f as usize),
            al_builtins::native::lookup(id.name()).map(|f| f as usize)
        );
    }
    assert!(al_builtins::builtin_id::ids::RECORD_INSERT_2.name() == "Record.Insert#2");
}
