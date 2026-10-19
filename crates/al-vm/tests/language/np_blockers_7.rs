fn text(vars: &str, body: &str) -> String {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    match al_testkit::run_in(&src, "C", "F") {
        Ok(al_builtins::value::Value::Text(t) | al_builtins::value::Value::Code(t)) => t.to_string(),
        Err(al_builtins::native::Signal::Error(e)) => format!("error:{}|{}", e.message, e.code),
        Err(al_builtins::native::Signal::Unsupported(w)) => format!("unsupported:{w}"),
        other => format!("{other:?}"),
    }
}

// types/lt-af-char-literal/AfCharLiteralArithmetic.al:22-58: a one-character literal next to a Char is a Char; the
// sum or difference is an Integer.
#[test]
fn a_char_plus_or_minus_a_one_character_literal_is_an_integer() {
    for (expr, want) in [
        ("Ch - 'A'", "2"),
        ("'A' - Ch", "-2"),
        ("Ch + 'A'", "132"),
        ("'A' + Ch", "132"),
        ("Ch - 'a'", "-30"),
        ("Ch - 'A' + 1", "3"),
        ("Ch - 'A' = 2", "Yes"),
        ("Ch + 'AB'", "CAB"),
        ("Ch + ''", "C"),
        ("'AB' + Ch", "ABC"),
    ] {
        let r = text("Ch: Char;", &format!("Ch := 'C'; exit(Format({expr}));"));
        assert_eq!(r, want, "{expr}");
    }
    let r = text("Ch: Char; I: Integer;", "Ch := 'C'; I := Ch - 'A'; exit(Format(I));");
    assert_eq!(r, "2");
}

fn table_text(vars: &str, body: &str) -> String {
    let src = format!(
        "table 50101 T {{ fields {{ field(1; Id; Integer) {{ }} field(2; Cd; Code[10]) {{ }} field(3; Dt; Date) {{ }} field(4; Dec; Decimal) {{ }} field(9; Dt2; DateTime) {{ }} }} keys {{ key(PK; Id) {{ }} }} }}
         codeunit 50100 C {{ [TryFunction] procedure TryEval(F: FieldRef; T: Text) begin Evaluate(F, T); end;
         procedure F(): Text var {vars} begin {body} end; }}"
    );
    match al_testkit::run_in(&src, "C", "F") {
        Ok(al_builtins::value::Value::Text(t) | al_builtins::value::Value::Code(t)) => t.to_string(),
        Err(al_builtins::native::Signal::Error(e)) => format!("error:{}|{}", e.message, e.code),
        Err(al_builtins::native::Signal::Unsupported(w)) => format!("unsupported:{w}"),
        other => format!("{other:?}"),
    }
}

// errors/lt-ms-refusals-128/N582EvalFieldRefUnused.al:84-91, 125-129: the statement form raises the error of the type and keeps the field.
#[test]
fn evaluate_into_a_fieldref_as_a_statement_raises_and_keeps_the_field() {
    let body = |no: i32, input: &str| {
        format!(
            "R.Dec := 9; R.Cd := 'KEEP'; RR.GetTable(R); F := RR.Field({no});
             if TryEval(F, '{input}') then exit('no error|' + Format(F.Value)); exit(GetLastErrorText() + '|' + GetLastErrorCode() + '|' + Format(F.Value));"
        )
    };
    let vars = "R: Record T; RR: RecordRef; F: FieldRef;";
    assert_eq!(
        table_text(vars, &body(4, "abc")),
        "The value \"abc\" can't be evaluated into type Decimal.|Evaluate|9"
    );
    assert_eq!(table_text(vars, &body(4, "12.5")), "no error|12.5");
    assert_eq!(
        table_text(vars, &body(2, "ABCDEFGHIJKLMNOP")),
        "The length of the string is 16, but it must be less than or equal to 10 characters. Value: ABCDEFGHIJKLMNOP|StringLengthExceeded|KEEP"
    );
    assert_eq!(table_text(vars, &body(2, "abc")), "no error|ABC");
    assert_eq!(
        table_text(vars, &body(3, "abc")),
        "The format of the Date 'abc' does not match your device's date settings.\\\\Make sure that the day, month, and year are in the correct order and use the correct separator (such as - or /). Alternatively, you can always use the format 'yyyy-mm-dd'.|Evaluate|"
    );
    // A DateTime field is not pinned.
    assert!(table_text(vars, &body(9, "abc")).starts_with("unsupported:Evaluate into a FieldRef of type"));
}
