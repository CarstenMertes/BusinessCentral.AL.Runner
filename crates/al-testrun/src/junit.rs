//! The JUnit XML of a run, written in one place (ADR-037 item 6). The two command lines write two dialects of it,
//! and both stay byte for byte what they were: `Dialect::Oracle` is the file `al-test-cli --junit` writes and the
//! oracle parser reads back; `Dialect::Runner` is the v2-shaped file `alrunner test --junit` writes. Each caller
//! builds the suites (how tests are grouped differs) and this module writes them.

use crate::Outcome;
use al_builtins::assumptions::Assumption;

/// Which of the two files to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// `<?xml version='1.0' ...?>`, no `errors` attribute, no time on `testsuites`, one `testsuite` per codeunit
    /// without properties, a failure always without a body, control characters in text replaced by `?`.
    Oracle,
    /// `<?xml version="1.0" ...?>`, `errors="0"` and the wall time on `testsuites`, counts and a `codeunit` property
    /// on each `testsuite`, a failure with the call stack as its body, line breaks and tabs escaped, control
    /// characters in text dropped.
    Runner,
}

/// One test.
#[derive(Debug, Clone)]
pub struct Case<'a> {
    /// The `classname` of the test case.
    pub class: String,
    pub name: &'a str,
    /// Seconds.
    pub time: f64,
    pub outcome: &'a Outcome,
    /// The unprobed assumptions the test ran through (also written for a pass).
    pub assumptions: &'a [Assumption],
    /// The failure body of the Runner dialect: the frames of the call stack, one per line, or empty for none.
    pub stack: String,
}

/// The tests of one test codeunit.
#[derive(Debug, Clone)]
pub struct Suite<'a> {
    /// The `name` of the `testsuite`.
    pub name: String,
    /// The codeunit id (the `codeunit` property of the Runner dialect).
    pub codeunit: u32,
    pub cases: Vec<Case<'a>>,
}

/// The document. `bc_build` is the Microsoft build the dependency apps came from, recorded as a property so a build
/// change that flips a case shows its cause; `wall_s` is the run's wall time (Runner dialect only).
pub fn write(suites: &[Suite<'_>], dialect: Dialect, bc_build: Option<&str>, wall_s: f64) -> String {
    let esc: fn(&str) -> String = match dialect {
        Dialect::Oracle => esc_oracle,
        Dialect::Runner => esc_runner,
    };
    let count = |f: &dyn Fn(&Outcome) -> bool| suites.iter().flat_map(|s| &s.cases).filter(|c| f(c.outcome)).count();
    let total: usize = suites.iter().map(|s| s.cases.len()).sum();
    let failures = count(&|o| matches!(o, Outcome::Fail(_)));
    let skipped = count(&|o| matches!(o, Outcome::NotRun(_)));
    let mut s = match dialect {
        Dialect::Oracle => format!(
            "<?xml version='1.0' encoding='utf-8'?>\n<testsuites tests=\"{total}\" failures=\"{failures}\" skipped=\"{skipped}\">\n"
        ),
        Dialect::Runner => format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<testsuites tests=\"{total}\" failures=\"{failures}\" errors=\"0\" skipped=\"{skipped}\" time=\"{wall_s:.3}\">\n"
        ),
    };
    if let Some(b) = bc_build {
        s.push_str(&format!(
            "  <properties><property name=\"bc_build\" value=\"{}\" /></properties>\n",
            esc(b)
        ));
    }
    for suite in suites {
        let time: f64 = suite.cases.iter().map(|c| c.time).sum();
        match dialect {
            Dialect::Oracle => s.push_str(&format!(
                "  <testsuite name=\"{}\" tests=\"{}\" time=\"{time:.3}\">\n",
                esc(&suite.name),
                suite.cases.len()
            )),
            Dialect::Runner => {
                let fail = suite
                    .cases
                    .iter()
                    .filter(|c| matches!(c.outcome, Outcome::Fail(_)))
                    .count();
                let not_run = suite
                    .cases
                    .iter()
                    .filter(|c| matches!(c.outcome, Outcome::NotRun(_)))
                    .count();
                s.push_str(&format!(
                    "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{fail}\" errors=\"0\" skipped=\"{not_run}\" time=\"{time:.3}\">\n    <properties><property name=\"codeunit\" value=\"{}\" /></properties>\n",
                    esc(&suite.name),
                    suite.cases.len(),
                    suite.codeunit
                ));
            }
        }
        for c in &suite.cases {
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                esc(&c.class),
                esc(c.name),
                c.time
            );
            // The assumptions a test ran through are a property of the test case, also of a pass: a flagged pass is
            // still a pass for the tools that read this file.
            let props = if c.assumptions.is_empty() {
                String::new()
            } else {
                format!(
                    "<properties><property name=\"assumptions\" value=\"{}\" /></properties>",
                    esc(&crate::flagged::id_list(c.assumptions))
                )
            };
            match (dialect, c.outcome) {
                (_, Outcome::Pass) if props.is_empty() => s.push_str(&format!("{head} />\n")),
                (_, Outcome::Pass) => s.push_str(&format!("{head}>{props}</testcase>\n")),
                (Dialect::Oracle, Outcome::Fail(m)) => {
                    s.push_str(&format!("{head}><failure message=\"{}\" /></testcase>\n", esc(m)))
                }
                (Dialect::Runner, Outcome::Fail(m)) if c.stack.is_empty() => s.push_str(&format!(
                    "{head}>{props}<failure message=\"{}\" /></testcase>\n",
                    esc(m)
                )),
                (Dialect::Runner, Outcome::Fail(m)) => s.push_str(&format!(
                    "{head}>{props}<failure message=\"{}\">{}</failure></testcase>\n",
                    esc(m),
                    esc(&c.stack)
                )),
                (_, Outcome::NotRun(m)) => {
                    s.push_str(&format!("{head}><skipped message=\"{}\" /></testcase>\n", esc(m)))
                }
            }
        }
        s.push_str("  </testsuite>\n");
    }
    s.push_str("</testsuites>\n");
    s
}

