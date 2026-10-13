//! Records end to end: AL source → compile → run against the in-memory store.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 50900 "ALT T"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { InitValue = 5; trigger OnValidate() begin Log += 'V' + Format(xRec.Amount) + '>' + Format(Amount); end; }
        field(3; Grp; Integer) { }
        field(4; Log; Text[250]) { }
    }
    keys { key(PK; "No.") { } key(ByGrp; Grp, "No.") { } }

    trigger OnInsert() begin Log += 'I'; end;
    trigger OnModify() begin Log += 'M'; end;
    trigger OnDelete() begin if "No." = 'X' then Error('cannot delete X'); end;

    procedure Bump(N: Decimal) begin Amount += N; end;
}
"#;

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ {body} }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == routine)
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        panic!("{routine} is blocked: {r}");
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn insert_get_count_deleteall() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'a'; R.Insert();
            R.\"No.\" := 'b'; R.Insert();
            Clear(R);
            R.Get('A');
            exit(Format(R.Count()) + R.\"No.\" + Format(R.IsEmpty()));
        end;",
        "F",
    );
    assert_eq!(text(r), "2ANo");
}

#[test]
fn init_applies_initvalue_and_keeps_the_primary_key() {
    // Doc (Record.Init): primary key fields are not initialized.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'K'; R.Amount := 9; R.Init(); exit(R.\"No.\" + Format(R.Amount)); end;",
        "F",
    );
    assert_eq!(text(r), "K5");
}

#[test]
fn duplicate_insert_raises_or_returns_false() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); asserterror R.Insert(); exit(GetLastErrorText()); end;",
        "F",
    );
    assert!(text(r).contains("already exists"));
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); exit(R.Insert()); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Boolean(false))));
}

#[test]
fn get_missing_raises_or_returns_false() {
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin exit(R.Get('nope')); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Boolean(false))));
    let r = run("procedure F() var R: Record \"ALT T\"; begin R.Get('nope'); end;", "F");
    assert!(matches!(r, Err(Signal::Error(e)) if e.message.contains("does not exist")));
}

#[test]
fn modify_and_delete() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'a'; R.Insert();
            R.Amount := 7; R.Modify();
            Clear(R); R.Get('A');
            R.Delete();
            exit(Format(R.Get('A')) + Format(R.Amount));
        end;",
        "F",
    );
    // Get fails (returns No) and leaves the buffer as it was (Amount 7).
    assert_eq!(text(r), "No7");
}

#[test]
fn find_and_next_follow_the_current_key() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; S: Text; begin
            R.\"No.\" := 'C'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'A'; R.Grp := 2; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 1; R.Insert();
            if R.FindSet() then repeat S += R.\"No.\"; until R.Next() = 0;
            S += '|';
            R.SetCurrentKey(Grp);
            if R.FindSet() then repeat S += R.\"No.\"; until R.Next() = 0;
            S += '|';
            if R.FindLast() then S += R.\"No.\";
            exit(S);
        end;",
        "F",
    );
    assert_eq!(text(r), "ABC|BCA|A");
}

#[test]
fn setrange_filters_and_reset() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 2; R.Insert();
            R.\"No.\" := 'C'; R.Grp := 3; R.Insert();
            R.SetRange(Grp, 2, 3);
            exit(Format(R.Count()) + Format(R.FindFirst()) + R.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "2YesB");
    let r = run(
        "procedure F(): Integer var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.SetRange(Grp, 9); R.Reset(); exit(R.Count()); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Integer(1))));
}

#[test]
fn deleteall_respects_filters() {
    let r = run(
        "procedure F(): Integer var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 2; R.Insert();
            R.SetRange(Grp, 2); R.DeleteAll(); R.Reset();
            exit(R.Count());
        end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Integer(1))));
}

#[test]
fn triggers_run_only_when_asked() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert(true);
            R.Modify(true);
            R.\"No.\" := 'B'; R.Insert(false);
            exit(R.Log);
        end;",
        "F",
    );
    // OnInsert then OnModify ran on A's buffer; B's insert ran no trigger.
    assert_eq!(text(r), "IM");
}

#[test]
fn trigger_errors_propagate() {
    let r = run(
        "procedure F() var R: Record \"ALT T\"; begin R.\"No.\" := 'X'; R.Insert(); R.Delete(true); end;",
        "F",
    );
    assert!(matches!(r, Err(Signal::Error(e)) if e.message == "cannot delete X"));
}

#[test]
fn validate_runs_onvalidate_with_xrec() {
    // xRec.Amount is 5: InitValue applies at variable declaration
    // (record/TestInitValueContracts.al:204).
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.Validate(Amount, 8); exit(R.Log); end;",
        "F",
    );
    assert_eq!(text(r), "V5>8");
}

