//! The corpus coverage map (issue 52): which builtin overloads the code of the counted apps calls, as the
//! checker resolved them, with one call site each.

use al_sema::{AppSource, Workspace};

fn ws(files: &[(&str, &str)]) -> Workspace {
    Workspace::load(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect(),
        }],
        &Default::default(),
    )
}

#[test]
fn an_overload_counts_only_when_the_checker_picked_it_and_names_one_call_site() {
    let src = "codeunit 50100 C\n{\n    procedure P()\n    var\n        T: Text;\n        B: Boolean;\n    begin\n        B := T.Contains('x');\n        T := Format(5);\n        T := Format(5, 0, '<Integer>');\n    end;\n}\n";
    let w = ws(&[("c.al", src)]);
    let members = al_sema::members::build(&w);
    let catalog = al_builtins::catalog::Catalog::load();
    let r = al_sema::check::check(&w, &members, &catalog);
    let map = al_sema::coverage::coverage(&w, &r, &catalog, &[0]);
    let text = map
        .types
        .iter()
        .find(|t| t.receiver == "Text")
        .expect("Text in the map");
    let contains = text
        .overloads
        .iter()
        .find(|o| o.id.starts_with("Text.Contains#"))
        .unwrap();
    assert_eq!(contains.call.as_deref(), Some("c.al:8"));
    // Two overloads of one method count each for itself.
    let system = map.types.iter().find(|t| t.receiver == "System").unwrap();
    let formats: Vec<_> = system
        .overloads
        .iter()
        .filter(|o| o.id.starts_with("System.Format#"))
        .collect();
    assert!(formats.len() >= 2, "{formats:?}");
    let pinned: Vec<_> = formats.iter().filter(|o| o.call.is_some()).collect();
    assert_eq!(pinned.len(), 2, "{formats:?}");
    assert!(pinned.iter().any(|o| o.call.as_deref() == Some("c.al:9")), "{pinned:?}");
    assert!(
        pinned.iter().any(|o| o.call.as_deref() == Some("c.al:10")),
        "{pinned:?}"
    );
    // An overload no code calls is listed without a call site.
    let trim = text.overloads.iter().find(|o| o.id.starts_with("Text.Trim#")).unwrap();
    assert_eq!(trim.call, None);
    assert_eq!(
        text.pinned(),
        text.overloads.iter().filter(|o| o.call.is_some()).count()
    );
    assert!(
        map.total() > 1000 && map.pinned() >= 3,
        "{} {}",
        map.total(),
        map.pinned()
    );
    // The Markdown form names the corpus commit and gives per-type counts.
    let md = map.markdown("abc1234");
    assert!(md.contains("abc1234"), "{md}");
    assert!(md.contains("| Text |"), "{md}");
}

#[test]
fn calls_in_apps_that_are_not_counted_do_not_pin() {
    let src = "codeunit 50100 C { procedure P() var T: Text; B: Boolean; begin B := T.Contains('x'); end; }";
    let w = ws(&[("c.al", src)]);
    let members = al_sema::members::build(&w);
    let catalog = al_builtins::catalog::Catalog::load();
    let r = al_sema::check::check(&w, &members, &catalog);
    let map = al_sema::coverage::coverage(&w, &r, &catalog, &[]);
    assert_eq!(map.pinned(), 0);
}
