//! End to end: JSON, XML, Evaluate and in-memory streams. Each test names the
//! corpus test that pins the behaviour it asserts.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src.into())],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.routines.iter().any(|r| r.name == routine))
        .expect("object");
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn cu(body: &str) -> String {
    format!("codeunit 50100 C {{ {body} }}")
}

/// Runs `procedure F(): Text` with the given locals and body.
fn text_of(vars: &str, body: &str) -> String {
    let src = cu(&format!("procedure F(): Text var {vars} begin {body} end;"));
    match run(&src, "F") {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn unsupported(vars: &str, body: &str) -> bool {
    let src = cu(&format!("procedure F(): Text var {vars} begin {body} end;"));
    matches!(run(&src, "F"), Err(Signal::Unsupported(_)))
}

// ---------------------------------------------------------------- JSON

#[test]
fn json_object_add_get_as_value() {
    // json/TestJsonObject.al: JsonObject_Add_Get_TextValue / IntegerValue / BooleanValue.
    let t = text_of(
        "J: JsonObject; T: JsonToken;",
        "J.Add('key', 'hello'); J.Add('num', 42); J.Add('flag', true);
         J.Get('key', T); exit(T.AsValue().AsText());",
    );
    assert_eq!(t, "hello");
    let t = text_of(
        "J: JsonObject; T: JsonToken;",
        "J.Add('num', 42); J.Get('num', T); exit(Format(T.AsValue().AsInteger()));",
    );
    assert_eq!(t, "42");
    let t = text_of(
        "J: JsonObject; T: JsonToken;",
        "J.Add('flag', true); J.Get('flag', T); exit(Format(T.AsValue().AsBoolean()));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn json_object_contains_get_remove() {
    // TestJsonObject.al: Contains_ExistingKey / MissingKey, Get_MissingKey_ReturnsFalse, Remove_DeletesKey.
    let t = text_of(
        "J: JsonObject; T: JsonToken; Ok: Boolean;",
        "J.Add('x', 1); Ok := J.Get('nothere', T);
         exit(Format(J.Contains('x')) + Format(J.Contains('missing')) + Format(Ok));",
    );
    assert_eq!(t, "YesNoNo");
    let t = text_of(
        "J: JsonObject;",
        "J.Add('a', 1); J.Remove('a'); exit(Format(J.Contains('a')));",
    );
    assert_eq!(t, "No");
}

#[test]
fn json_object_keys_are_case_sensitive() {
    // json/TestJsonXmlDeepContracts.al: JsonObject_Keys_AreCaseSensitive.
    let t = text_of(
        "J: JsonObject; T: JsonToken;",
        "J.Add('key', 'lower'); J.Add('KEY', 'upper');
         J.Get('key', T); exit(T.AsValue().AsText() + '|' + Format(J.Contains('KEY')));",
    );
    assert_eq!(t, "lower|Yes");
}

#[test]
fn json_write_to_and_read_from() {
    // TestJsonXmlDeepContracts.al: empty object/array serialise to {} and [].
    assert_eq!(text_of("J: JsonObject; S: Text;", "J.WriteTo(S); exit(S);"), "{}");
    assert_eq!(text_of("J: JsonArray; S: Text;", "J.WriteTo(S); exit(S);"), "[]");
    // TestJsonObject.al: WriteTo contains "k"; ReadFrom parses.
    let t = text_of("J: JsonObject; S: Text;", "J.Add('k', 'v'); J.WriteTo(S); exit(S);");
    assert!(t.contains("\"k\""), "{t}");
    let t = text_of(
        "J: JsonObject; T: JsonToken;",
        "J.ReadFrom('{\"name\":\"Bob\",\"age\":25}'); J.Get('name', T); exit(T.AsValue().AsText());",
    );
    assert_eq!(t, "Bob");
}

#[test]
fn json_array_basics() {
    // json/TestJsonArray.al: Count, Get by zero-based index, ReadFrom, nested object.
    let t = text_of(
        "A: JsonArray; T: JsonToken;",
        "A.Add(10); A.Add(20); A.Get(1, T); exit(Format(A.Count()) + Format(T.AsValue().AsInteger()));",
    );
    assert_eq!(t, "220");
    let t = text_of(
        "A: JsonArray; T: JsonToken;",
        "A.ReadFrom('[\"a\",\"b\"]'); A.Get(0, T); exit(T.AsValue().AsText() + Format(A.Count()));",
    );
    assert_eq!(t, "a2");
    let t = text_of(
        "A: JsonArray; I: JsonObject;",
        "I.Add('id', 1); A.Add(I); exit(Format(A.Count()));",
    );
    assert_eq!(t, "1");
}

#[test]
fn json_tokens_share_and_classify() {
    // json/TestJsonToken.al + TestJsonTypedGetters.al: AsToken/AsObject/AsArray, Is*, nested Get.
    let t = text_of(
        "J: JsonObject; T: JsonToken; O: JsonObject; X: JsonToken;",
        "J.ReadFrom('{\"a\":1}'); T := J.AsToken(); O := T.AsObject(); O.Get('a', X);
         exit(Format(T.IsObject()) + Format(T.IsArray()) + Format(T.IsValue()) + Format(X.IsValue()) + Format(X.AsValue().AsInteger()));",
    );
    assert_eq!(t, "YesNoNoYes1");
    // JsonObject is a reference type: AsObject shares, so a write through O is visible in J.
    let t = text_of(
        "J: JsonObject; T: JsonToken; O: JsonObject;",
        "T := J.AsToken(); O := T.AsObject(); O.Add('late', 1); exit(Format(J.Contains('late')));",
    );
    assert_eq!(t, "Yes");
    let t = text_of(
        "J: JsonObject; A: JsonArray; T: JsonToken;",
        "A.Add(1); J.Add('items', A); J.Get('items', T); exit(Format(T.IsArray()));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn json_typed_getters() {
    // json/TestJsonTypedGetters.al and TestJsonComplete.al.
    let t = text_of(
        "J: JsonObject;",
        "J.ReadFrom('{\"t\":true,\"f\":false,\"n\":42,\"d\":2.5,\"s\":\"hello\",\"b\":1000000,\"o\":2}');
         exit(Format(J.GetBoolean('t')) + Format(J.GetBoolean('f')) + Format(J.GetInteger('n')) + Format(J.GetDecimal('d') > 2.0)
              + J.GetText('s') + Format(J.GetBigInteger('b')) + Format(J.GetOption('o')));",
    );
    assert_eq!(t, "YesNo42Yeshello10000002");
    let t = text_of(
        "J: JsonObject; Inner: JsonArray; O: JsonObject;",
        "J.ReadFrom('{\"arr\":[1,2,3],\"obj\":{\"x\":1}}'); Inner := J.GetArray('arr'); O := J.GetObject('obj');
         exit(Format(Inner.Count()) + Format(O.GetInteger('x')));",
    );
    assert_eq!(t, "31");
    let t = text_of(
        "A: JsonArray;",
        "A.ReadFrom('[true,false,20,1.5,\"apple\",999999]');
         exit(Format(A.GetBoolean(0)) + Format(A.GetBoolean(1)) + Format(A.GetInteger(2)) + Format(A.GetDecimal(3) > 1.0) + A.GetText(4) + Format(A.GetBigInteger(5)));",
    );
    assert_eq!(t, "YesNo20Yesapple999999");
}

#[test]
fn json_missing_key_throws_but_its_text_is_not_pinned() {
    // TestJsonTypedGetters.al JsonObject_GetInteger_MissingKey_Throws only requires a non-empty
    // error text; the corpus does not pin the text, so the runtime refuses rather than inventing one.
    assert!(unsupported(
        "J: JsonObject; R: Integer;",
        "J.ReadFrom('{\"other\":5}'); R := J.GetInteger('missing'); exit('');"
    ));
}

#[test]
fn json_value_set_and_read() {
    // json/TestJsonValue.al and TestJsonComplete.al.
    let t = text_of("V: JsonValue;", "V.SetValue('hello'); exit(V.AsText());");
    assert_eq!(t, "hello");
    let t = text_of(
        "V: JsonValue;",
        "V.SetValue(42); exit(Format(V.AsInteger()) + Format(V.AsBigInteger()) + Format(V.AsOption()) + Format(V.AsDuration() > 0));",
    );
    assert_eq!(t, "424242Yes");
    let t = text_of(
        "V: JsonValue;",
        "V.SetValueToNull(); exit(Format(V.IsNull()) + Format(V.IsUndefined()));",
    );
    assert_eq!(t, "YesNo");
    let t = text_of(
        "V: JsonValue;",
        "V.SetValueToUndefined(); exit(Format(V.IsUndefined()));",
    );
    assert_eq!(t, "Yes");
    let t = text_of(
        "V: JsonValue;",
        "V.SetValue(1); exit(Format(V.IsNull()) + Format(V.IsUndefined()));",
    );
    assert_eq!(t, "NoNo");
    let t = text_of(
        "V: JsonValue; C: Char;",
        "V.SetValue('A'); C := V.AsChar(); exit(Format(C));",
    );
    assert_eq!(t, "A");
    let t = text_of(
        "V: JsonValue; C: Code[100];",
        "V.SetValue('ITEM001'); C := V.AsCode(); exit(C);",
    );
    assert_eq!(t, "ITEM001");
}

#[test]
fn json_null_is_distinct_from_a_missing_key_and_round_trips() {
    // TestJsonXmlDeepContracts.al JsonObject_NullValue_vs_MissingKey_Distinct;
    // TestRoundtripContracts.al JsonObject_WriteTo_ReadFrom_PreservesNull.
    let t = text_of(
        "J: JsonObject; J2: JsonObject; V: JsonValue; T: JsonToken; S: Text;",
        "V.SetValueToNull(); J.Add('nullkey', V); J.WriteTo(S); J2.ReadFrom(S); J2.Get('nullkey', T);
         exit(Format(J.Contains('nullkey')) + Format(J.Contains('missing')) + Format(T.AsValue().IsNull()));",
    );
    assert_eq!(t, "YesNoYes");
}

#[test]
fn json_round_trips_text_numbers_booleans_and_structure() {
    // TestRoundtripContracts.al and TestJsonXmlDeepContracts.al.
    let t = text_of(
        "J: JsonObject; J2: JsonObject; T: JsonToken; S: Text;",
        "J.Add('text', 'Ünïcödé'); J.Add('big', 2000000000); J.Add('active', true); J.Add('deleted', false);
         J.WriteTo(S); J2.ReadFrom(S);
         exit(J2.GetText('text') + Format(J2.GetInteger('big')) + Format(J2.GetBoolean('active')) + Format(J2.GetBoolean('deleted')));",
    );
    assert_eq!(t, "Ünïcödé2000000000YesNo");
    // Escaped characters survive a round trip (JsonObject_EscapeChars_RoundTrip uses a newline,
    // which needs Format(10, 0, '<Integer>')[1]: not lowered yet).
    let t = text_of(
        "J: JsonObject; J2: JsonObject; S: Text; Orig: Text;",
        "Orig := 'quote\"s and \\ and ' + 'tab'; J.Add('content', Orig); J.WriteTo(S); J2.ReadFrom(S);
         exit(Format(J2.GetText('content') = Orig));",
    );
    assert_eq!(t, "Yes");
    let t = text_of(
        "A: JsonArray; A2: JsonArray; I1: JsonObject; I2: JsonObject; T: JsonToken; S: Text;",
        "I1.Add('id', 1); A.Add(I1); I2.Add('id', 2); A.Add(I2); A.WriteTo(S); A2.ReadFrom(S);
         A2.Get(1, T); exit(Format(A2.Count()) + Format(T.AsObject().GetInteger('id')));",
    );
    assert_eq!(t, "22");
}

#[test]
fn json_select_token_by_path() {
    // TestJsonComplete.al JsonToken_SelectToken_ViaPath; TestJsonTypedGetters.al
    // JsonToken_SelectToken_FindsNestedValue / FindsArrayElement; TestJsonXmlDeepContracts.al ReturnsCorrectType.
    let t = text_of(
        "J: JsonObject; T: JsonToken; S: JsonToken;",
        "J.ReadFrom('{\"a\":{\"b\":{\"c\":42}},\"items\":[100,200],\"arr\":[1],\"obj\":{\"k\":\"v\"}}'); T := J.AsToken();
         T.SelectToken('a.b.c', S); exit(Format(S.AsValue().AsInteger()));",
    );
    assert_eq!(t, "42");
    let t = text_of(
        "J: JsonObject; T: JsonToken; S: JsonToken;",
        "J.ReadFrom('{\"items\":[100,200]}'); T := J.AsToken(); T.SelectToken('items[0]', S); exit(Format(S.AsValue().AsInteger()));",
    );
    assert_eq!(t, "100");
    let t = text_of(
        "J: JsonObject; T: JsonToken; S: JsonToken;",
        "J.ReadFrom('{\"arr\":[1],\"obj\":{\"k\":\"v\"}}'); T := J.AsToken();
         T.SelectToken('arr', S); exit(Format(S.IsArray()) + Format(S.IsObject()));",
    );
    assert_eq!(t, "YesNo");
}

#[test]
fn json_token_path_and_clone() {
    // TestJsonTypedGetters.al JsonToken_Path_ReturnsPath: the path contains "nested".
    let t = text_of(
        "J: JsonObject; I: JsonObject; T: JsonToken;",
        "I.Add('id', 1); J.Add('nested', I); J.Get('nested', T); exit(T.Path());",
    );
    assert_eq!(t, "nested");
    // TestJsonToken.al JsonToken_Clone_CreatesIndependentCopy.
    let t = text_of(
        "J: JsonObject; T: JsonToken; C: JsonToken;",
        "J.Add('v', 42); J.Get('v', T); C := T.Clone(); exit(Format(C.AsValue().AsInteger()));",
    );
    assert_eq!(t, "42");
}

#[test]
fn json_dates_and_durations() {
    // TestJsonComplete.al: GetDate/GetDateTime/GetTime parse ISO text; GetDuration reads a number;
    // JsonArray.Add(Date) then GetDate returns the date.
    let t = text_of(
        "J: JsonObject; D: Date; DT: DateTime; T: Time; Dur: Duration;",
        "J.ReadFrom('{\"d\":\"2024-01-15\",\"dt\":\"2024-01-15T12:00:00\",\"t\":\"12:00:00\",\"dur\":3600000}');
         D := J.GetDate('d'); DT := J.GetDateTime('dt'); T := J.GetTime('t'); Dur := J.GetDuration('dur');
         exit(Format(D <> 0D) + Format(DT <> 0DT) + Format(T <> 0T) + Format(Dur > 0));",
    );
    assert_eq!(t, "YesYesYesYes");
    let t = text_of(
        "A: JsonArray; D: Date;",
        "A.Add(20240315D); D := A.GetDate(0); exit(Format(D = 20240315D));",
    );
    assert_eq!(t, "Yes");
}

#[test]
fn json_yaml_is_not_implemented() {
    // JsonObject.ReadFromYaml has no behaviour the corpus pins beyond "throws": refused, not faked.
    assert!(unsupported("J: JsonObject;", "J.ReadFromYaml('key: value'); exit('');"));
}

// ------------------------------------------------------------ Evaluate

#[test]
fn evaluate_numbers() {
    // text/TestEvaluate.al: Evaluate(I, '42') = 42; Evaluate(D, '3.14') > 3.0;
    // handlers/TestPageFieldFormats_Tests.al:753 Evaluate(BigInteger, '9000000000').
    let t = text_of(
        "I: Integer; D: Decimal; B: BigInteger;",
        "Evaluate(I, '42'); Evaluate(D, '3.14'); Evaluate(B, '9000000000'); exit(Format(I) + '|' + Format(D = 3.14) + '|' + Format(B));",
    );
    assert_eq!(t, "42|Yes|9000000000");
    let t = text_of("I: Integer;", "Evaluate(I, '-7'); exit(Format(I));");
    assert_eq!(t, "-7");
}

#[test]
fn evaluate_reports_failure_through_its_result() {
    // text/TestEvaluate.al: Evaluate_InvalidInteger_ReturnsFalse and Evaluate_EmptyString_ReturnsFalse;
    // json/TestRoundtripContracts.al Evaluate_InvalidText_ReturnsFalse (the variable is not asserted).
    let t = text_of(
        "I: Integer; R: Boolean; E: Boolean;",
        "R := Evaluate(I, 'notanumber'); E := Evaluate(I, ''); exit(Format(R) + Format(E));",
    );
    assert_eq!(t, "NoNo");
    let t = text_of(
        "I: Integer; R: Boolean;",
        "R := Evaluate(I, '12'); exit(Format(R) + Format(I));",
    );
    assert_eq!(t, "Yes12");
}

#[test]
fn evaluate_failure_without_using_the_result_is_refused_not_invented() {
    // The error text of a failed statement-form Evaluate on an Integer is not pinned by the corpus.
    assert!(unsupported("I: Integer;", "Evaluate(I, 'notanumber'); exit('');"));
}

#[test]
fn evaluate_booleans() {
    // text/TestEvaluate.al 'true' / 'false'; types/TestFormatBooleanContracts.al:112 evaluates Format(true) = 'Yes'.
    let t = text_of(
        "A: Boolean; B: Boolean; C: Boolean; D: Boolean;",
        "Evaluate(A, 'true'); Evaluate(B, 'false'); Evaluate(C, Format(true)); Evaluate(D, Format(false));
         exit(Format(A) + Format(B) + Format(C) + Format(D));",
    );
    assert_eq!(t, "YesNoYesNo");
}

#[test]
fn evaluate_dates_round_trip_through_format() {
    // text/TestEvaluate.al Evaluate_Date_ParsesDate; json/TestRoundtripContracts.al:408 (20240315D).
    let t = text_of(
        "D: Date; D2: Date; S: Text;",
        "D := 20240315D; S := Format(D); Evaluate(D2, S); exit(Format(D = D2) + S);",
    );
    assert!(t.starts_with("Yes"), "{t}");
    // Number 9 is the XML (ISO) form.
    let t = text_of("D: Date;", "Evaluate(D, '2024-03-15', 9); exit(Format(D = 20240315D));");
    assert_eq!(t, "Yes");
}

#[test]
fn evaluate_guid_and_datetime_xml_form() {
    // types/TestGuid.al:78 round trip; handlers/TestPageFieldFormats_Tests.al:1401 braces literal;
    // session/TestSessionEvaluateRegionalPatterns.al:38 DateTime with format number 9.
    let t = text_of(
        "G: Guid; G2: Guid;",
        "Evaluate(G, '{11111111-2222-3333-4444-555555555555}'); G2 := G; Evaluate(G2, Format(G)); exit(Format(G) + Format(G = G2));",
    );
    assert_eq!(t, "{11111111-2222-3333-4444-555555555555}Yes");
    let t = text_of(
        "Dt: DateTime; Ok: Boolean;",
        "Ok := Evaluate(Dt, '2026-01-02T10:11:12.0000000Z', 9); exit(Format(Ok) + Format(Dt = 0DT));",
    );
    assert_eq!(t, "YesNo");
}

#[test]
fn evaluate_of_forms_the_corpus_does_not_pin_is_refused() {
    assert!(unsupported("I: Integer;", "Evaluate(I, '1,000'); exit('');"));
    assert!(unsupported("B: Boolean;", "Evaluate(B, 'maybe'); exit('');"));
}
