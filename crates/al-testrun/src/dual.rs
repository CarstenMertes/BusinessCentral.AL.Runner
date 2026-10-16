//! The dual run (ADR-034 decision 4; docs/reviews/2026-10-bytecode-design.md, "Dual run").
//!
//! With `RunOptions::dual_run` each test codeunit runs twice on equal fresh sessions: once on the tree walker
//! (`ExecMode::Tree`, the oracle) and once on the executable form (`ExecMode::Bytecode`). The two are compared
//! test by test: the outcome (pass, fail with its text, not run with its reason), the call stack of an AL error
//! (text, so the frames and lines), the unprobed assumptions the test ran through, and the data the store holds when
//! the test has ended. The first difference of each test is reported; a difference fails the run.
//!
//! While the executable form covers nothing, the second run is the tree walker too, which is how the harness is
//! tested: two runs of one interpreter must show no difference.

use crate::Outcome;
use al_builtins::assumptions::Assumption;

/// One thing in which the two runs of a test differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DualDiff {
    pub codeunit: u32,
    pub codeunit_name: String,
    /// The test; empty for a difference of the whole codeunit run (one side panicked).
    pub test: String,
    /// `outcome`, `error stack`, `assumptions`, `state` or `panic`.
    pub what: &'static str,
    /// What the tree walker gave, and what the executable form gave.
    pub tree: String,
    pub bytecode: String,
}

impl std::fmt::Display for DualDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let test = if self.test.is_empty() {
            "(whole codeunit)"
        } else {
            &self.test
        };
        write!(
            f,
            "dual run: {} {} ({}): {}: tree walker {:?}, bytecode {:?}",
            self.codeunit, self.codeunit_name, test, self.what, self.tree, self.bytecode
        )
    }
}

/// What a codeunit run left, in the parts the comparison reads.
pub(crate) struct Side<'a> {
    pub done: &'a [(String, Outcome)],
    pub stacks: &'a [(String, String)],
    pub hits: &'a [(String, Vec<Assumption>)],
    pub digests: &'a [(String, Vec<(u32, u64)>)],
}

/// Whether the runner stopped the test at its CPU time limit (`--test-timeout-secs`).
fn stopped_by_limit(o: &Outcome) -> bool {
    matches!(o, Outcome::NotRun(m) if m.contains("time limit of") && m.contains("CPU time"))
}

