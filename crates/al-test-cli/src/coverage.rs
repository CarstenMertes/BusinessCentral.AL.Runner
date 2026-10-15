//! The corpus coverage map: which builtin methods and overloads of the catalog some corpus test
//! calls (`--coverage-map`, `docs/corpus-coverage.md`).
//!
//! The checker resolves every call of the corpus's own files; an overload counts as pinned only when
//! the checker picked that exact overload (`Res::Builtin(id)`), and the map names one call site
//! (the first in file and line order, so the output does not depend on thread timing). The point is
//! to see, per type, what a probe topic still has to cover (owner decision 2026-10-15: close whole
//! topics, not one blocker at a time).

use al_builtins::catalog::Catalog;
use al_sema::Workspace;
use al_sema::check::{CheckResult, Res};
use std::collections::BTreeMap;

/// One resolved builtin call in a file of the corpus.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Call {
    pub id: String,
    pub file: String,
    pub line: usize,
}

/// The builtin calls the checker resolved in the files of the workspace apps (not the dependency
/// apps). One entry per (overload, file, line).
pub fn collect_calls(ws: &Workspace, checked: &CheckResult) -> Vec<Call> {
    let mut lines: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut out = std::collections::BTreeSet::new();
    for ((file, start, _end), res) in checked.resolutions.iter() {
        let Res::Builtin(id) = res else { continue };
        let f = &ws.files[*file];
        if ws.kinds[f.app] != al_sema::AppKind::Workspace {
            continue;
        }
        // Offsets of the line starts, once per file.
        let starts = lines.entry(*file).or_insert_with(|| {
            let mut v = vec![0usize];
            v.extend(ws.text(*file).match_indices('\n').map(|(i, _)| i + 1));
            v
        });
        let line = starts.partition_point(|&s| s <= *start);
        out.insert(Call {
            id: id.clone(),
            file: f.path.clone(),
            line,
        });
    }
    out.into_iter().collect()
}

/// The map of the pinned corpus under `root` (the repository root): the corpus's `al-language` app
/// and its fixture, with the dependency apps of the corpus's `.alpackages` so that calls on Microsoft's
/// types resolve, checked with the preprocessor symbol of BC 27 and the whole catalog.
pub fn corpus_coverage(root: &std::path::Path, corpus_commit: &str) -> Result<Coverage, String> {
    let tests = root.join("corpus/tests");
    let fixture = tests.join("al-language-internals-fixture");
    let mut apps = vec![
        crate::runtime::load_app(&fixture)?,
        crate::runtime::load_app(&tests.join("al-language"))?,
    ];
    let mut kinds = vec![al_sema::AppKind::Workspace; apps.len()];
    let dependencies = crate::deps::load(&tests.join("al-language"), &[fixture], &[])?;
    kinds.extend(dependencies.apps.iter().map(|_| al_sema::AppKind::Dependency));
    apps.extend(dependencies.apps);
    let ws = Workspace::load_apps(apps, kinds, &al_version::corpus_symbols(al_version::MIN_BC_MAJOR));
    let members = al_sema::members::build(&ws);
    let catalog = Catalog::load();
    let checked = al_sema::check::check(&ws, &members, &catalog);
    Ok(build(&catalog, &collect_calls(&ws, &checked), corpus_commit))
}

/// One overload of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overload {
    pub id: String,
    /// The catalog line, as `signatures.txt` has it.
    pub signature: String,
    /// One call in the corpus that the checker resolved to this overload.
    pub site: Option<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCoverage {
    pub name: String,
    pub overloads: Vec<Overload>,
}

impl TypeCoverage {
    pub fn pinned(&self) -> usize {
        self.overloads.iter().filter(|o| o.site.is_some()).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub corpus_commit: String,
    pub types: Vec<TypeCoverage>,
}

impl Coverage {
    pub fn total(&self) -> (usize, usize) {
        (
            self.types.iter().map(TypeCoverage::pinned).sum(),
            self.types.iter().map(|t| t.overloads.len()).sum(),
        )
    }
}

/// The map of `catalog` against `calls`. Types and overloads are in catalog order of name and id.
pub fn build(catalog: &Catalog, calls: &[Call], corpus_commit: &str) -> Coverage {
    let mut first: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
    for c in calls {
        let site = (c.file.as_str(), c.line);
        let e = first.entry(c.id.as_str()).or_insert(site);
        if site < *e {
            *e = site;
        }
    }
    let mut by_type: BTreeMap<String, Vec<Overload>> = BTreeMap::new();
    for sig in &catalog.signatures {
        let id = catalog.id(sig);
        let site = first.get(id.as_str()).map(|(f, l)| (f.to_string(), *l));
        by_type.entry(sig.receiver.clone()).or_default().push(Overload {
            signature: sig.to_line(),
            id,
            site,
        });
    }
    Coverage {
        corpus_commit: corpus_commit.to_string(),
        types: by_type
            .into_iter()
            .map(|(name, overloads)| TypeCoverage { name, overloads })
            .collect(),
    }
}

/// The map as Markdown: a summary table, then the overloads no corpus test calls, per type.
pub fn to_markdown(c: &Coverage) -> String {
    let (pinned, all) = c.total();
    let mut s = String::new();
    s.push_str("# Corpus coverage of the builtin catalog\n\n");
    s.push_str("Generated by `al-test-cli --coverage-map` (the test `corpus_coverage_map_is_current` checks it). Do not edit.\n\n");
    s.push_str(&format!(
        "Corpus commit `{}`. An overload is pinned when the checker resolves at least one call of the corpus's own files to exactly that overload.\n\n",
        c.corpus_commit
    ));
    s.push_str(&format!("**Total: pinned {pinned} of {all} overloads.**\n\n"));
    s.push_str("| Type | Pinned | Overloads | Unpinned |\n|---|---:|---:|---:|\n");
    for t in &c.types {
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            t.name,
            t.pinned(),
            t.overloads.len(),
            t.overloads.len() - t.pinned()
        ));
    }
    s.push_str("\n## Unpinned overloads\n");
    for t in c.types.iter().filter(|t| t.pinned() < t.overloads.len()) {
        s.push_str(&format!(
            "\n### {} (pinned {} of {})\n\n",
            t.name,
            t.pinned(),
            t.overloads.len()
        ));
        for o in t.overloads.iter().filter(|o| o.site.is_none()) {
            s.push_str(&format!("- `{}` `{}`\n", o.id, o.signature));
        }
    }
    s.push_str("\n## Pinned overloads\n");
    for t in c.types.iter().filter(|t| t.pinned() > 0) {
        s.push_str(&format!("\n### {}\n\n", t.name));
        for o in t.overloads.iter().filter(|o| o.site.is_some()) {
            let (f, l) = o.site.as_ref().expect("filtered");
            s.push_str(&format!("- `{}` `{}`: {f}:{l}\n", o.id, o.signature));
        }
    }
    s
}

