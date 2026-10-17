use al_builtins::value::Value;

fn text(vars: &str, body: &str) -> String {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    match al_testkit::run_in(&src, "C", "F") {
        Ok(Value::Text(t) | Value::Code(t)) => t.to_string(),
        Err(al_builtins::native::Signal::Error(e)) => format!("error:{}|{}", e.message, e.code),
        other => format!("{other:?}"),
    }
}

#[test]
fn foreach_over_a_never_used_json_array_runs_zero_times() {
    // json/JsonArrayBasics.al:274-281 pins the cleared array; a never-used one is the same state.
    let r = text(
        "A: JsonArray; T: JsonToken; R: Text;",
        "foreach T in A do R += 'x'; exit('[' + R + ']');",
    );
    assert_eq!(r, "[]");
}

#[test]
fn foreach_over_a_json_array_copied_from_a_never_used_one() {
    let r = text(
        "A: JsonArray; B: JsonArray; T: JsonToken; R: Text;",
        "B := A; foreach T in B do R += 'x'; exit('[' + R + ']');",
    );
    assert_eq!(r, "[]");
}
