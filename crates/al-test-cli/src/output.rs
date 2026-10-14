//! Renders a report as Markdown (CI step summary) and the runtime's outcomes as
//! JUnit in the same shape the corpus CI emits, so the two diff line for line.

use crate::oracle::{CaseId, OracleCase};
use crate::report::{Counts, Outcome, Report, Timings};

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
    if !r.suites.is_empty() {
        s.push_str("\n### Other test apps\n\n| app | oracle | total | pass | fail | not run | rate |\n|---|---|---:|---:|---:|---:|---:|\n");
        for x in &r.suites {
            let c = &x.counts;
            s.push_str(&format!(
                "| {} | BC {} | {} | {} | {} | {} | {:.1}% |\n",
                x.name,
                x.oracle_leg,
                c.total,
                c.pass,
                c.fail,
                c.not_run,
                c.rate() * 100.0
            ));
        }
        for x in r.suites.iter().filter(|x| x.bc_failed > 0) {
            s.push_str(&format!(
                "\n{}: real BC fails {} case(s) on purpose (a control); the runtime fails {} of them too.\n",
                x.name, x.bc_failed, x.bc_failed_reproduced
            ));
        }
    }
    if let Some(t) = &r.timings {
        s.push_str("\n### How long it took\n\n");
        s.push_str(&timings_table(std::slice::from_ref(t)));
        s.push_str(
            "\nLoad reads the apps and the dependency apps. Compile turns them into what the runtime runs. \
             Install baseline runs the install code of the Microsoft apps once. Tests runs every test.\n",
        );
        let by_app: Vec<String> = t
            .suites
            .iter()
            .map(|x| format!("{} {}", x.name, secs(x.tests_s)))
            .collect();
        s.push_str(&format!("\nTests by test app: {}.\n", by_app.join(", ")));
        if let Some(d) = &t.dotnet {
            s.push_str(&format!(
                "\n{}. Time spent waiting for a sidecar is part of Tests but is not the runtime; the perf budget leaves it out.\n",
                d.line()
            ));
        }
        if t.eager {
            s.push_str("\nThis run compiled every dependency routine, not only the ones the tests reach.\n");
        }
        if let Some(c) = &t.compile_cache {
            s.push_str(&format!("\nCompile cache: {}.\n", c.line()));
            if c.built {
                s.push_str(&format!(
                    "Building the entry took {} and is part of Compile above; a run that finds the entry does not pay it.\n",
                    secs(c.build_s)
                ));
            }
        }
    }
    if !r.dependencies.is_empty() {
        s.push_str(&format!("\nDependency apps: {}.\n", r.dependencies.join(", ")));
    }
    if let Some(b) = &r.bc_build {
        s.push_str(&format!("\nMicrosoft apps downloaded from BC build {b}.\n"));
    }
    if r.excluded_bc_failed > 0 {
        s.push_str(&format!(
            "\n{} case(s) real BC did not pass are excluded.\n",
            r.excluded_bc_failed
        ));
    }
    s
}

fn secs(x: f64) -> String {
    format!("{x:.2} s")
}

