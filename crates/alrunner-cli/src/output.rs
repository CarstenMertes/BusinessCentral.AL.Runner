//! What a run prints and writes: the summary, the lines per test, JUnit XML.

use al_testrun::Outcome;
use al_testrun::runtime::TestResult;

/// Counts of a set of results.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub pass: usize,
    pub fail: usize,
    pub not_run: usize,
}

impl Counts {
    pub fn of<'a>(results: impl IntoIterator<Item = &'a TestResult>) -> Counts {
        let mut c = Counts::default();
        for r in results {
            c.total += 1;
            match r.outcome {
                Outcome::Pass => c.pass += 1,
                Outcome::Fail(_) => c.fail += 1,
                Outcome::NotRun(_) => c.not_run += 1,
            }
        }
        c
    }
}

/// The exit code of a finished run (ADR-016 rule 2): 6 when nothing was selected, 1 when a test
/// failed, 7 when a test was not run (unless `allow_not_run`), else 0. Codes 2 and 3 are decided
/// before this point.
pub fn exit_code(c: &Counts, allow_not_run: bool) -> u8 {
    if c.total == 0 {
        6
    } else if c.fail > 0 {
        1
    } else if c.not_run > 0 && !allow_not_run {
        7
    } else {
        0
    }
}

/// The first line of a message, cut to `max` characters.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    al_testrun::refusals::clip(line, max)
}

/// `Tests: 12 total, 5 passed, 1 failed, 6 not run`, and the warning that goes with a run that
/// has no test data.
pub fn summary_line(c: &Counts) -> String {
    format!(
        "Tests: {} total, {} passed, {} failed, {} not run (no test data: failures are mostly missing setup data and are not trusted; the not-run counts and reasons are)",
        c.total, c.pass, c.fail, c.not_run
    )
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            // XML 1.0 has no other control characters.
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// JUnit XML grouped by test codeunit. A test that did not run is `<skipped>` with its reason.
/// `bc_build` is recorded as a property, so a build change that flips a test shows its cause.
pub fn junit(results: &[&TestResult], bc_build: Option<&str>, wall_s: f64) -> String {
    let c = Counts::of(results.iter().copied());
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<testsuites tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{wall_s:.3}\">\n",
        c.total, c.fail, c.not_run
    );
    if let Some(b) = bc_build {
        s.push_str(&format!(
            "  <properties><property name=\"bc_build\" value=\"{}\" /></properties>\n",
            esc(b)
        ));
    }
    let mut i = 0;
    while i < results.len() {
        let first = results[i];
        let mut j = i;
        while j < results.len() && results[j].app == first.app && results[j].codeunit == first.codeunit {
            j += 1;
        }
        let group = &results[i..j];
        let gc = Counts::of(group.iter().copied());
        s.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\">\n    <properties><property name=\"codeunit\" value=\"{}\" /></properties>\n",
            esc(&first.codeunit_name),
            gc.total,
            gc.fail,
            gc.not_run,
            first.codeunit
        ));
        for r in group {
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\"",
                esc(&r.codeunit_name),
                esc(&r.test)
            );
            match &r.outcome {
                Outcome::Pass => s.push_str(&format!("{head} />\n")),
                Outcome::Fail(m) => s.push_str(&format!("{head}><failure message=\"{}\" /></testcase>\n", esc(m))),
                Outcome::NotRun(m) => s.push_str(&format!("{head}><skipped message=\"{}\" /></testcase>\n", esc(m))),
            }
        }
        s.push_str("  </testsuite>\n");
        i = j;
    }
    s.push_str("</testsuites>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(cu: u32, name: &str, test: &str, outcome: Outcome) -> TestResult {
        TestResult {
            suite: 0,
            app: 0,
            codeunit: cu,
            codeunit_name: name.into(),
            test: test.into(),
            outcome,
        }
    }

    #[test]
    fn exit_codes_follow_the_rank_failure_then_not_run() {
        let c = |pass, fail, not_run| Counts {
            total: pass + fail + not_run,
            pass,
            fail,
            not_run,
        };
        assert_eq!(exit_code(&c(3, 0, 0), false), 0);
        assert_eq!(exit_code(&c(2, 1, 0), false), 1);
        assert_eq!(exit_code(&c(2, 0, 1), false), 7);
        assert_eq!(exit_code(&c(2, 0, 1), true), 0);
        assert_eq!(exit_code(&c(1, 1, 1), true), 1);
        assert_eq!(exit_code(&c(0, 0, 0), true), 6);
    }

    #[test]
    fn junit_groups_by_codeunit_and_escapes() {
        let rs = [
            r(1, "A <b>", "T1", Outcome::Pass),
            r(1, "A <b>", "T2", Outcome::Fail("bad \"x\"\nline2".into())),
            r(2, "B", "T3", Outcome::NotRun("no & way".into())),
        ];
        let refs: Vec<&TestResult> = rs.iter().collect();
        let x = junit(&refs, Some("27.5.1.2"), 1.5);
        assert!(
            x.contains("<testsuites tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\" time=\"1.500\">"),
            "{x}"
        );
        assert!(
            x.contains("<testsuite name=\"A &lt;b&gt;\" tests=\"2\" failures=\"1\""),
            "{x}"
        );
        assert!(x.contains("<failure message=\"bad &quot;x&quot;&#10;line2\" />"), "{x}");
        assert!(x.contains("<skipped message=\"no &amp; way\" />"), "{x}");
        assert!(x.contains("name=\"bc_build\" value=\"27.5.1.2\""), "{x}");
        assert_eq!(x.matches("<testsuite ").count(), 2);
    }
}
