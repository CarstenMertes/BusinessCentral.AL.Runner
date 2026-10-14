//! Reading a run: the ranking of refusal reasons (`--top-refusals`) and the comparison with the
//! committed baseline (`--baseline-diff`). Pure functions over the run's outcomes.

use crate::oracle::{CaseId, OracleCase};
use crate::report::Outcome;
use std::collections::{BTreeMap, BTreeSet};

pub use al_testrun::refusals::{Refusal, clip, normalise_reason, render_top};

/// The not-run cases of `cases`, grouped by normalised reason, most frequent first (ties by
/// reason). Cases real BC fails are not counted. `in_scope` narrows to an area.
pub fn top_refusals(
    cases: &[OracleCase],
    in_scope: &dyn Fn(&CaseId) -> bool,
    outcome: &dyn Fn(&CaseId) -> Outcome,
) -> Vec<Refusal> {
    al_testrun::refusals::rank(
        cases
            .iter()
            .filter(|c| !c.bc_failed && in_scope(&c.id))
            .filter_map(|c| match outcome(&c.id) {
                Outcome::NotRun(reason) => Some((c.id.clone(), reason)),
                _ => None,
            }),
    )
}

/// The run against the baseline file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BaselineDiff {
    /// Pass now, not in the baseline.
    pub newly_passing: Vec<CaseId>,
    /// Ran and failed, not in the baseline.
    pub failing: Vec<(CaseId, String)>,
    /// In the baseline, no longer passing, with what they do now.
    pub regressed: Vec<(CaseId, String)>,
}

pub fn baseline_diff(
    baseline: &BTreeSet<CaseId>,
    cases: &[OracleCase],
    outcome: &dyn Fn(&CaseId) -> Outcome,
) -> BaselineDiff {
    let mut d = BaselineDiff::default();
    for c in cases.iter().filter(|c| !c.bc_failed) {
        let o = outcome(&c.id);
        let known = baseline.contains(&c.id);
        match (&o, known) {
            (Outcome::Pass, false) => d.newly_passing.push(c.id.clone()),
            (Outcome::Fail(why), false) => d.failing.push((c.id.clone(), format!("fail: {}", clip(why, 120)))),
            (Outcome::Fail(why), true) => d.regressed.push((c.id.clone(), format!("fail: {}", clip(why, 120)))),
            (Outcome::NotRun(why), true) => d.regressed.push((c.id.clone(), format!("not run: {}", clip(why, 120)))),
            _ => {}
        }
    }
    // A baseline case the oracle no longer lists cannot pass either.
    let listed: BTreeSet<&CaseId> = cases.iter().map(|c| &c.id).collect();
    for b in baseline.iter().filter(|b| !listed.contains(b)) {
        if !matches!(outcome(b), Outcome::Pass) {
            d.regressed.push((b.clone(), "not in the oracle".into()));
        }
    }
    d
}

/// The comparison as text, each list grouped by area.
pub fn render_diff(label: &str, d: &BaselineDiff, area_of: &dyn Fn(u32) -> Option<String>) -> String {
    let mut s = format!(
        "Baseline diff, {label}: {} newly pass, {} fail and are not in the baseline, {} baseline cases no longer pass\n",
        d.newly_passing.len(),
        d.failing.len(),
        d.regressed.len()
    );
    let section = |s: &mut String, title: &str, items: Vec<(&CaseId, &str)>| {
        if items.is_empty() {
            return;
        }
        s.push_str(&format!("\n{title} ({}):\n", items.len()));
        let mut by_area: BTreeMap<String, Vec<(&CaseId, &str)>> = BTreeMap::new();
        for (c, why) in items {
            by_area
                .entry(area_of(c.codeunit).unwrap_or_else(|| "(no area)".into()))
                .or_default()
                .push((c, why));
        }
        for (area, list) in by_area {
            s.push_str(&format!("  {area} ({}):\n", list.len()));
            for (c, why) in list {
                if why.is_empty() {
                    s.push_str(&format!("    {} {}\n", c.codeunit, c.name));
                } else {
                    s.push_str(&format!("    {} {}  [{why}]\n", c.codeunit, c.name));
                }
            }
        }
    };
    section(
        &mut s,
        "Newly passing, add to the baseline",
        d.newly_passing.iter().map(|c| (c, "")).collect(),
    );
    section(
        &mut s,
        "Run and fail, not in the baseline",
        d.failing.iter().map(|(c, w)| (c, w.as_str())).collect(),
    );
    section(
        &mut s,
        "In the baseline but no longer passing (regressions)",
        d.regressed.iter().map(|(c, w)| (c, w.as_str())).collect(),
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(cu: u32, n: &str) -> OracleCase {
        OracleCase {
            id: CaseId {
                codeunit: cu,
                name: n.into(),
            },
            bc_failed: false,
        }
    }

    #[test]
    fn top_refusals_rank_by_count_and_keep_the_first_example() {
        let cases = [case(2, "B"), case(1, "A"), case(3, "C"), case(4, "D")];
        let out = |id: &CaseId| match id.codeunit {
            1 | 2 => Outcome::NotRun(format!("X.Y: builtin Foo#{} is not implemented", id.codeunit)),
            3 => Outcome::NotRun("something else".into()),
            _ => Outcome::Pass,
        };
        let top = top_refusals(&cases, &|_| true, &out);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].key, "builtin Foo#N is not implemented");
        assert_eq!(top[0].count, 2);
        assert_eq!(top[0].example.codeunit, 1);
        assert_eq!(top[1].count, 1);
        let text = render_top("BC 1", &top, 1);
        assert!(
            text.contains("3 not run, 2 distinct reasons") && text.contains("e.g. 1 A"),
            "{text}"
        );
        assert!(text.contains("1 more reasons"), "{text}");
    }

    #[test]
    fn the_diff_separates_new_passes_new_failures_and_regressions() {
        let cases = [
            case(1, "NewPass"),
            case(1, "NewFail"),
            case(2, "Held"),
            case(2, "Lost"),
            case(2, "Gone"),
        ];
        let baseline: BTreeSet<CaseId> = ["Held", "Lost"]
            .iter()
            .map(|n| CaseId {
                codeunit: 2,
                name: (*n).into(),
            })
            .chain([CaseId {
                codeunit: 9,
                name: "Removed".into(),
            }])
            .collect();
        let out = |id: &CaseId| match id.name.as_str() {
            "NewPass" | "Held" => Outcome::Pass,
            "NewFail" => Outcome::Fail("boom".into()),
            "Lost" => Outcome::NotRun("refused".into()),
            _ => Outcome::NotRun("n/a".into()),
        };
        let d = baseline_diff(&baseline, &cases, &out);
        assert_eq!(d.newly_passing.len(), 1);
        assert_eq!(d.failing.len(), 1);
        let lost: Vec<_> = d.regressed.iter().map(|(c, _)| c.name.as_str()).collect();
        assert_eq!(lost, ["Lost", "Removed"]);
        let text = render_diff("BC 1", &d, &|cu| Some(if cu == 1 { "a".into() } else { "b".into() }));
        assert!(
            text.contains("1 newly pass") && text.contains("  a (1):\n    1 NewPass"),
            "{text}"
        );
    }
}