/// The differences between two runs of one test codeunit; the first of each test only.
pub(crate) fn compare(
    codeunit: u32,
    name: &str,
    tree: &Side,
    other: &Side,
    table_name: &dyn Fn(u32) -> String,
) -> Vec<DualDiff> {
    let mut out = Vec::new();
    let lookup = |list: &[(String, String)], t: &str| list.iter().find(|(n, _)| n == t).map(|(_, s)| s.clone());
    let marks = |list: &[(String, Vec<Assumption>)], t: &str| {
        list.iter()
            .find(|(n, _)| n == t)
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    };
    let digest =
        |list: &[(String, Vec<(u32, u64)>)], t: &str| list.iter().find(|(n, _)| n == t).map(|(_, d)| d.clone());
    let mut tests: Vec<&String> = tree.done.iter().map(|(t, _)| t).collect();
    for (t, _) in other.done {
        if !tests.contains(&t) {
            tests.push(t);
        }
    }
    // The tests of a codeunit share one session, whose row version and system time stamps count on from test to
    // test. Where the CPU time limit stops a test depends on the speed of the machine, so from the first test either
    // run had stopped the counters, and with them the rows' system fields, differ: the state is not compared after it.
    let mut limit_seen = false;
    for t in tests {
        let o_tree = tree.done.iter().find(|(n, _)| n == t).map(|(_, o)| o);
        let o_other = other.done.iter().find(|(n, _)| n == t).map(|(_, o)| o);
        limit_seen |= o_tree.is_some_and(stopped_by_limit) || o_other.is_some_and(stopped_by_limit);
        let mut diff = |what: &'static str, a: String, b: String| {
            out.push(DualDiff {
                codeunit,
                codeunit_name: name.to_string(),
                test: t.clone(),
                what,
                tree: a,
                bytecode: b,
            })
        };
        if o_tree != o_other {
            diff("outcome", format!("{o_tree:?}"), format!("{o_other:?}"));
        } else if o_tree.is_some_and(stopped_by_limit) {
            // Where the CPU time limit stops a test depends on the speed of the machine: the data and the
            // assumptions of two stopped runs are not comparable.
        } else if lookup(tree.stacks, t) != lookup(other.stacks, t) {
            diff(
                "error stack",
                lookup(tree.stacks, t).unwrap_or_default(),
                lookup(other.stacks, t).unwrap_or_default(),
            );
        } else if marks(tree.hits, t) != marks(other.hits, t) {
            diff(
                "assumptions",
                format!("{:?}", marks(tree.hits, t)),
                format!("{:?}", marks(other.hits, t)),
            );
        } else if !limit_seen && digest(tree.digests, t) != digest(other.digests, t) {
            // The first table (object index) whose rows differ, or that only one run holds.
            let (a, b) = (
                digest(tree.digests, t).unwrap_or_default(),
                digest(other.digests, t).unwrap_or_default(),
            );
            let first = a
                .iter()
                .map(|(o, _)| *o)
                .chain(b.iter().map(|(o, _)| *o))
                .filter(|o| a.iter().find(|(x, _)| x == o) != b.iter().find(|(x, _)| x == o))
                .min();
            diff(
                "state",
                format!(
                    "table {} (object {}) rows hash {:?}",
                    first.map_or_else(String::new, table_name),
                    first.unwrap_or(0),
                    a.iter().find(|(o, _)| Some(*o) == first).map(|(_, h)| h)
                ),
                format!(
                    "table {} (object {}) rows hash {:?}",
                    first.map_or_else(String::new, table_name),
                    first.unwrap_or(0),
                    b.iter().find(|(o, _)| Some(*o) == first).map(|(_, h)| h)
                ),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_runs_stopped_by_the_cpu_time_limit_are_not_compared_beyond_the_outcome() {
        // Where a test is stopped depends on the speed of the machine, so the data it leaves and the assumptions it
        // reached are not the same twice; only that both were stopped is.
        let stopped = vec![
            (
                "T".to_string(),
                Outcome::NotRun(
                    "the test used more than the time limit of 20 s of CPU time and was stopped (--test-timeout-secs)"
                        .into(),
                ),
            ),
            // A later test of the codeunit starts from the counters the stopped one left.
            ("U".to_string(), Outcome::Pass),
        ];
        let a = vec![
            ("T".to_string(), vec![(1u32, 1u64)]),
            ("U".to_string(), vec![(1u32, 1u64)]),
        ];
        let b = vec![
            ("T".to_string(), vec![(1u32, 2u64)]),
            ("U".to_string(), vec![(1u32, 2u64)]),
        ];
        let d = compare(
            7,
            "C",
            &side(&stopped, &[], &[], &a),
            &side(&stopped, &[], &[], &b),
            &|o| o.to_string(),
        );
        assert!(d.is_empty(), "{d:?}");
        // One stopped and one not is still an outcome difference.
        let passed = vec![("T".to_string(), Outcome::Pass), ("U".to_string(), Outcome::Pass)];
        let d = compare(
            7,
            "C",
            &side(&stopped, &[], &[], &a),
            &side(&passed, &[], &[], &a),
            &|o| o.to_string(),
        );
        assert_eq!((d.len(), d[0].what), (1, "outcome"));
    }

    fn side<'a>(
        done: &'a [(String, Outcome)],
        stacks: &'a [(String, String)],
        hits: &'a [(String, Vec<Assumption>)],
        digests: &'a [(String, Vec<(u32, u64)>)],
    ) -> Side<'a> {
        Side {
            done,
            stacks,
            hits,
            digests,
        }
    }

    #[test]
    fn equal_runs_show_nothing_and_the_first_difference_of_a_test_is_named() {
        let pass = vec![
            ("A".to_string(), Outcome::Pass),
            ("B".to_string(), Outcome::Fail("x".into())),
        ];
        let digests = vec![
            ("A".to_string(), vec![(1u32, 1u64)]),
            ("B".to_string(), vec![(1u32, 2u64)]),
        ];
        let same = compare(
            7,
            "C",
            &side(&pass, &[], &[], &digests),
            &side(&pass, &[], &[], &digests),
            &|o| o.to_string(),
        );
        assert!(same.is_empty());
        let other_outcome = vec![
            ("A".to_string(), Outcome::Pass),
            ("B".to_string(), Outcome::Fail("y".into())),
        ];
        let d = compare(
            7,
            "C",
            &side(&pass, &[], &[], &digests),
            &side(&other_outcome, &[], &[], &digests),
            &|o| o.to_string(),
        );
        assert_eq!((d.len(), d[0].test.as_str(), d[0].what), (1, "B", "outcome"));
        let other_digests = vec![
            ("A".to_string(), vec![(1u32, 1u64)]),
            ("B".to_string(), vec![(1u32, 3u64)]),
        ];
        let d = compare(
            7,
            "C",
            &side(&pass, &[], &[], &digests),
            &side(&pass, &[], &[], &other_digests),
            &|o| o.to_string(),
        );
        assert_eq!((d.len(), d[0].what), (1, "state"));
        assert!(d[0].to_string().contains("tree walker") && d[0].to_string().contains("7 C (B)"));
    }
}
