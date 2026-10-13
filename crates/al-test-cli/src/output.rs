//! Renders a report as Markdown (CI step summary) and the runtime's outcomes as
//! JUnit in the same shape the corpus CI emits, so the two diff line for line.

use crate::oracle::{CaseId, OracleCase};
use crate::report::{Counts, Outcome, Report};

pub fn markdown(r: &Report) -> String {
    let mut s = String::from(
        "## Corpus pass rate\n\n| scope | total | pass | fail | not run | rate |\n|---|---:|---:|---:|---:|---:|\n",
    );
    row(&mut s, "**overall**", &r.overall);
    for (m, c) in &r.milestones {
        row(&mut s, m, c);
    }
    s.push_str("\n### By area\n\n| area | total | pass | fail | not run | rate |\n|---|---:|---:|---:|---:|---:|\n");
    for (a, c) in &r.areas {
        row(&mut s, a, c);
    }
    if r.excluded_bc_failed > 0 {
        s.push_str(&format!(
            "\n{} case(s) real BC did not pass are excluded.\n",
            r.excluded_bc_failed
        ));
    }
    s
}

fn row(s: &mut String, name: &str, c: &Counts) {
    s.push_str(&format!(
        "| {name} | {} | {} | {} | {} | {:.1}% |\n",
        c.total,
        c.pass,
        c.fail,
        c.not_run,
        c.rate() * 100.0
    ));
}

pub fn junit(oracle: &[OracleCase], outcome: &dyn Fn(&CaseId) -> Outcome) -> String {
    let mut by_cu: std::collections::BTreeMap<u32, Vec<(&CaseId, Outcome)>> = Default::default();
    for c in oracle.iter().filter(|c| !c.bc_failed) {
        by_cu.entry(c.id.codeunit).or_default().push((&c.id, outcome(&c.id)));
    }
    let all = by_cu.values().flatten();
    let total = all.clone().count();
    let failures = all.clone().filter(|(_, o)| matches!(o, Outcome::Fail(_))).count();
    let skipped = all.filter(|(_, o)| matches!(o, Outcome::NotRun(_))).count();
    let mut s = format!(
        "<?xml version='1.0' encoding='utf-8'?>\n<testsuites tests=\"{total}\" failures=\"{failures}\" skipped=\"{skipped}\">\n"
    );
    for (cu, cases) in &by_cu {
        s.push_str(&format!(
            "  <testsuite name=\"Codeunit {cu}\" tests=\"{}\">\n",
            cases.len()
        ));
        for (id, o) in cases {
            let head = format!("    <testcase classname=\"Codeunit {cu}\" name=\"{}\"", esc(&id.name));
            match o {
                Outcome::Pass => s.push_str(&format!("{head} />\n")),
                Outcome::Fail(m) => s.push_str(&format!("{head}><failure message=\"{}\" /></testcase>\n", esc(m))),
                Outcome::NotRun(m) => s.push_str(&format!("{head}><skipped message=\"{}\" /></testcase>\n", esc(m))),
            }
        }
        s.push_str("  </testsuite>\n");
    }
    s.push_str("</testsuites>\n");
    s
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::build;

    fn id(cu: u32, n: &str) -> CaseId {
        CaseId {
            codeunit: cu,
            name: n.into(),
        }
    }
    fn oracle() -> Vec<OracleCase> {
        vec![
            OracleCase {
                id: id(1, "a"),
                bc_failed: false,
            },
            OracleCase {
                id: id(1, "b<&>"),
                bc_failed: false,
            },
            OracleCase {
                id: id(2, "c"),
                bc_failed: false,
            },
        ]
    }
    fn outcome(c: &CaseId) -> Outcome {
        match c.name.as_str() {
            "a" => Outcome::Pass,
            "c" => Outcome::NotRun("unsupported builtin `Format`".into()),
            _ => Outcome::Fail("Assert.AreEqual failed. <1> & <2>".into()),
        }
    }
    fn areas(cu: u32) -> Option<String> {
        Some(if cu == 1 { "text" } else { "record" }.into())
    }

    #[test]
    fn markdown_has_overall_milestone_and_area_rows() {
        let md = markdown(&build(&oracle(), &areas, &outcome));
        assert!(md.contains("| **overall** | 3 | 1 | 1 | 1 | 33.3% |"), "{md}");
        assert!(md.contains("| M2 pure semantics | 2 | 1 | 1 | 0 | 50.0% |"), "{md}");
        assert!(md.contains("| record | 1 | 0 | 0 | 1 | 0.0% |"), "{md}");
    }

    #[test]
    fn junit_round_trips_through_the_oracle_parser() {
        let xml = junit(&oracle(), &outcome);
        let back = crate::oracle::parse_junit(&xml).unwrap();
        let ids: Vec<_> = back.iter().map(|c| (c.id.clone(), c.bc_failed)).collect();
        assert_eq!(
            ids,
            vec![(id(1, "a"), false), (id(1, "b<&>"), true), (id(2, "c"), true)]
        );
    }

    #[test]
    fn junit_marks_not_run_as_skipped_and_fail_as_failure() {
        let xml = junit(&oracle(), &outcome);
        assert!(
            xml.contains("<skipped message=\"unsupported builtin `Format`\""),
            "{xml}"
        );
        assert!(
            xml.contains("<failure message=\"Assert.AreEqual failed. &lt;1&gt; &amp; &lt;2&gt;\""),
            "{xml}"
        );
        assert!(
            xml.contains("<testsuites tests=\"3\" failures=\"1\" skipped=\"1\">"),
            "{xml}"
        );
    }
}
