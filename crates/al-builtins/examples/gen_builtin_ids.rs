//! Writes `src/builtin_ids.rs` (ADR-032 step 2). Run after the catalog or a registry changes:
//! `cargo run -p al-builtins --example gen_builtin_ids`.
fn main() {
    let names = al_builtins::native::all_builtin_names();
    let src = al_builtins::builtin_id::render_table(&names);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_ids.rs");
    std::fs::write(&path, src).unwrap();
    println!("{} ids written to {}", names.len(), path.display());
}
