//! Joins the oracle with the runtime's outcomes into the pass-rate report,
//! and enforces the ratchet: a case that passed before must still pass.

use crate::oracle::{CaseId, OracleCase};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// The runtime ran the case and it failed (assertion or runtime error).
    Fail(String),
    /// The runtime could not run the case: compile-time unsupported, or no runtime yet.
    NotRun(String),
}

/// Milestone groups from the original brief (docs/history/spike-brief.md), by area folder.
pub const MILESTONES: &[(&str, &[&str])] = &[
    (
        "M2 pure semantics",
        &["types", "text", "collections", "codeunit", "error-handling"],
    ),
    ("M3 record engine", &["record", "recordref", "fieldref"]),
    ("M4 stretch", &["json", "xml", "streams", "session"]),
];

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub total: usize,
    pub pass: usize,
    pub fail: usize,
    pub not_run: usize,
}

impl Counts {
    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.pass as f64 / self.total as f64
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub overall: Counts,
    pub areas: BTreeMap<String, Counts>,
    pub milestones: BTreeMap<String, Counts>,
    /// Cases real BC itself did not pass; excluded from every count.
    pub excluded_bc_failed: usize,
    /// Oracle cases whose codeunit is not in the corpus: the oracle and the
    /// corpus pin disagree, which makes the whole report untrustworthy.
    pub unmapped: Vec<String>,
}

pub fn build(
    oracle: &[OracleCase],
    area_of: &dyn Fn(u32) -> Option<String>,
    outcome: &dyn Fn(&CaseId) -> Outcome,
) -> Report {
    let mut r = Report {
        overall: Counts::default(),
        areas: BTreeMap::new(),
        milestones: MILESTONES
            .iter()
            .map(|(m, _)| (m.to_string(), Counts::default()))
            .collect(),
        excluded_bc_failed: 0,
        unmapped: Vec::new(),
    };
    for case in oracle {
        if case.bc_failed {
            r.excluded_bc_failed += 1;
            continue;
        }
        let Some(area) = area_of(case.id.codeunit) else {
            r.unmapped.push(format!("{} {}", case.id.codeunit, case.id.name));
            continue;
        };
        let o = outcome(&case.id);
        let milestone = MILESTONES.iter().find(|(_, areas)| areas.contains(&area.as_str()));
        let mut targets = vec![&mut r.overall];
        let area_counts = r.areas.entry(area.clone()).or_default();
        targets.push(area_counts);
        if let Some((m, _)) = milestone {
            targets.push(r.milestones.get_mut(*m).expect("milestone pre-seeded"));
        }
        for c in targets {
            c.total += 1;
            match o {
                Outcome::Pass => c.pass += 1,
                Outcome::Fail(_) => c.fail += 1,
                Outcome::NotRun(_) => c.not_run += 1,
            }
        }
    }
    r
}

/// Cases listed as passing in the baseline that do not pass now.
pub fn regressions(baseline: &BTreeSet<CaseId>, outcome: &dyn Fn(&CaseId) -> Outcome) -> Vec<CaseId> {
    baseline
        .iter()
        .filter(|c| outcome(c) != Outcome::Pass)
        .cloned()
        .collect()
}

/// Baseline file format: one `<codeunit> <procedure>` per line, `#` comments.
pub fn parse_baseline(text: &str) -> Result<BTreeSet<CaseId>, String> {
    let mut set = BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (cu, name) = line
            .split_once(' ')
            .ok_or_else(|| format!("line {}: expected `<codeunit> <procedure>`", n + 1))?;
        let codeunit = cu
            .parse()
            .map_err(|_| format!("line {}: `{cu}` is not a codeunit number", n + 1))?;
        set.insert(CaseId {
            codeunit,
            name: name.trim().to_string(),
        });
    }
    Ok(set)
}