/// Oracle dialect: markup characters escaped, and the control characters XML 1.0 forbids (all below U+0020 but tab,
/// LF, CR) replaced by `?`.
fn esc_oracle(t: &str) -> String {
    t.chars()
        .map(|c| {
            if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') {
                '?'
            } else {
                c
            }
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Runner dialect: markup characters, line breaks and tabs escaped; the other control characters (XML 1.0 has none)
/// dropped.
fn esc_runner(s: &str) -> String {
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
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- The two writers this module replaced, verbatim in effect, over the model, as the oracle of the
    // differential test. (The Oracle writer did not escape the suite and class names: they were always `Codeunit N`, which
    // escaping leaves as it is.)

    fn legacy_oracle(suites: &[Suite<'_>], bc_build: Option<&str>) -> String {
        let all = suites.iter().flat_map(|s| &s.cases);
        let total = all.clone().count();
        let failures = all.clone().filter(|c| matches!(c.outcome, Outcome::Fail(_))).count();
        let skipped = all.filter(|c| matches!(c.outcome, Outcome::NotRun(_))).count();
        let mut s = format!(
            "<?xml version='1.0' encoding='utf-8'?>\n<testsuites tests=\"{total}\" failures=\"{failures}\" skipped=\"{skipped}\">\n"
        );
        if let Some(b) = bc_build {
            s.push_str(&format!(
                "  <properties><property name=\"bc_build\" value=\"{}\" /></properties>\n",
                legacy_esc_oracle(b)
            ));
        }
        for suite in suites {
            s.push_str(&format!(
                "  <testsuite name=\"{}\" tests=\"{}\" time=\"{:.3}\">\n",
                legacy_esc_oracle(&suite.name),
                suite.cases.len(),
                suite.cases.iter().map(|c| c.time).sum::<f64>()
            ));
            for c in &suite.cases {
                let head = format!(
                    "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                    legacy_esc_oracle(&c.class),
                    legacy_esc_oracle(c.name),
                    c.time
                );
                match c.outcome {
                    Outcome::Pass => {
                        if c.assumptions.is_empty() {
                            s.push_str(&format!("{head} />\n"));
                        } else {
                            s.push_str(&format!(
                                "{head}><properties><property name=\"assumptions\" value=\"{}\" /></properties></testcase>\n",
                                legacy_esc_oracle(&crate::flagged::id_list(c.assumptions))
                            ));
                        }
                    }
                    Outcome::Fail(m) => s.push_str(&format!(
                        "{head}><failure message=\"{}\" /></testcase>\n",
                        legacy_esc_oracle(m)
                    )),
                    Outcome::NotRun(m) => s.push_str(&format!(
                        "{head}><skipped message=\"{}\" /></testcase>\n",
                        legacy_esc_oracle(m)
                    )),
                }
            }
            s.push_str("  </testsuite>\n");
        }
        s.push_str("</testsuites>\n");
        s
    }

    fn legacy_esc_oracle(t: &str) -> String {
        t.chars()
            .map(|c| {
                if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') {
                    '?'
                } else {
                    c
                }
            })
            .collect::<String>()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn legacy_esc_runner(s: &str) -> String {
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
                c if (c as u32) < 0x20 => {}
                c => out.push(c),
            }
        }
        out
    }

    fn legacy_runner(suites: &[Suite<'_>], bc_build: Option<&str>, wall_s: f64) -> String {
        let all = || suites.iter().flat_map(|s| &s.cases);
        let (total, fail, not_run) = (
            all().count(),
            all().filter(|c| matches!(c.outcome, Outcome::Fail(_))).count(),
            all().filter(|c| matches!(c.outcome, Outcome::NotRun(_))).count(),
        );
        let mut s = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<testsuites tests=\"{total}\" failures=\"{fail}\" errors=\"0\" skipped=\"{not_run}\" time=\"{wall_s:.3}\">\n"
        );
        if let Some(b) = bc_build {
            s.push_str(&format!(
                "  <properties><property name=\"bc_build\" value=\"{}\" /></properties>\n",
                legacy_esc_runner(b)
            ));
        }
        for suite in suites {
            let gf = suite
                .cases
                .iter()
                .filter(|c| matches!(c.outcome, Outcome::Fail(_)))
                .count();
            let gn = suite
                .cases
                .iter()
                .filter(|c| matches!(c.outcome, Outcome::NotRun(_)))
                .count();
            s.push_str(&format!(
                "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{:.3}\">\n    <properties><property name=\"codeunit\" value=\"{}\" /></properties>\n",
                legacy_esc_runner(&suite.name),
                suite.cases.len(),
                gf,
                gn,
                suite.cases.iter().map(|c| c.time).sum::<f64>(),
                suite.codeunit
            ));
            for c in &suite.cases {
                let head = format!(
                    "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
                    legacy_esc_runner(&c.class),
                    legacy_esc_runner(c.name),
                    c.time
                );
                let props = if c.assumptions.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<properties><property name=\"assumptions\" value=\"{}\" /></properties>",
                        legacy_esc_runner(&crate::flagged::id_list(c.assumptions))
                    )
                };
                match c.outcome {
                    Outcome::Pass if !props.is_empty() => s.push_str(&format!("{head}>{props}</testcase>\n")),
                    Outcome::Pass => s.push_str(&format!("{head} />\n")),
                    Outcome::Fail(m) if c.stack.is_empty() => s.push_str(&format!(
                        "{head}>{props}<failure message=\"{}\" /></testcase>\n",
                        legacy_esc_runner(m)
                    )),
                    Outcome::Fail(m) => s.push_str(&format!(
                        "{head}>{props}<failure message=\"{}\">{}</failure></testcase>\n",
                        legacy_esc_runner(m),
                        legacy_esc_runner(&c.stack)
                    )),
                    Outcome::NotRun(m) => s.push_str(&format!(
                        "{head}><skipped message=\"{}\" /></testcase>\n",
                        legacy_esc_runner(m)
                    )),
                }
            }
            s.push_str("  </testsuite>\n");
        }
        s.push_str("</testsuites>\n");
        s
    }

    #[test]
    fn both_dialects_are_what_the_writers_they_replaced_wrote() {
        let texts = [
            "",
            "plain",
            "a<b>&\"c\"",
            "tab\there",
            "line\nbreak\r\n",
            "ctl\u{1}\u{1f}x",
            "caf\u{e9} \u{20ac}",
            "'q'",
        ];
        let marks_none: Vec<Assumption> = Vec::new();
        let marks_some: Vec<Assumption> = Assumption::ALL.iter().copied().take(3).collect();
        let mut outcomes = Vec::new();
        for t in texts {
            outcomes.push(Outcome::Pass);
            outcomes.push(Outcome::Fail(t.to_string()));
            outcomes.push(Outcome::NotRun(t.to_string()));
        }
        let mut suites = Vec::new();
        for (k, chunk) in outcomes.chunks(4).enumerate() {
            suites.push(Suite {
                name: format!("Suite {k} {}", texts[k % texts.len()]),
                codeunit: 50000 + k as u32,
                cases: chunk
                    .iter()
                    .enumerate()
                    .map(|(i, o)| Case {
                        class: format!("Class {k}"),
                        name: texts[(k + i) % texts.len()],
                        time: 0.0005 * (k * 7 + i) as f64,
                        outcome: o,
                        assumptions: if (k + i) % 2 == 0 { &marks_none } else { &marks_some },
                        stack: if i % 2 == 0 {
                            String::new()
                        } else {
                            texts[(k + 2 * i) % texts.len()].to_string()
                        },
                    })
                    .collect(),
            });
        }
        for build in [None, Some("27.5.1<&>")] {
            assert_eq!(
                write(&suites, Dialect::Oracle, build, 0.0),
                legacy_oracle(&suites, build)
            );
            assert_eq!(
                write(&suites, Dialect::Runner, build, 12.3456),
                legacy_runner(&suites, build, 12.3456)
            );
        }
        assert_eq!(write(&[], Dialect::Runner, None, 0.0), legacy_runner(&[], None, 0.0));
        assert_eq!(write(&[], Dialect::Oracle, None, 0.0), legacy_oracle(&[], None));
    }

    #[test]
    fn the_oracle_dialect_replaces_control_characters_and_the_runner_dialect_drops_them() {
        assert_eq!(esc_oracle("a\u{1f}b\u{0}c\td"), "a?b?c\td");
        assert_eq!(esc_runner("a\u{1f}b\u{0}c\td\n"), "abc&#9;d&#10;");
    }
}