/// One small table of run times: a row per run, the phases in order, then the total.
pub fn timings_table(ts: &[Timings]) -> String {
    let mut s = String::from(
        "| Version | Load | Compile | Install baseline | Tests | Total |\n|---|---:|---:|---:|---:|---:|\n",
    );
    for t in ts {
        let eager = if t.eager { ", all dependencies compiled" } else { "" };
        s.push_str(&format!(
            "| BC {}{eager} | {} | {} | {} | {} | {} |\n",
            t.bc_version,
            secs(t.load_s),
            secs(t.compile_s),
            secs(t.baseline_s),
            secs(t.tests_s),
            secs(t.total_s)
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

/// `bc_build` is the Microsoft build the dependency apps came from; it is recorded as a JUnit
/// property so that a build change that flips a case shows its cause next to the result.
pub fn junit(oracle: &[OracleCase], outcome: &dyn Fn(&CaseId) -> Outcome, bc_build: Option<&str>) -> String {
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
    if let Some(b) = bc_build {
        s.push_str(&format!(
            "  <properties><property name=\"bc_build\" value=\"{}\" /></properties>\n",
            esc(b)
        ));
    }
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

/// XML text: markup characters escaped, and the control characters XML 1.0 forbids
/// (all below U+0020 but tab, LF, CR) replaced by `?`.
fn esc(t: &str) -> String {
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
        let xml = junit(&oracle(), &outcome, None);
        let back = crate::oracle::parse_junit(&xml).unwrap();
        let ids: Vec<_> = back.iter().map(|c| (c.id.clone(), c.bc_failed)).collect();
        assert_eq!(
            ids,
            vec![(id(1, "a"), false), (id(1, "b<&>"), true), (id(2, "c"), true)]
        );
    }

    #[test]
    fn the_microsoft_build_is_a_junit_property_and_the_oracle_parser_still_reads_the_file() {
        let xml = junit(&oracle(), &outcome, Some("27.5.46862.56249"));
        assert!(
            xml.contains(r#"<property name="bc_build" value="27.5.46862.56249" />"#),
            "{xml}"
        );
        assert_eq!(crate::oracle::parse_junit(&xml).unwrap().len(), 3);
        assert!(!junit(&oracle(), &outcome, None).contains("bc_build"));
    }

    #[test]
    fn the_report_names_the_build_the_dependency_apps_came_from() {
        let mut r = build(&oracle(), &areas, &outcome);
        assert!(!markdown(&r).contains("BC build"));
        r.bc_build = Some("27.5.46862.56249".into());
        assert!(markdown(&r).contains("BC build 27.5.46862.56249"), "{}", markdown(&r));
        assert!(
            serde_json::to_string(&r)
                .unwrap()
                .contains(r#""bc_build":"27.5.46862.56249""#)
        );
    }

    fn timings() -> crate::report::Timings {
        crate::report::Timings {
            bc_version: "29.0".into(),
            eager: false,
            load_s: 1.0,
            compile_s: 2.5,
            baseline_s: 3.0,
            tests_s: 12.25,
            total_s: 18.75,
            suites: vec![
                crate::report::SuiteTime {
                    name: "corpus".into(),
                    tests_s: 11.0,
                },
                crate::report::SuiteTime {
                    name: "OnPrem".into(),
                    tests_s: 1.25,
                },
            ],
            dotnet: None,
            compile_cache: None,
        }
    }

    #[test]
    fn markdown_says_what_the_compile_cache_did_and_what_building_it_cost() {
        let mut t = timings();
        t.compile_cache = Some(crate::report::CacheTimes {
            hit: false,
            built: true,
            open_s: 0.1,
            build_s: 9.5,
            mb: 13.0,
            objects: 9683,
            relowered: 29,
        });
        let mut r = build(&oracle(), &areas, &outcome);
        r.timings = Some(t.clone());
        let md = markdown(&r);
        assert!(md.contains("Compile cache: miss, built in 9.50 s"), "{md}");
        assert!(md.contains("Building the entry took 9.50 s"), "{md}");
        t.compile_cache = Some(crate::report::CacheTimes {
            hit: true,
            built: false,
            open_s: 0.05,
            build_s: 0.0,
            mb: 13.0,
            objects: 9683,
            relowered: 29,
        });
        r.timings = Some(t);
        let md = markdown(&r);
        assert!(md.contains("Compile cache: hit, opened in 0.05 s"), "{md}");
        assert!(!md.contains("Building the entry took"), "{md}");
    }

    #[test]
    fn markdown_has_a_timing_table_with_the_phases_and_the_total() {
        let mut r = build(&oracle(), &areas, &outcome);
        assert!(!markdown(&r).contains("How long it took"), "no timings, no section");
        r.timings = Some(timings());
        let md = markdown(&r);
        assert!(md.contains("### How long it took"), "{md}");
        assert!(
            md.contains("| Version | Load | Compile | Install baseline | Tests | Total |"),
            "{md}"
        );
        assert!(
            md.contains("| BC 29.0 | 1.00 s | 2.50 s | 3.00 s | 12.25 s | 18.75 s |"),
            "{md}"
        );
        assert!(md.contains("corpus 11.00 s"), "{md}");
        assert!(md.contains("OnPrem 1.25 s"), "{md}");
    }

    #[test]
    fn the_sidecar_start_is_reported_apart_from_the_phases() {
        let mut r = build(&oracle(), &areas, &outcome);
        let mut t = timings();
        assert!(
            !{
                r.timings = Some(t.clone());
                markdown(&r)
            }
            .contains("sidecar")
        );
        t.dotnet = Some(crate::report::DotNetTimes {
            processes: 2,
            start_s: 2.5,
            start_max_s: 1.5,
            find_s: 0.25,
            waited_s: 0.5,
            calls: 35,
            calls_s: 0.25,
        });
        r.timings = Some(t);
        let md = markdown(&r);
        assert!(
            md.contains(
                "2 .NET sidecar process(es) started: starting 2.50 s in total, 1.50 s at most; finding dotnet 0.25 s; sessions waited 0.50 s for theirs; 35 request(s) 0.25 s"
            ),
            "{md}"
        );
        // A report written before the field existed still reads.
        let json = r#"{"timings":{"bc_version":"29.0","eager":false,"load_s":1.0,"compile_s":1.0,"baseline_s":1.0,"tests_s":1.0,"total_s":4.0,"suites":[]}}"#;
        assert!(crate::report::Timings::from_report_json(json).unwrap().dotnet.is_none());
    }

    #[test]
    fn the_json_report_carries_the_timings() {
        let mut r = build(&oracle(), &areas, &outcome);
        r.timings = Some(timings());
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let t = &v["timings"];
        for key in ["load_s", "compile_s", "baseline_s", "tests_s", "total_s"] {
            assert!(t[key].is_number(), "{key} in {t}");
        }
        assert_eq!(t["bc_version"], "29.0");
        assert_eq!(t["suites"][1]["name"], "OnPrem");
    }

    #[test]
    fn a_timing_table_is_read_back_from_report_files() {
        let mut r = build(&oracle(), &areas, &outcome);
        r.timings = Some(timings());
        let json = serde_json::to_string_pretty(&r).unwrap();
        let t = crate::report::Timings::from_report_json(&json).unwrap();
        let mut eager = t.clone();
        eager.eager = true;
        eager.bc_version = "28.0".into();
        let table = timings_table(&[t, eager]);
        assert!(
            table.contains("| Version | Load | Compile | Install baseline | Tests | Total |"),
            "{table}"
        );
        assert!(table.contains("| BC 29.0 |"), "{table}");
        assert!(table.contains("| BC 28.0, all dependencies compiled |"), "{table}");
        assert!(crate::report::Timings::from_report_json("{}").is_err());
    }

    #[test]
    fn junit_never_emits_characters_xml_forbids() {
        // XML 1.0 allows no control characters but tab, LF and CR; a reason that carries one
        // (a qualified-name separator) made the CI artifact unparseable.
        let xml = esc("a\u{1f}b\u{0}c\td");
        assert!(
            !xml.chars()
                .any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')),
            "{xml:?}"
        );
        assert!(
            xml.contains('a') && xml.contains('b') && xml.contains("c\td"),
            "{xml:?}"
        );
    }

    #[test]
    fn junit_marks_not_run_as_skipped_and_fail_as_failure() {
        let xml = junit(&oracle(), &outcome, None);
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
