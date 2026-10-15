//! `captureValues` and `iterationTracking` (ADR-030): what a test's code did, as the JSON of AL Runner v2's `execute`.

use crate::coverage::Label;
use al_testrun::events::CapturedAt;
use al_vm::{CaptureConfig, ClosedBy, Rendered};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn number(text: &str) -> Value {
    text.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .and_then(serde_json::Number::from_f64)
        .map_or_else(|| json!(text), Value::Number)
}

fn rendered(v: &Rendered) -> (Value, Option<String>) {
    match v {
        Rendered::Null => (Value::Null, None),
        Rendered::Bool(b) => (json!(b), None),
        Rendered::Int(i) => (json!(i), None),
        Rendered::Num(n) => (number(n), None),
        Rendered::Text(t) => (json!(t), None),
        Rendered::Unrendered(why) => (Value::Null, Some(why.clone())),
    }
}

fn tag(m: &mut Map<String, Value>, base: u32, loop_id: Option<u32>, iteration: Option<u32>) {
    if let (Some(l), Some(i)) = (loop_id, iteration) {
        m.insert("loop".into(), json!(base + l));
        m.insert("iteration".into(), json!(i));
    }
}

/// The capture of one test as v2 puts it into a test result: `capturedValues` with `captureValues`, `loops` with
/// `iterationTracking`, and (with `iterationTracking`) `messages`, the tagged ones of a scratch run. Loop ids are
/// `base` and above, so the ids of a response do not repeat. `labels` names the files.
pub fn json(c: &CapturedAt, cfg: CaptureConfig, base: u32, labels: &dyn Fn(usize, &str) -> Label) -> Value {
    let mut out = Map::new();
    let cap = &c.captured;
    if cap.truncated {
        out.insert("capturedValuesTruncated".into(), json!(true));
    }
    if cfg.values {
        let values: Vec<Value> = cap
            .values
            .iter()
            .map(|v| {
                let (value, error) = rendered(&v.value);
                let mut m = Map::new();
                m.insert("scopeName".into(), json!(v.scope));
                m.insert("variableName".into(), json!(v.variable));
                m.insert("value".into(), value);
                m.insert("statementId".into(), json!(v.statement));
                if let Some(e) = error {
                    m.insert("captureError".into(), json!(e));
                }
                tag(&mut m, base, v.loop_id, v.iteration);
                Value::Object(m)
            })
            .collect();
        out.insert("capturedValues".into(), json!(values));
    }
    if cfg.loops {
        let loops: Vec<Value> = cap
            .loops
            .iter()
            .map(|l| {
                let mut m = Map::new();
                m.insert("id".into(), json!(base + l.id));
                m.insert("scope".into(), json!(l.scope));
                if let Some(loc) = c.locations.get(&l.statement) {
                    m.insert("file".into(), json!(labels(loc.app, &loc.file).absolute));
                    m.insert("line".into(), json!(loc.line));
                    m.insert("column".into(), json!(loc.column));
                    m.insert("endLine".into(), json!(loc.end_line));
                }
                m.insert("iterationCount".into(), json!(l.iterations.len()));
                m.insert(
                    "closedBy".into(),
                    json!(match l.closed_by {
                        ClosedBy::Exit => "exit",
                        ClosedBy::ScopeExit => "scopeExit",
                        ClosedBy::Unfinished => "unfinished",
                    }),
                );
                if let Some(p) = l.parent {
                    m.insert("parentLoop".into(), json!(base + p));
                    if let Some(i) = l.parent_iteration {
                        m.insert("parentIteration".into(), json!(i));
                    }
                }
                let passes: Vec<Value> = l
                    .iterations
                    .iter()
                    .enumerate()
                    .map(|(i, statements)| {
                        let lines: BTreeSet<u32> = statements
                            .iter()
                            .filter_map(|s| c.locations.get(s).map(|loc| loc.line))
                            .collect();
                        json!({ "index": i + 1, "statements": statements, "lines": lines })
                    })
                    .collect();
                m.insert("iterations".into(), json!(passes));
                Value::Object(m)
            })
            .collect();
        out.insert("loops".into(), json!(loops));
        let messages: Vec<Value> = cap
            .messages
            .iter()
            .map(|msg| {
                let mut m = Map::new();
                m.insert("text".into(), json!(msg.text));
                m.insert("scopeName".into(), json!(msg.scope));
                if let Some(s) = msg.statement {
                    m.insert("statementId".into(), json!(s));
                }
                tag(&mut m, base, msg.loop_id, msg.iteration);
                Value::Object(m)
            })
            .collect();
        out.insert("messages".into(), json!(messages));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_testrun::events::Loc;
    use al_vm::{Captured, LoopRecord, MessageRecord, ValueRecord};

    fn labels(_: usize, f: &str) -> Label {
        Label {
            absolute: format!("/w/{f}"),
            relative: f.into(),
        }
    }

    fn sample() -> CapturedAt {
        let mut locations = std::collections::BTreeMap::new();
        for (id, line) in [(10, 3), (11, 4), (12, 5)] {
            locations.insert(
                id,
                Loc {
                    app: 0,
                    file: "a.al".into(),
                    line,
                    column: 9,
                    end_line: line,
                },
            );
        }
        CapturedAt {
            captured: Captured {
                truncated: false,
                values: vec![
                    ValueRecord {
                        scope: "OnRun".into(),
                        variable: "x".into(),
                        value: Rendered::Num("1.5".into()),
                        statement: 11,
                        loop_id: Some(0),
                        iteration: Some(2),
                    },
                    ValueRecord {
                        scope: "OnRun".into(),
                        variable: "l".into(),
                        value: Rendered::Unrendered("a List is not rendered yet".into()),
                        statement: 12,
                        loop_id: None,
                        iteration: None,
                    },
                ],
                loops: vec![LoopRecord {
                    id: 0,
                    scope: "OnRun".into(),
                    statement: 10,
                    parent: None,
                    parent_iteration: None,
                    iterations: vec![vec![11], vec![11, 12]],
                    closed_by: ClosedBy::Exit,
                }],
                messages: vec![MessageRecord {
                    text: "hi".into(),
                    scope: "OnRun".into(),
                    statement: Some(12),
                    loop_id: Some(0),
                    iteration: Some(2),
                }],
            },
            locations,
        }
    }

    #[test]
    fn values_loops_and_messages_have_the_shape_of_v2_and_ids_start_at_the_base() {
        let both = CaptureConfig {
            values: true,
            loops: true,
        };
        let j = json(&sample(), both, 7, &labels);
        assert_eq!(
            j["capturedValues"][0],
            json!({"scopeName":"OnRun","variableName":"x","value":1.5,"statementId":11,"loop":7,"iteration":2})
        );
        assert_eq!(
            j["capturedValues"][1],
            json!({"scopeName":"OnRun","variableName":"l","value":null,"statementId":12,"captureError":"a List is not rendered yet"})
        );
        let l = &j["loops"][0];
        assert_eq!(
            (
                l["id"].clone(),
                l["file"].clone(),
                l["line"].clone(),
                l["iterationCount"].clone(),
                l["closedBy"].clone()
            ),
            (json!(7), json!("/w/a.al"), json!(3), json!(2), json!("exit"))
        );
        assert_eq!(
            l["iterations"][1],
            json!({"index": 2, "statements": [11, 12], "lines": [4, 5]})
        );
        assert_eq!(
            j["messages"][0],
            json!({"text":"hi","scopeName":"OnRun","statementId":12,"loop":7,"iteration":2})
        );
    }

    #[test]
    fn only_what_was_asked_for_is_there() {
        let values = json(
            &sample(),
            CaptureConfig {
                values: true,
                loops: false,
            },
            0,
            &labels,
        );
        assert!(
            values.get("capturedValues").is_some() && values.get("loops").is_none() && values.get("messages").is_none()
        );
        let loops = json(
            &sample(),
            CaptureConfig {
                values: false,
                loops: true,
            },
            0,
            &labels,
        );
        assert!(loops.get("loops").is_some() && loops.get("capturedValues").is_none());
    }
}
