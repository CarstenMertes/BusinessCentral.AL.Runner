//! What a run prints and writes: the summary, the lines per test, JUnit XML.

use al_testrun::Outcome;
use al_testrun::runtime::TestResult;

/// Counts of a set of results.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub pass: usize,
    /// Of the passes: the tests that ran through an unprobed assumption (`al_builtins::assumptions`).
    pub flagged: usize,
    pub fail: usize,
    pub not_run: usize,
    /// Of the not run: the tests a missing or unusable .NET helper stopped (the reason starts with `dotnet-unavailable`).
    pub dotnet_unavailable: usize,
}

impl Counts {
    pub fn of<'a>(results: impl IntoIterator<Item = &'a TestResult>) -> Counts {
        let mut c = Counts::default();
        for r in results {
            c.total += 1;
            match r.outcome {
                Outcome::Pass => {
                    c.pass += 1;
                    c.flagged += usize::from(!r.assumptions.is_empty());
                }
                Outcome::Fail(_) => c.fail += 1,
                Outcome::NotRun(ref why) => {
                    c.not_run += 1;
                    if why.starts_with(al_vm::dotnet::UNAVAILABLE) {
                        c.dotnet_unavailable += 1;
                    }
                }
            }
        }
        c
    }
}

/// The exit code of a finished run (ADR-016 rule 2): 6 when nothing was selected, 1 when a test
/// failed, 8 when a test was not run because the .NET helper is unavailable (a problem of the
/// machine, not of the tests; unless `allow_not_run`), 7 when a test was not run (unless
/// `allow_not_run`), else 0. Codes 2 and 3 are decided
/// before this point.
pub fn exit_code(c: &Counts, allow_not_run: bool) -> u8 {
    if c.total == 0 {
        6
    } else if c.fail > 0 {
        1
    } else if c.dotnet_unavailable > 0 && !allow_not_run {
        8
    } else if c.not_run > 0 && !allow_not_run {
        7
    } else {
        0
    }
}

/// The warning for a run whose tests were stopped by a missing or unusable .NET helper, so that it
/// is never mistaken for a lower pass count. `why` is the reason of one such test.
pub fn dotnet_warning(c: &Counts, why: &str) -> Option<String> {
    (c.dotnet_unavailable > 0).then(|| {
        format!(
            "WARNING: DotNet unavailable: {} test(s) not run because the .NET helper could not be used ({}). Pass --dotnet-sidecar PATH/AlDotNetSidecar.dll or set ALRT_DOTNET_SIDECAR; the pass count of this run is not comparable with one that had the helper.",
            c.dotnet_unavailable,
            first_line(why.trim_start_matches(al_vm::dotnet::UNAVAILABLE).trim_start_matches(':').trim(), 160)
        )
    })
}

