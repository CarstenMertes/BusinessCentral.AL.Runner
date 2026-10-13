//! `catalog/signatures.txt` must be exactly what the pinned corpus's docs
//! scrape generates. Regenerate with `UPDATE_GOLDEN=1 cargo test -p al-builtins`.

use std::path::Path;

#[test]
fn signatures_file_is_up_to_date_with_the_pinned_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let json = std::fs::read_to_string(root.join("../../corpus/scripts/al-surface.json"))
        .expect("corpus submodule not checked out: run `git submodule update --init`");
    let lines = al_builtins::catalog::generate(&json).unwrap_or_else(|e| panic!("unparseable syntax lines:\n{e}"));
    let generated = format!(
        "# GENERATED from corpus/scripts/al-surface.json by al-builtins (ADR-008). Do not edit; use overrides.txt.\n{}\n",
        lines.join("\n")
    );
    let path = root.join("catalog/signatures.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap_or_default(),
        generated,
        "stale; rerun with UPDATE_GOLDEN=1"
    );
}
