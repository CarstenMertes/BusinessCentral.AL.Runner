//! The oracle: the JUnit report the corpus CI produced against real BC.
//!
//! Every `<testcase>` in it passed on real BC, so it is the denominator of the
//! pass rate. A case is identified by its codeunit id and procedure name.

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CaseId {
    pub codeunit: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleCase {
    pub id: CaseId,
    /// True when real BC reported this case as failed, errored or skipped.
    pub bc_failed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OracleError {
    Xml(String),
    BadCodeunit(String),
}

pub fn parse_junit(xml: &str) -> Result<Vec<OracleCase>, OracleError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut cases = Vec::new();
    // The case whose element is still open, waiting for a failure child or its end tag.
    let mut open: Option<OracleCase> = None;
    loop {
        let ev = reader.read_event().map_err(|e| OracleError::Xml(e.to_string()))?;
        match ev {
            Event::Empty(e) if e.name().as_ref() == b"testcase" => cases.push(testcase(&e)?),
            Event::Start(e) if e.name().as_ref() == b"testcase" => open = Some(testcase(&e)?),
            Event::Start(e) | Event::Empty(e) if matches!(e.name().as_ref(), b"failure" | b"error" | b"skipped") => {
                if let Some(c) = open.as_mut() {
                    c.bc_failed = true;
                }
            }
            Event::End(e) if e.name().as_ref() == b"testcase" => {
                cases.extend(open.take());
            }
            Event::Eof => {
                if open.is_some() {
                    return Err(OracleError::Xml("unterminated <testcase>".into()));
                }
                return Ok(cases);
            }
            _ => {}
        }
    }
}

fn testcase(e: &quick_xml::events::BytesStart) -> Result<OracleCase, OracleError> {
    let mut classname = None;
    let mut name = None;
    for a in e.attributes() {
        let a = a.map_err(|e| OracleError::Xml(e.to_string()))?;
        let v = a
            .unescape_value()
            .map_err(|e| OracleError::Xml(e.to_string()))?
            .into_owned();
        match a.key.as_ref() {
            b"classname" => classname = Some(v),
            b"name" => name = Some(v),
            _ => {}
        }
    }
    let classname = classname.ok_or_else(|| OracleError::Xml("testcase without classname".into()))?;
    let name = name.ok_or_else(|| OracleError::Xml("testcase without name".into()))?;
    let codeunit = classname
        .strip_prefix("Codeunit ")
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| OracleError::BadCodeunit(classname.clone()))?;
    Ok(OracleCase {
        id: CaseId { codeunit, name },
        bc_failed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<testsuites name="DEFAULT" tests="3" failures="1">
  <testsuite name="Codeunit 60002" tests="2">
    <testcase classname="Codeunit 60002" name="A_Works" time="0.004" />
    <testcase classname="Codeunit 60002" name="B_Fails" time="0.003"><failure message="x">boom</failure></testcase>
  </testsuite>
  <testsuite name="Codeunit 60006" tests="1">
    <testcase classname="Codeunit 60006" name="C_Works" time="0.040" />
  </testsuite>
</testsuites>"#;

    fn id(cu: u32, n: &str) -> CaseId {
        CaseId {
            codeunit: cu,
            name: n.into(),
        }
    }

    #[test]
    fn parses_every_testcase_with_its_codeunit() {
        let cases = parse_junit(SAMPLE).unwrap();
        let ids: Vec<_> = cases.iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            ids,
            vec![id(60002, "A_Works"), id(60002, "B_Fails"), id(60006, "C_Works")]
        );
    }

    #[test]
    fn marks_a_case_with_a_failure_child_as_bc_failed() {
        let cases = parse_junit(SAMPLE).unwrap();
        let failed: Vec<_> = cases
            .iter()
            .filter(|c| c.bc_failed)
            .map(|c| c.id.name.as_str())
            .collect();
        assert_eq!(failed, vec!["B_Fails"]);
    }

    #[test]
    fn rejects_a_classname_that_is_not_a_codeunit() {
        let xml = r#"<testsuites><testsuite name="x"><testcase classname="Page 1" name="n"/></testsuite></testsuites>"#;
        assert!(matches!(parse_junit(xml), Err(OracleError::BadCodeunit(_))));
    }

    #[test]
    fn rejects_malformed_xml() {
        assert!(matches!(parse_junit("<testsuites><testcase"), Err(OracleError::Xml(_))));
    }
}