/// The first line of a message, cut to `max` characters.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    // A control character (a NUL in a compared text, say) is shown as `\x00`, a tab stays.
    let shown: String = line
        .chars()
        .map(|c| {
            if c.is_control() && c != '\t' {
                format!("\\x{:02X}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect();
    al_testrun::refusals::clip(&shown, max)
}

/// `Tests: 12 total, 5 passed, 1 failed, 6 not run`, and the warning that goes with a run that
/// has no test data.
pub fn summary_line(c: &Counts, test_data: bool) -> String {
    let flagged = if c.flagged > 0 {
        format!(" ({} flagged: rely on unprobed assumptions)", c.flagged)
    } else {
        String::new()
    };
    format!(
        "Tests: {} total, {} passed{flagged}, {} failed, {} not run{}",
        c.total,
        c.pass,
        c.fail,
        c.not_run,
        if test_data {
            ""
        } else {
            " (no test data: failures are mostly missing setup data and are not trusted; the not-run counts and reasons are)"
        }
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
/// The frames of an error's call stack for a person: one per line, innermost first. A frame whose
/// format is not known is in the stack as a marker; it reads as such.
pub fn stack_lines(stack: &str) -> Vec<String> {
    stack
        .lines()
        .map(|l| {
            if l.contains(al_testrun::UNTRACKED_CALL_STACK) {
                "(a frame whose format is not known)".to_string()
            } else {
                l.to_string()
            }
        })
        .collect()
}

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
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{:.3}\">\n    <properties><property name=\"codeunit\" value=\"{}\" /></properties>\n",
            esc(&first.codeunit_name),
            gc.total,
            gc.fail,
            gc.not_run,
            group.iter().map(|r| r.time.as_secs_f64()).sum::<f64>(),
            first.codeunit
        ));
        for r in group {
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                esc(&r.codeunit_name),
                esc(&r.test),
                r.time.as_secs_f64()
            );
            // The assumptions a test ran through are a property of the test case, also of a pass: a flagged
            // pass is still a pass for the tools that read this file.
            let props = if r.assumptions.is_empty() {
                String::new()
            } else {
                format!(
                    "<properties><property name=\"assumptions\" value=\"{}\" /></properties>",
                    esc(&al_testrun::flagged::id_list(&r.assumptions))
                )
            };
            match &r.outcome {
                Outcome::Pass if !props.is_empty() => s.push_str(&format!("{head}>{props}</testcase>\n")),
                Outcome::Pass => s.push_str(&format!("{head} />\n")),
                Outcome::Fail(m) if r.call_stack.is_empty() => s.push_str(&format!(
                    "{head}>{props}<failure message=\"{}\" /></testcase>\n",
                    esc(m)
                )),
                Outcome::Fail(m) => s.push_str(&format!(
                    "{head}>{props}<failure message=\"{}\">{}</failure></testcase>\n",
                    esc(m),
                    esc(&stack_lines(&r.call_stack).join("\n"))
                )),
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
            call_stack: String::new(),
            assumptions: Vec::new(),
            time: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn exit_codes_follow_the_rank_failure_then_not_run() {
        let c = |pass, fail, not_run| Counts {
            total: pass + fail + not_run,
            pass,
            flagged: 0,
            fail,
            not_run,
            dotnet_unavailable: 0,
        };
        assert_eq!(exit_code(&c(3, 0, 0), false), 0);
        assert_eq!(exit_code(&c(2, 1, 0), false), 1);
        assert_eq!(exit_code(&c(2, 0, 1), false), 7);
        assert_eq!(exit_code(&c(2, 0, 1), true), 0);
        assert_eq!(exit_code(&c(1, 1, 1), true), 1);
        assert_eq!(exit_code(&c(0, 0, 0), true), 6);
    }

    #[test]
    fn tests_stopped_by_a_missing_dotnet_helper_have_their_own_exit_code_and_warning() {
        let rs = [
            r(1, "A", "T1", Outcome::Pass),
            r(
                1,
                "A",
                "T2",
                Outcome::NotRun("dotnet-unavailable: DotNet 'System.Convert' sidecar not built; run ...".into()),
            ),
            r(1, "A", "T3", Outcome::NotRun("something else".into())),
        ];
        let c = Counts::of(rs.iter());
        assert_eq!((c.not_run, c.dotnet_unavailable), (2, 1));
        assert_eq!(exit_code(&c, false), 8);
        assert_eq!(
            exit_code(&c, true),
            0,
            "--allow-not-run is the user's word that not run is fine"
        );
        let w = dotnet_warning(&c, "dotnet-unavailable: DotNet 'System.Convert' sidecar not built").unwrap();
        assert!(
            w.contains("DotNet unavailable: 1 test(s)") && w.contains("ALRT_DOTNET_SIDECAR"),
            "{w}"
        );
        assert!(
            w.contains("sidecar not built") && !w.contains("dotnet-unavailable"),
            "{w}"
        );
        let none = Counts::of(rs[..1].iter().chain(rs[2..].iter()));
        assert_eq!(exit_code(&none, false), 7);
        assert!(dotnet_warning(&none, "").is_none());
    }

    #[test]
    fn a_pass_that_ran_through_an_assumption_is_flagged_in_the_summary_and_junit_and_still_a_pass() {
        let mut flagged = r(1, "A", "T1", Outcome::Pass);
        flagged.assumptions = vec![
            al_testrun::assumptions::Assumption::GuidToText,
            al_testrun::assumptions::Assumption::RecordGetRecordId,
        ];
        let rs = [
            flagged,
            r(1, "A", "T2", Outcome::Pass),
            r(1, "A", "T3", Outcome::Fail("x".into())),
        ];
        let c = Counts::of(rs.iter());
        assert_eq!((c.pass, c.flagged), (2, 1));
        assert_eq!(
            summary_line(&c, true),
            "Tests: 3 total, 2 passed (1 flagged: rely on unprobed assumptions), 1 failed, 0 not run"
        );
        let clean = Counts::of(rs[1..].iter());
        assert_eq!(
            summary_line(&clean, true),
            "Tests: 2 total, 1 passed, 1 failed, 0 not run"
        );
        let refs: Vec<&TestResult> = rs.iter().collect();
        let x = junit(&refs, None, 0.0);
        assert!(
            x.contains("name=\"T1\"><properties><property name=\"assumptions\" value=\"guid-to-text,record-get-record-id\" /></properties></testcase>"),
            "{x}"
        );
        assert!(x.contains("name=\"T2\" />"), "{x}");
        assert!(x.contains("failures=\"1\""), "{x}");
        assert!(
            !x.contains("name=\"T1\"><skipped") && !x.contains("name=\"T1\"><failure"),
            "{x}"
        );
    }

    #[test]
    fn a_control_character_in_a_message_is_shown_as_an_escape() {
        // Tests-Workflow has a failure whose text holds a NUL (`Expected:<asdf\0>`): printed raw it made
        // the whole output a binary file to grep and to a terminal.
        assert_eq!(
            first_line("Expected:<asdf\u{0}> and \u{7}bell\tab", 80),
            "Expected:<asdf\\x00> and \\x07bell\tab"
        );
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

    #[test]
    fn junit_carries_the_time_of_each_test_and_the_sum_for_its_codeunit() {
        let mut a = r(1, "A", "T1", Outcome::Pass);
        a.time = std::time::Duration::from_millis(1250);
        let mut b = r(1, "A", "T2", Outcome::Fail("x".into()));
        b.time = std::time::Duration::from_millis(500);
        let mut c = r(1, "A", "T3", Outcome::NotRun("y".into()));
        c.time = std::time::Duration::ZERO;
        let refs = [&a, &b, &c];
        let x = junit(&refs, None, 2.0);
        assert!(
            x.contains("<testcase classname=\"A\" name=\"T1\" time=\"1.250\" />"),
            "{x}"
        );
        assert!(x.contains("name=\"T2\" time=\"0.500\">"), "{x}");
        assert!(x.contains("name=\"T3\" time=\"0.000\">"), "{x}");
        assert!(
            x.contains("<testsuite name=\"A\" tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\" time=\"1.750\">"),
            "{x}"
        );
    }

    #[test]
    fn junit_puts_the_call_stack_in_the_failure_and_names_an_unknown_frame() {
        let mut f = r(1, "A", "T1", Outcome::Fail("boom".into()));
        f.call_stack = format!(
            "\"A\"(CodeUnit 1).Inner line 2\n{}\n\"A\"(CodeUnit 1).T1 line 1",
            al_testrun::UNTRACKED_CALL_STACK
        );
        let refs = [&f];
        let x = junit(&refs, None, 0.0);
        assert!(x.contains("<failure message=\"boom\">&quot;A&quot;(CodeUnit 1).Inner line 2&#10;(a frame whose format is not known)&#10;"), "{x}");
        assert!(x.contains("</failure>"), "{x}");
        assert!(!x.contains(al_testrun::UNTRACKED_CALL_STACK), "{x}");
    }
}