#[test]
fn assignment_copies_fields_but_not_filters() {
    let r = run(
        "procedure F(): Text var A: Record \"ALT T\"; B: Record \"ALT T\"; begin
            A.\"No.\" := 'A'; A.Insert();
            A.\"No.\" := 'B'; A.Insert();
            A.SetRange(\"No.\", 'A');
            B := A;
            B.\"No.\" := 'Z';
            exit(A.\"No.\" + B.\"No.\" + Format(B.Count()));
        end;",
        "F",
    );
    assert_eq!(text(r), "BZ2");
}

#[test]
fn record_parameters_by_value_and_by_var() {
    let r = run(
        "local procedure ByVal(R: Record \"ALT T\") begin R.\"No.\" := 'V'; end;
         local procedure ByVar(var R: Record \"ALT T\") begin R.\"No.\" := 'R'; end;
         procedure F(): Text var R: Record \"ALT T\"; S: Text; begin R.\"No.\" := 'A'; ByVal(R); S := R.\"No.\"; ByVar(R); exit(S + R.\"No.\"); end;",
        "F",
    );
    assert_eq!(text(r), "AR");
}

#[test]
fn table_procedures_run_on_the_receiver() {
    let r = run(
        "procedure F(): Decimal var R: Record \"ALT T\"; begin R.Bump(2); R.Bump(3); exit(R.Amount); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Decimal(d)) if d == 10.into()), "{r:?}");
}

#[test]
fn asserterror_rolls_back_to_the_last_commit() {
    // record/TestRecordRenameRollback.al: an error caught by asserterror rolls back to the last commit point.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert(); Commit();
            asserterror begin R.\"No.\" := 'B'; R.Insert(); Error('x'); end;
            exit(Format(R.Get('A')) + Format(R.Get('B')));
        end;",
        "F",
    );
    assert_eq!(text(r), "YesNo");
}

#[test]
fn temporary_records_have_their_own_table() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; T: Record \"ALT T\" temporary; begin
            T.\"No.\" := 'A'; T.Insert();
            exit(Format(T.Count()) + Format(R.Count()));
        end;",
        "F",
    );
    assert_eq!(text(r), "10");
}

#[test]
fn find_without_argument_is_find_equal_by_primary_key() {
    // Doc (Record.Find): Which defaults to '='.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.Grp := 7;
            exit(Format(R.Find()) + Format(R.Grp));
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes1");
}

#[test]
fn find_equal_with_filters_is_not_guessed() {
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.SetRange(Grp, 1); exit(R.Find()); end;",
        "F",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))));
}

#[test]
fn date_and_time_init_values() {
    let src = "table 50910 D { fields { field(1; K; Integer) { } field(2; Dt; Date) { InitValue = 20240115D; } field(3; Tm; Time) { InitValue = 120000T; } } }\n";
    let body =
        "procedure F(): Text var R: Record D; begin exit(Format(R.Dt, 0, 9) + ' ' + Format(R.Tm = 120000T)); end;";
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), format!("{src}codeunit 50100 C {{ {body} }}"))],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty());
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert_eq!(text(s.call(oi, 0, inst, vec![]).map(|(v, _)| v)), "2024-01-15 Yes");
}

// Was `rename_of_a_table_other_fields_relate_to_is_unsupported`: the refusal stood in for rename
// propagation until it was implemented (tests/rename.rs holds the full set); the same scenario now
// follows the relation (corpus record/TestRenamePropagation.al:20-40).
#[test]
fn rename_of_a_table_other_fields_relate_to_rewrites_them() {
    let src = "table 50911 P { fields { field(1; K; Code[10]) { } } }\ntable 50912 Ch { fields { field(1; Id; Integer) { } field(2; Parent; Code[10]) { TableRelation = P.K; } } }\n";
    let body = "procedure F(): Text var R: Record P; C: Record Ch; begin R.K := 'A'; R.Insert(); C.Id := 1; C.Parent := 'A'; C.Insert(); R.Rename('B'); C.Get(1); exit(C.Parent); end;";
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), format!("{src}codeunit 50100 C {{ {body} }}"))],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert_eq!(text(s.call(oi, 0, inst, vec![]).map(|(v, _)| v)), "B");
}

#[test]
fn setfilter_with_expressions_and_placeholders() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'AP1'; R.Grp := 5; R.Insert();
            R.\"No.\" := 'AP2'; R.Grp := 15; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 25; R.Insert();
            R.SetFilter(\"No.\", 'ap*');
            R.SetFilter(Grp, '>%1', 10);
            exit(Format(R.Count()) + Format(R.FindFirst()) + R.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "1YesAP2");
}

#[test]
fn manual_subscribers_that_nothing_binds_do_not_block_writes() {
    // corpus _fixtures/events/ALTManualTableEventSubscriber.al: a Manual subscriber only fires while bound.
    let src = format!("{TABLE}\ncodeunit 50101 Sub {{ EventSubscriberInstance = Manual;
        [EventSubscriber(ObjectType::Table, Database::\"ALT T\", 'OnAfterInsertEvent', '', false, false)]
        local procedure OnAfterInsert(var Rec: Record \"ALT T\") begin Error('must not fire'); end; }}
        codeunit 50100 C {{ procedure F(): Integer var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); exit(R.Count()); end; }}");
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert!(matches!(s.call(oi, 0, inst, vec![]), Ok((Value::Integer(1), _))));
}

#[test]
fn extension_field_triggers_do_not_block_other_writes() {
    let src = format!("{TABLE}\ntableextension 50102 X extends \"ALT T\" {{ fields {{ field(50000; Ext; Integer) {{ trigger OnValidate() begin Log := 'ext'; end; }} }} }}
        codeunit 50100 C {{ procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.Validate(Ext, 1); R.DeleteAll(); exit(R.Log + Format(R.Count())); end; }}");
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert_eq!(text(s.call(oi, 0, inst, vec![]).map(|(v, _)| v)), "ext0");
}

#[test]
fn writes_assign_increasing_row_versions() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; V1: BigInteger; V2: BigInteger; begin
            R.\"No.\" := 'A'; R.Insert(); R.Get('A'); V1 := R.SystemRowVersion;
            R.Grp := 1; R.Modify(); R.Get('A'); V2 := R.SystemRowVersion;
            exit(Format(V1 > 0) + Format(V2 > V1));
        end;",
        "F",
    );
    assert_eq!(text(r), "YesYes");
}