pub fn format_baseline(cases: &BTreeSet<CaseId>) -> String {
    let mut s = String::from(
        "# Corpus cases the runtime passes. CI fails if any of these stops passing.\n# Regenerate with `al-test-cli --update-baseline`; never edit by hand to hide a regression.\n",
    );
    for c in cases {
        s.push_str(&format!("{} {}\n", c.codeunit, c.name));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(cu: u32, n: &str, bc_failed: bool) -> OracleCase {
        OracleCase {
            id: id(cu, n),
            bc_failed,
        }
    }
    fn id(cu: u32, n: &str) -> CaseId {
        CaseId {
            codeunit: cu,
            name: n.into(),
        }
    }
    fn areas(cu: u32) -> Option<String> {
        match cu {
            1 => Some("text".into()),
            2 => Some("record".into()),
            3 => Some("handlers".into()),
            _ => None,
        }
    }

    fn oracle() -> Vec<OracleCase> {
        vec![
            case(1, "a", false),
            case(1, "b", false),
            case(2, "c", false),
            case(3, "d", false),
            case(3, "e", true),
        ]
    }

    fn outcome(c: &CaseId) -> Outcome {
        match c.name.as_str() {
            "a" | "c" => Outcome::Pass,
            "b" => Outcome::Fail("x".into()),
            _ => Outcome::NotRun("no runtime".into()),
        }
    }

    #[test]
    fn counts_per_area_and_overall() {
        let r = build(&oracle(), &areas, &outcome);
        assert_eq!(
            r.overall,
            Counts {
                total: 4,
                pass: 2,
                fail: 1,
                not_run: 1
            }
        );
        assert_eq!(
            r.areas["text"],
            Counts {
                total: 2,
                pass: 1,
                fail: 1,
                not_run: 0
            }
        );
        assert_eq!(
            r.areas["record"],
            Counts {
                total: 1,
                pass: 1,
                fail: 0,
                not_run: 0
            }
        );
        assert_eq!(
            r.areas["handlers"],
            Counts {
                total: 1,
                pass: 0,
                fail: 0,
                not_run: 1
            }
        );
    }

    #[test]
    fn cases_bc_failed_are_excluded_not_counted() {
        let r = build(&oracle(), &areas, &outcome);
        assert_eq!(r.excluded_bc_failed, 1);
    }

    #[test]
    fn milestones_sum_their_areas() {
        let r = build(&oracle(), &areas, &outcome);
        assert_eq!(
            r.milestones["M2 pure semantics"],
            Counts {
                total: 2,
                pass: 1,
                fail: 1,
                not_run: 0
            }
        );
        assert_eq!(
            r.milestones["M3 record engine"],
            Counts {
                total: 1,
                pass: 1,
                fail: 0,
                not_run: 0
            }
        );
        assert_eq!(r.milestones["M4 stretch"], Counts::default());
    }

    #[test]
    fn oracle_cases_without_a_corpus_codeunit_are_reported_unmapped() {
        let mut o = oracle();
        o.push(case(99, "z", false));
        let r = build(&o, &areas, &outcome);
        assert_eq!(r.unmapped, vec!["99 z".to_string()]);
    }

    #[test]
    fn rate_is_pass_over_total() {
        assert_eq!(
            Counts {
                total: 4,
                pass: 1,
                fail: 0,
                not_run: 3
            }
            .rate(),
            0.25
        );
        assert_eq!(Counts::default().rate(), 0.0);
    }

    #[test]
    fn a_baseline_case_that_no_longer_passes_is_a_regression() {
        let base: BTreeSet<_> = [id(1, "a"), id(1, "b"), id(3, "d")].into();
        assert_eq!(regressions(&base, &outcome), vec![id(1, "b"), id(3, "d")]);
    }

    #[test]
    fn baseline_round_trips_and_skips_comments_and_blanks() {
        let text = "# passing cases\n\n60002 A_Works\n60001 Z\n";
        let set = parse_baseline(text).unwrap();
        assert_eq!(set, [id(60001, "Z"), id(60002, "A_Works")].into());
        assert_eq!(parse_baseline(&format_baseline(&set)).unwrap(), set);
    }

    #[test]
    fn baseline_rejects_a_line_without_a_codeunit_number() {
        assert!(parse_baseline("abc A\n").is_err());
        assert!(parse_baseline("60002\n").is_err());
    }
}
