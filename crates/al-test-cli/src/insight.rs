//! Reading a run: the ranking of refusal reasons (`--top-refusals`) and the comparison with the
//! committed baseline (`--baseline-diff`). Pure functions over the run's outcomes.

use crate::oracle::{CaseId, OracleCase};
use crate::report::Outcome;
use std::collections::{BTreeMap, BTreeSet};

/// One refusal reason with the cases that share it.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The reason with names and numbers folded (`normalise_reason`).
    pub key: String,
    pub count: usize,
    /// The first case (by codeunit, then name) with this reason, and its reason as raised.
    pub example: CaseId,
    pub example_reason: String,
}

/// Folds a refusal reason so that reasons that differ only in a name or a number group together:
/// the routine the reason was found in (`Object.Routine: `, `handler X: `, `OnRun: `) goes, quoted
/// names become `<name>` and numbers become `N`.
pub fn normalise_reason(raw: &str) -> String {
    let mut rest = raw.trim();
    for _ in 0..4 {
        let Some((head, tail)) = rest.split_once(": ") else {
            break;
        };
        if tail.trim().is_empty() || !is_site_prefix(head) {
            break;
        }
        rest = tail.trim_start();
    }
    fold_numbers(&fold_quotes(rest))
}

/// Whether `head` (the text before the first `: `) says where a reason was found, not what it is.
fn is_site_prefix(head: &str) -> bool {
    if head == "OnRun" {
        return true;
    }
    if head.starts_with("handler ") && !head.contains(['.', '`', '(']) {
        return true;
    }
    // A variable: `LayoutList`: platform table ...
    if head.len() > 2 && head.starts_with('`') && head.ends_with('`') && !head[1..head.len() - 1].contains('`') {
        return true;
    }
    // `Object Name.RoutineName`: an identifier after the last dot. DotNet and builtin texts that
    // name a member the same way are reasons, not sites.
    if head.starts_with("DotNet") || head.starts_with("builtin ") || head.contains(['`', '(', '\'']) {
        return false;
    }
    match head.rsplit_once('.') {
        Some((obj, routine)) => {
            !obj.is_empty()
                && !routine.is_empty()
                && routine.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !routine.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

/// `'x'`, `` `x` `` and `"x"` become `<name>`. An apostrophe inside a word is not a quote.
fn fold_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let opens = matches!(c, '\'' | '`' | '"') && (i == 0 || !chars[i - 1].is_alphanumeric());
        if opens {
            let close = (i + 1..chars.len())
                .find(|&j| chars[j] == c && chars.get(j + 1).is_none_or(|n| !n.is_alphanumeric()));
            if let Some(j) = close {
                out.push_str("<name>");
                i = j + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// A run of digits that is not part of a word (`#4`, `(61)`, `format 2`) becomes `N`.
fn fold_numbers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let word = |c: char| c.is_alphabetic() || c == '_';
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        let in_word = (i > 0 && word(chars[i - 1])) || (j < chars.len() && word(chars[j]));
        if in_word {
            out.extend(&chars[i..j]);
        } else {
            out.push('N');
        }
        i = j;
    }
    out
}

/// The not-run cases of `cases`, grouped by normalised reason, most frequent first (ties by
/// reason). Cases real BC fails are not counted. `in_scope` narrows to an area.
pub fn top_refusals(
    cases: &[OracleCase],
    in_scope: &dyn Fn(&CaseId) -> bool,
    outcome: &dyn Fn(&CaseId) -> Outcome,
) -> Vec<Refusal> {
    let mut groups: BTreeMap<String, Refusal> = BTreeMap::new();
    for c in cases.iter().filter(|c| !c.bc_failed && in_scope(&c.id)) {
        let Outcome::NotRun(reason) = outcome(&c.id) else {
            continue;
        };
        let key = normalise_reason(&reason);
        let g = groups.entry(key.clone()).or_insert_with(|| Refusal {
            key,
            count: 0,
            example: c.id.clone(),
            example_reason: reason.clone(),
        });
        g.count += 1;
        if c.id < g.example {
            g.example = c.id.clone();
            g.example_reason = reason;
        }
    }
    let mut v: Vec<Refusal> = groups.into_values().collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    v
}

/// The ranking as text: `limit` entries, then what was left out.
pub fn render_top(title: &str, list: &[Refusal], limit: usize) -> String {
    let total: usize = list.iter().map(|r| r.count).sum();
    let mut s = format!("Top refusals, {title}: {total} not run, {} distinct reasons\n", list.len());
    for r in list.iter().take(limit) {
        s.push_str(&format!("{:>6}  {}\n", r.count, r.key));
        let raw = if r.example_reason == r.key {
            String::new()
        } else {
            format!(": {}", clip(&r.example_reason, 200))
        };
        s.push_str(&format!("        e.g. {} {}{raw}\n", r.example.codeunit, r.example.name));
    }
    if list.len() > limit {
        let rest: usize = list[limit..].iter().map(|r| r.count).sum();
        s.push_str(&format!(
            "        ... {} more reasons, {rest} cases (--top-refusals {})\n",
            list.len() - limit,
            list.len()
        ));
    }
    s
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
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

    #[test]
    fn reasons_that_differ_in_a_name_or_a_number_fold_together() {
        let n = normalise_reason;
        assert_eq!(
            n("Test Query Object.Query_SaveAsCsv: builtin Query.SaveAsCsv#4 is not implemented"),
            "builtin Query.SaveAsCsv#N is not implemented"
        );
        assert_eq!(n("builtin Query.SaveAsCsv#2 is not implemented"), "builtin Query.SaveAsCsv#N is not implemented");
        assert_eq!(
            n("`LayoutList`: platform table 'Report Layout List' (2000000234) has no provider"),
            "platform table <name> (N) has no provider"
        );
        assert_eq!(
            n("handler OkGetDemandToReserve: Request Page Parameters Helper.GetFiltersForTable: foreach over a DotNet value is refused"),
            "foreach over a DotNet value is refused"
        );
        assert_eq!(n("OnRun: Some Codeunit.OnRun: division by zero (not pinned)"), "division by zero (not pinned)");
        assert_eq!(n("Format of a Duration with standard format 2 (not pinned)"), "Format of a Duration with standard format N (not pinned)");
    }

    #[test]
    fn a_dotnet_member_is_a_reason_not_a_site_and_a_name_with_digits_in_a_word_stays() {
        assert_eq!(
            normalise_reason("DotNet X.Y.IsSandbox: refused by policy: server class 'X.Y' is neither registered"),
            "DotNet X.Y.IsSandbox: refused by policy: server class <name> is neither registered"
        );
        assert_eq!(normalise_reason("Test01 does not fit"), "Test01 does not fit");
        assert_eq!(normalise_reason("it doesn't fit and don't try"), "it doesn't fit and don't try");
    }

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
        assert!(text.contains("3 not run, 2 distinct reasons") && text.contains("e.g. 1 A"), "{text}");
        assert!(text.contains("1 more reasons"), "{text}");
    }

    #[test]
    fn the_diff_separates_new_passes_new_failures_and_regressions() {
        let cases = [case(1, "NewPass"), case(1, "NewFail"), case(2, "Held"), case(2, "Lost"), case(2, "Gone")];
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
        assert!(text.contains("1 newly pass") && text.contains("  a (1):\n    1 NewPass"), "{text}");
    }
}
