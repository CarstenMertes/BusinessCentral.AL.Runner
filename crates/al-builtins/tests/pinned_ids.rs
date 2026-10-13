//! Overload ids are positions (ADR-008, ADR-015 3.3), and registered implementations are keyed
//! by them. A regenerated catalog may add ids; it may not renumber one. The pin file lists
//! `id<TAB>catalog line`; extend it by appending the new lines, never by editing old ones.

use al_builtins::catalog::Catalog;

#[test]
fn every_pinned_overload_id_still_names_the_same_overload() {
    let pinned = include_str!("pinned_ids.tsv");
    let cat = Catalog::load();
    let mut moved = Vec::new();
    for l in pinned.lines() {
        let (id, line) = l.split_once('\t').expect("id<TAB>line");
        match cat.by_id(id) {
            Some(s) if s.to_line() == line => {}
            Some(s) => moved.push(format!("{id}: was `{line}`, now `{}`", s.to_line())),
            None => moved.push(format!("{id}: was `{line}`, now gone")),
        }
    }
    assert!(moved.is_empty(), "overload ids changed:\n{}", moved.join("\n"));
}
