//! End to end: single builtins that need the compiled program (enum metadata) or session state.
//! Each test names the corpus test that pins the behaviour it asserts.

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

const STATUS: &str = "enum 50101 \"ALT Status\" { value(0; \" \") { Caption = ' '; } value(1; Draft) { Caption = 'Draft'; } value(5; Closed) { Caption = 'Closed'; } }\n";

/// Runs `procedure F(): Integer var <vars> begin <body> end;` next to the enum.
fn int_of(vars: &str, body: &str) -> Result<i32, Signal> {
    let src = format!("{STATUS}codeunit 50100 C {{ procedure F(): Integer var {vars} begin {body} end; }}");
    match run(&src, "F")? {
        Value::Integer(i) => Ok(i),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- Enum

#[test]
fn enum_names_lists_every_declared_name() {
    // types/TestEnumNamesContracts.al:19-60: `"ALT Status".Names()` holds the names, blank ordinal included.
    let v = "N: List of [Text];";
    assert_eq!(int_of(v, "N := \"ALT Status\".Names(); exit(N.Count());").unwrap(), 3);
    assert_eq!(
        int_of(
            v,
            "N := \"ALT Status\".Names(); if N.Contains('Draft') and N.Contains('Closed') then exit(1); exit(0);"
        )
        .unwrap(),
        1
    );
}

#[test]
fn enum_names_works_on_a_variable() {
    // types/TestEnumNamesContracts.al:77-85 (`S.Names()`).
    let v = "S: Enum \"ALT Status\"; N: List of [Text];";
    assert_eq!(
        int_of(v, "S := \"ALT Status\"::Draft; N := S.Names(); exit(N.Count());").unwrap(),
        3
    );
}

#[test]
fn enum_ordinals_lists_every_declared_value() {
    // session/TestSystemExtended.al:381-390 and types/TestLanguageEdgeCases.al:91-105.
    let v = "O: List of [Integer];";
    assert_eq!(
        int_of(v, "O := \"ALT Status\".Ordinals(); exit(O.Count());").unwrap(),
        3
    );
    assert_eq!(int_of(v, "O := \"ALT Status\".Ordinals(); if O.Contains(0) and O.Contains(1) and O.Contains(5) then exit(1); exit(0);").unwrap(), 1);
}

#[test]
fn names_and_ordinals_line_up() {
    // types/TestEnumNamesContracts.al:62-70: equal counts; the Nth name belongs to the Nth ordinal.
    let v = "N: List of [Text]; O: List of [Integer];";
    assert_eq!(
        int_of(v, "N := \"ALT Status\".Names(); O := \"ALT Status\".Ordinals(); if (N.Get(2) = 'Draft') and (O.Get(2) = 1) then exit(1); exit(0);").unwrap(),
        1
    );
}

#[test]
fn enum_as_integer_returns_the_ordinal() {
    // types/TestEnum.al:27-40.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(
        int_of(v, "S := \"ALT Status\"::Closed; exit(S.AsInteger());").unwrap(),
        5
    );
    assert_eq!(int_of(v, "exit(S.AsInteger());").unwrap(), 0);
}

#[test]
fn enum_from_integer_carries_any_ordinal() {
    // types/TestEnum.al:15-22 (a declared ordinal), :83-91 and types/TestEnumDefaultImplTests.Codeunit.al:70
    // (an undeclared one keeps its number); types/TestEnumShippedOrdinalTests.Codeunit.al:91 spells the
    // receiver `Enum::"Name"`.
    let v = "S: Enum \"ALT Status\";";
    assert_eq!(
        int_of(
            v,
            "S := \"ALT Status\".FromInteger(5); if S = \"ALT Status\"::Closed then exit(1); exit(0);"
        )
        .unwrap(),
        1
    );
    assert_eq!(
        int_of(v, "S := \"ALT Status\".FromInteger(99); exit(S.AsInteger());").unwrap(),
        99
    );
    assert_eq!(
        int_of(v, "S := Enum::\"ALT Status\".FromInteger(1); exit(S.AsInteger());").unwrap(),
        1
    );
}

// ---------------------------------------------------------------- NumberSequence

fn text_of(vars: &str, body: &str) -> Result<String, Signal> {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    match run(&src, "F")? {
        Value::Text(s) | Value::Code(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn number_sequence_honours_seed_and_increment_per_scope() {
    // session/TestNumberSequence.al:15-60: the default scope is company specific, a global one is
    // independent, and Next starts at the seed and advances by the increment.
    let r = text_of(
        "a: Text;",
        "NumberSequence.Insert('S', 5, 2);
         a := Format(NumberSequence.Exists('S', true)) + Format(NumberSequence.Exists('S', false));
         a += '|' + Format(NumberSequence.Next('S')) + ',' + Format(NumberSequence.Next('S'));
         NumberSequence.Insert('S', 100, 10, false);
         a += '|' + Format(NumberSequence.Next('S', false)) + ',' + Format(NumberSequence.Next('S', true));
         exit(a);",
    );
    assert_eq!(r.unwrap(), "YesNo|5,7|100,9");
}

#[test]
fn number_sequence_current_restart_range_and_delete() {
    // session/TestNumberSequence.al:63-170.
    let r = text_of(
        "a: Text; i: BigInteger; first: BigInteger;",
        "NumberSequence.Insert('S', 10, 3, false);
         a := Format(NumberSequence.Current('S', false));
         first := NumberSequence.Range('S', 4, false);
         a += '|' + Format(first) + ',' + Format(NumberSequence.Current('S', false)) + ',' + Format(NumberSequence.Next('S', false));
         NumberSequence.Restart('S', 50, false);
         a += '|' + Format(NumberSequence.Next('S', false)) + ',' + Format(NumberSequence.Next('S', false));
         first := NumberSequence.Range('S', 2, i, false);
         a += '|' + Format(first) + ',' + Format(i);
         NumberSequence.Delete('S', false);
         exit(a + '|' + Format(NumberSequence.Exists('S', false)));",
    );
    assert_eq!(r.unwrap(), "10|10,19,22|50,53|56,3|No");
}

#[test]
fn number_sequence_errors_are_catchable_and_a_missing_delete_is_not_one() {
    // session/TestNumberSequence.al:173-205 and error-handling/TestNumberSequenceTrappableErrors.al.
    let r = text_of(
        "a: Text; v: BigInteger;",
        "NumberSequence.Insert('S', 1, 1, false);
         asserterror NumberSequence.Insert('S', 100, 10, false);
         a := GetLastErrorText();
         NumberSequence.Delete('S', false);
         NumberSequence.Delete('S', false);
         asserterror v := NumberSequence.Next('S', false);
         a += '|' + GetLastErrorText();
         asserterror v := NumberSequence.Current('S', false);
         a += '|' + GetLastErrorText();
         asserterror NumberSequence.Restart('S', 1, false);
         a += '|' + GetLastErrorText();
         asserterror v := NumberSequence.Range('S', 3, false);
         a += '|' + GetLastErrorText();
         exit(a + '|' + Format(NumberSequence.Exists('S', false)));",
    );
    let r = r.unwrap();
    let parts: Vec<&str> = r.split('|').collect();
    assert!(parts[0].contains("already exists"), "{r}");
    assert!(parts[1..=4].iter().all(|p| p.contains("does not exist")), "{r}");
    assert_eq!(parts[5], "No");
}

#[test]
fn number_sequence_survives_an_asserterror_rollback() {
    // session/TestNumberSequence.al:173-186 and :188-201: a failed call leaves the sequence as it
    // was; error-handling/TestNumberSequenceTrappableErrors.al:134 ("Number sequences are
    // database objects, not rows"): a sequence outlives the rollback an asserterror triggers.
    let r = text_of(
        "a: Text;",
        "NumberSequence.Insert('S', 1, 1, false);
         asserterror Error('x');
         exit(Format(NumberSequence.Exists('S', false)));",
    );
    assert_eq!(r.unwrap(), "Yes");
}

#[test]
fn number_sequence_forms_the_corpus_does_not_pin_are_refused() {
    let r = text_of("", "NumberSequence.Insert('S'); exit('');");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
    let r = text_of(
        "",
        "NumberSequence.Insert('S', 1, 1); NumberSequence.Restart('S'); exit('');",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

// ---------------------------------------------------------------- Database.IsInWriteTransaction

const ROWS: &str = "table 50900 \"ALT Row\" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }\n";

fn text_with_table(body: &str) -> Result<String, Signal> {
    let src = format!("{ROWS}codeunit 50100 C {{ procedure F(): Text var R: Record \"ALT Row\"; begin {body} end; }}");
    match run(&src, "F")? {
        Value::Text(s) => Ok(s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_write_opens_a_write_transaction_until_the_commit() {
    // codeunit/TestWriteTxTestBoundary.al:93,103,142: an uncommitted Insert leaves one open; a
    // Commit (and the test boundary) closes it.
    let r = text_with_table(
        "exit(Format(Database.IsInWriteTransaction()));
         ",
    );
    assert_eq!(r.unwrap(), "No");
    let r = text_with_table(
        "R.Id := 1; R.Insert();
         exit(Format(Database.IsInWriteTransaction()));",
    );
    assert_eq!(r.unwrap(), "Yes");
    let r = text_with_table(
        "R.Id := 1; R.Insert(); Commit();
         exit(Format(Database.IsInWriteTransaction()));",
    );
    assert_eq!(r.unwrap(), "No");
}

// ---------------------------------------------------------------- WebServiceActionContext

fn wsac(body: &str) -> Result<String, Signal> {
    text_of("c: WebServiceActionContext; d: WebServiceActionContext; a: Text; b: Boolean; v: Variant;", body)
}

#[test]
fn wsac_fresh_instance_and_round_trips() {
    // session/TestWebServiceActionContext.al:180-250 (defaults) and :256-345 (round trips).
    let r = wsac(
        "a := Format(c.GetObjectId()) + '|' + Format(c.GetResultCode());
         c.SetObjectId(50123); c.SetObjectType(ObjectType::Page); c.SetResultCode(WebServiceActionResultCode::Created);
         a += '|' + Format(c.GetObjectId()) + ',' + Format(c.GetResultCode());
         if c.GetObjectType() = ObjectType::Page then a += ',page';
         c.SetObjectId(-7);
         exit(a + '|' + Format(c.GetObjectId()));",
    );
    assert_eq!(r.unwrap(), "0|None|50123,Created,page|-7");
}

#[test]
fn wsac_a_fresh_object_type_is_none_of_the_seven_members() {
    // session/TestWebServiceActionContext.al:187-200 and :411-428 (Clear returns it to the pristine value).
    let r = wsac(
        "if (c.GetObjectType() <> ObjectType::Table) and (c.GetObjectType() <> ObjectType::Page) and (c.GetObjectType() <> ObjectType::Report)
            and (c.GetObjectType() <> ObjectType::Codeunit) and (c.GetObjectType() <> ObjectType::XmlPort)
            and (c.GetObjectType() <> ObjectType::Query) and (c.GetObjectType() <> ObjectType::MenuSuite) then a := 'none';
         c.SetObjectType(ObjectType::Page);
         Clear(c);
         if c.GetObjectType() = d.GetObjectType() then a += ',pristine';
         exit(a);",
    );
    assert_eq!(r.unwrap(), "none,pristine");
}

#[test]
fn wsac_updated_reads_back_as_get() {
    // session/TestWebServiceActionContext.al:352-395: Get and Updated share the value 200.
    let r = wsac(
        "c.SetResultCode(WebServiceActionResultCode::Updated); a := Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::Get); a += ',' + Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::Deleted); a += ',' + Format(c.GetResultCode());
         c.SetResultCode(WebServiceActionResultCode::None); a += ',' + Format(c.GetResultCode());
         exit(a);",
    );
    assert_eq!(r.unwrap(), "Get,Get,Deleted,None");
}

#[test]
fn wsac_entity_keys_are_a_map_by_field_id() {
    // session/TestWebServiceActionContext.al:437-520: a duplicate field id returns false when the
    // result is used and raises (naming the type) when it is not; Clear empties the map.
    let r = wsac(
        "a := Format(c.AddEntityKey(1, 'x')) + Format(c.AddEntityKey(2, 42)) + Format(c.AddEntityKey(1, 'y')) + Format(c.AddEntityKey(1, 'x'));
         asserterror c.AddEntityKey(1, 'z');
         a += '|' + Format(StrPos(GetLastErrorText(), 'WebServiceActionContext') > 0);
         Clear(c);
         exit(a + '|' + Format(c.AddEntityKey(1, 'again')));",
    );
    assert_eq!(r.unwrap(), "YesYesNoNo|Yes|Yes");
}

#[test]
fn wsac_assignment_shares_the_context_and_clear_replaces_it() {
    // session/TestWebServiceActionContext.al:552-590: `Copy := Original` shares; the keys too.
    let r = wsac(
        "c.SetObjectId(50123); d := c; d.SetObjectId(50124); a := Format(c.GetObjectId());
         b := c.AddEntityKey(1, 'k');
         a += ',' + Format(d.AddEntityKey(1, 'k'));
         Clear(d);
         exit(a + ',' + Format(c.GetObjectId()) + ',' + Format(d.GetObjectId()));",
    );
    assert_eq!(r.unwrap(), "50124,No,50124,0");
}

#[test]
fn wsac_in_a_variant_has_an_empty_text() {
    // session/TestWebServiceActionContext.al:615-650.
    let r = wsac(
        "c.SetObjectId(50123); v := c;
         exit(Format(v.IsInteger()) + Format(v.IsText()) + Format(v.IsRecord()) + '|' + Format(v));",
    );
    assert_eq!(r.unwrap(), "NoNoNo|");
}