/// The map as JSON, for tools.
pub fn to_json(c: &Coverage) -> String {
    let (pinned, all) = c.total();
    let types: Vec<serde_json::Value> = c
        .types
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": t.name,
                "pinned": t.pinned(),
                "overloads": t.overloads.len(),
                "methods": t.overloads.iter().map(|o| serde_json::json!({
                    "id": o.id,
                    "signature": o.signature,
                    "pinned": o.site.is_some(),
                    "site": o.site.as_ref().map(|(f, l)| format!("{f}:{l}")),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&serde_json::json!({
        "corpus_commit": c.corpus_commit,
        "pinned": pinned,
        "overloads": all,
        "types": types,
    }))
    .expect("json");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_sema::workspace::AppSource;

    /// A fixture "corpus" of one file: calls on a Text, a Label-free global and a List.
    const FIXTURE: &str = "codeunit 50100 C
{
    procedure P()
    var
        T: Text;
        L: List of [Text];
    begin
        if T.Contains('x') then
            T.Split(',');
        L.Add('a');
        Message('hi');
        T.Contains('y');
    end;
}
";

    fn map() -> Coverage {
        let ws = Workspace::load(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("fixture.al".into(), FIXTURE.into())],
            }],
            &Default::default(),
        );
        let members = al_sema::members::build(&ws);
        let catalog = Catalog::load();
        let checked = al_sema::check::check(&ws, &members, &catalog);
        let calls = collect_calls(&ws, &checked);
        build(&catalog, &calls, "abc123")
    }

    fn find<'a>(c: &'a Coverage, ty: &str, name: &str) -> Vec<&'a Overload> {
        c.types
            .iter()
            .filter(|t| t.name == ty)
            .flat_map(|t| &t.overloads)
            .filter(|o| o.signature.starts_with(&format!("{ty}.{name} ")))
            .collect()
    }

    #[test]
    fn an_overload_is_pinned_when_a_call_resolves_to_it_and_names_the_first_site() {
        let c = map();
        let contains = find(&c, "Text", "Contains");
        assert_eq!(contains.len(), 1);
        // Lines 8 and 12 call it; the first one is named.
        assert_eq!(contains[0].site, Some(("fixture.al".to_string(), 8)));
        let split = find(&c, "Text", "Split");
        assert!(split.len() >= 2, "Split has several overloads");
        assert_eq!(
            split.iter().filter(|o| o.site.is_some()).count(),
            1,
            "only the called one"
        );
        assert_eq!(find(&c, "List", "Add")[0].site, Some(("fixture.al".to_string(), 10)));
        assert!(find(&c, "Dialog", "Message").iter().any(|o| o.site.is_some()));
        assert!(find(&c, "Text", "StartsWith")[0].site.is_none());
    }

    #[test]
    fn the_totals_count_pinned_against_all_overloads_and_the_output_names_both() {
        let c = map();
        let (pinned, all) = c.total();
        assert!(pinned >= 4 && pinned < all, "{pinned} of {all}");
        let md = to_markdown(&c);
        assert!(md.contains("Corpus commit `abc123`"));
        assert!(md.contains(&format!("**Total: pinned {pinned} of {all} overloads.**")));
        assert!(md.contains("| Text | "), "{}", &md[..400]);
        assert!(md.contains("### Text (pinned"), "unpinned section per type");
        assert!(md.contains("fixture.al:8"), "a pinned overload names its call");
        let json: serde_json::Value = serde_json::from_str(&to_json(&c)).unwrap();
        assert_eq!(json["pinned"], pinned);
        assert_eq!(json["overloads"], all);
        assert_eq!(json["corpus_commit"], "abc123");
    }

    #[test]
    fn calls_in_a_dependency_app_do_not_count() {
        let ws = Workspace::load_apps(
            vec![
                AppSource {
                    id: "d".into(),
                    name: "d".into(),
                    dependencies: vec![],
                    files: vec![("dep.al".into(), FIXTURE.into())],
                },
                AppSource {
                    id: "w".into(),
                    name: "w".into(),
                    dependencies: vec!["d".into()],
                    files: vec![("own.al".into(), "codeunit 60000 W { procedure Q() begin end; }".into())],
                },
            ],
            vec![al_sema::AppKind::Dependency, al_sema::AppKind::Workspace],
            &Default::default(),
        );
        let members = al_sema::members::build(&ws);
        let checked = al_sema::check::check(&ws, &members, &Catalog::load());
        assert!(collect_calls(&ws, &checked).iter().all(|c| c.file != "dep.al"));
    }
}
