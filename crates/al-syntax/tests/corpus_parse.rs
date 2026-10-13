//! Milestone 1, front end: every corpus file parses with zero errors and the
//! tree's text is the file, byte for byte.

use std::path::{Path, PathBuf};

fn corpus_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests");
    assert!(
        root.join("al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                out.push(p);
            }
        }
    }
    out.sort();
    assert!(out.len() > 1300, "expected the whole corpus, found {} files", out.len());
    out
}

#[test]
fn every_corpus_file_parses_without_errors_and_round_trips() {
    let syms = ["BC27PLUS".to_string()].into();
    let mut failures = Vec::new();
    let files = corpus_files();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let parse = al_syntax::parse(&src, &syms);
        if parse.syntax().text() != src.as_str() {
            failures.push(format!("{}: tree text differs from source", f.display()));
        }
        if let Some(e) = parse.errors.first() {
            let line = src[..e.offset.min(src.len())].matches('\n').count() + 1;
            failures.push(format!(
                "{}:{line}: {} ({} error(s))",
                f.display(),
                e.message,
                parse.errors.len()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files failed:\n{}",
        failures.len(),
        files.len(),
        failures.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Guards against a grammar that "passes" by swallowing code into raw
/// property values or section arguments: every `procedure`/`trigger` keyword
/// must become a routine node, every `begin` a block, every `[Test]` an attribute.
#[test]
fn corpus_code_is_parsed_structurally_not_swallowed() {
    use al_syntax::SyntaxKind::*;
    let syms = ["BC27PLUS".to_string()].into();
    let (mut kw_routines, mut kw_begins, mut nodes_routines, mut nodes_blocks, mut test_attrs) = (0, 0, 0, 0, 0);
    for f in corpus_files() {
        let src = std::fs::read_to_string(&f).unwrap();
        let tree = al_syntax::parse(&src, &syms).syntax();
        for t in tree.descendants_with_tokens().filter_map(|e| e.into_token()) {
            if t.kind() == Ident {
                let w = t.text().to_ascii_lowercase();
                kw_routines += (w == "procedure" || w == "trigger") as usize;
                kw_begins += (w == "begin") as usize;
            }
        }
        for n in tree.descendants() {
            match n.kind() {
                Procedure | Trigger => nodes_routines += 1,
                Block => nodes_blocks += 1,
                Attribute if n.text().to_string().eq_ignore_ascii_case("[Test]") => test_attrs += 1,
                _ => {}
            }
        }
    }
    assert_eq!(
        nodes_routines, kw_routines,
        "routine nodes vs procedure/trigger keywords"
    );
    assert_eq!(nodes_blocks, kw_begins, "block nodes vs begin keywords");
    assert!(test_attrs > 4800, "only {test_attrs} [Test] attributes parsed");
}
