//! FlowFields, FlowFilters, CalcFields/CalcSums/SetAutoCalcFields and
//! AutoIncrement, end to end: AL source → compile → run on the in-memory store.
//! Each test cites the corpus file that pins the behaviour.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

/// The corpus fixtures ALT Parent / ALT Child (corpus/tests/al-language/_fixtures/tables/ALTParentChild.al).
const PARENT_CHILD: &str = r#"
table 60004 "ALT Parent"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Code"; Code[20]) { }
        field(3; "Child Count"; Integer) { FieldClass = FlowField; CalcFormula = count("ALT Child" where("Parent Entry No." = field("Entry No."))); }
        field(4; "Child Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum("ALT Child".Amount where("Parent Entry No." = field("Entry No."))); }
        field(5; "First Child Code"; Code[20]) { FieldClass = FlowField; CalcFormula = lookup("ALT Child".Code where("Parent Entry No." = field("Entry No."))); }
    }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}
table 60005 "ALT Child"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Parent Entry No."; Integer) { }
        field(3; "Code"; Code[20]) { }
        field(4; "Amount"; Decimal) { }
    }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}
"#;

fn run_with(tables: &str, body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{tables}\ncodeunit 50100 C {{ {body} }}");
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

fn run(body: &str) -> Result<Value, Signal> {
    run_with(PARENT_CHILD, body, "F")
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn unsupported(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Unsupported(s)) => s,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Seeds parent 1 with two children (30 and 50) and parent 2 with one (7).
const SEED: &str = "
    procedure Seed()
    var P: Record \"ALT Parent\"; C: Record \"ALT Child\";
    begin
        P.\"Entry No.\" := 1; P.Insert();
        P.\"Entry No.\" := 2; P.Insert();
        C.\"Entry No.\" := 10; C.\"Parent Entry No.\" := 1; C.Amount := 30; C.Code := 'FIRST'; C.Insert();
        C.\"Entry No.\" := 11; C.\"Parent Entry No.\" := 1; C.Amount := 50; C.Code := 'SECOND'; C.Insert();
        C.\"Entry No.\" := 12; C.\"Parent Entry No.\" := 2; C.Amount := 7; C.Code := 'ONLY'; C.Insert();
    end;
";

// corpus record/TestRecordFlowField.al:15,36,76 (count, sum, several fields in one call).
#[test]
fn calcfields_count_and_sum_over_the_children() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\";
        begin
            Seed();
            P.Get(1);
            P.CalcFields(\"Child Count\", \"Child Amount\");
            exit(Format(P.\"Child Count\") + '/' + Format(P.\"Child Amount\"));
        end;"
    ));
    assert_eq!(text(r), "2/80");
}

// corpus record/TestRecordFlowField.al:57 (lookup of the single child's code).
#[test]
fn lookup_returns_the_code_of_the_one_matching_row() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\";
        begin
            Seed();
            P.Get(2);
            P.CalcFields(\"First Child Code\", \"Child Count\");
            exit(P.\"First Child Code\" + Format(P.\"Child Count\"));
        end;"
    ));
    assert_eq!(text(r), "ONLY1");
}

// Which of several matching rows lookup() reads depends on the key BC picks;
// the corpus only ever looks up through one matching row (or rows that agree),
// so disagreeing rows are refused rather than guessed.
#[test]
fn lookup_over_rows_that_disagree_is_refused() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\";
        begin
            Seed();
            P.Get(1);
            P.CalcFields(\"First Child Code\");
            exit(P.\"First Child Code\");
        end;"
    ));
    assert!(unsupported(r).contains("lookup"));
}

// corpus record/TestFlowFieldContracts.al:79 (CalcFields returns true) and :98 (no children give 0).
#[test]
fn calcfields_returns_true_and_an_empty_source_gives_zero() {
    let r = run("procedure F(): Text
        var P: Record \"ALT Parent\"; Ok: Boolean;
        begin
            P.\"Entry No.\" := 1; P.Insert();
            P.Get(1);
            Ok := P.CalcFields(\"Child Count\", \"Child Amount\");
            exit(Format(Ok) + Format(P.\"Child Count\") + Format(P.\"Child Amount\"));
        end;");
    assert_eq!(text(r), "Yes00");
}

// corpus record/TestFlowFieldContracts.al:36 (a calculated value is a snapshot, stale until
// calculated again).
#[test]
fn a_calculated_value_is_stale_until_calculated_again() {
    let r = run("procedure F(): Text
        var P: Record \"ALT Parent\"; C: Record \"ALT Child\"; Seen: Text;
        begin
            P.\"Entry No.\" := 1; P.Insert();
            C.\"Entry No.\" := 1; C.\"Parent Entry No.\" := 1; C.Insert();
            P.Get(1);
            P.CalcFields(\"Child Count\");
            Seen := Format(P.\"Child Count\");
            C.\"Entry No.\" := 2; C.Insert();
            Seen += Format(P.\"Child Count\");
            P.CalcFields(\"Child Count\");
            exit(Seen + Format(P.\"Child Count\"));
        end;");
    assert_eq!(text(r), "112");
}

// corpus record/TestRecordNavContracts.al:200 (a record that was never inserted calculates to 0).
#[test]
fn calcfields_on_a_record_that_was_never_inserted_gives_zero() {
    let r = run("procedure F(): Text
        var P: Record \"ALT Parent\";
        begin
            P.\"Entry No.\" := 9999;
            P.CalcFields(\"Child Count\");
            exit(Format(P.\"Child Count\"));
        end;");
    assert_eq!(text(r), "0");
}

// corpus record/TestFlowFieldContracts.al:119,133 (FindFirst and Next calculate per record).
#[test]
fn set_auto_calc_fields_calculates_on_find_first_and_on_next() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\"; Seen: Text;
        begin
            Seed();
            P.SetAutoCalcFields(\"Child Count\", \"Child Amount\");
            P.FindFirst();
            Seen := Format(P.\"Child Count\") + ':' + Format(P.\"Child Amount\");
            P.Next();
            exit(Seen + '/' + Format(P.\"Child Count\") + ':' + Format(P.\"Child Amount\"));
        end;"
    ));
    assert_eq!(text(r), "2:80/1:7");
}

// corpus record/TestSetAutoCalcFieldsOnGet.Codeunit.al:25,38,48,87 (Get, Find('=') and a
// plain Get that does not calculate).
#[test]
fn set_auto_calc_fields_applies_to_get_and_find_equal_but_a_plain_get_does_not_calculate() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\"; Q: Record \"ALT Parent\"; C: Record \"ALT Child\"; Seen: Text;
        begin
            Seed();
            Q.Get(1);
            Seen := Format(Q.\"Child Count\");
            P.SetAutoCalcFields(\"Child Count\");
            P.Get(2);
            Seen += Format(P.\"Child Count\");
            C.\"Entry No.\" := 20; C.\"Parent Entry No.\" := 2; C.Insert();
            P.Get(2);
            Seen += Format(P.\"Child Count\");
            P.Init();
            P.\"Entry No.\" := 1;
            Seen += Format(P.Find('='));
            exit(Seen + Format(P.\"Child Count\"));
        end;"
    ));
    assert_eq!(text(r), "012Yes2");
}

// corpus record/TestFlowFieldContracts.al:156 (a temporary record: the source table has no
// rows, so both readings agree and the value stays 0).
#[test]
fn set_auto_calc_fields_on_a_temporary_record_over_an_empty_source_stays_zero() {
    let r = run("procedure F(): Text
        var P: Record \"ALT Parent\" temporary;
        begin
            P.\"Entry No.\" := 1; P.Insert();
            P.SetAutoCalcFields(\"Child Count\");
            P.FindFirst();
            exit(Format(P.\"Child Count\"));
        end;");
    assert_eq!(text(r), "0");
}

// The corpus does not say whether a FlowField on a temporary record reads the persistent
// source table, so a source with rows is refused.
#[test]
fn a_flow_field_on_a_temporary_record_over_a_source_with_rows_is_refused() {
    let r = run(&format!(
        "{SEED}
        procedure F(): Text
        var P: Record \"ALT Parent\" temporary;
        begin
            Seed();
            P.\"Entry No.\" := 1; P.Insert();
            P.CalcFields(\"Child Count\");
            exit(Format(P.\"Child Count\"));
        end;"
    ));
    assert!(unsupported(r).contains("temporary"));
}

// corpus record/TestRecordFlowFieldFilterVisitor.al:30,49 (a filter on a FlowField selects by the
// calculated value).
#[test]
fn a_filter_on_a_flow_field_selects_by_the_calculated_value() {
    let r = run("procedure F(): Text
        var P: Record \"ALT Parent\"; C: Record \"ALT Child\"; Seen: Text;
        begin
            P.\"Entry No.\" := 1; P.Insert();
            P.\"Entry No.\" := 2; P.Insert();
            P.Reset();
            P.SetRange(\"Child Count\", 0);
            Seen := Format(P.Count());
            C.\"Entry No.\" := 101; C.\"Parent Entry No.\" := 1; C.Insert();
            C.\"Entry No.\" := 102; C.\"Parent Entry No.\" := 1; C.Insert();
            Seen += Format(P.Count()) + Format(P.IsEmpty());
            P.FindFirst();
            exit(Seen + Format(P.\"Entry No.\"));
        end;");
    assert_eq!(text(r), "21No2");
}

/// A compact version of the corpus CFM/CFS/CFF fixtures: one source table, one table with a
/// FlowField per formula shape.
const LINES: &str = r#"
table 60450 "FF Line"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Doc No."; Code[20]) { }
        field(3; Amount; Decimal) { }
        field(4; Qty; Integer) { }
        field(5; "Posting Date"; Date) { }
        field(6; Open; Boolean) { }
        field(7; Status; Option) { OptionMembers = Open,Released,Closed; }
        field(8; Elapsed; Duration) { }
        field(9; Account; Code[20]) { }
    }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}
table 60451 "FF Head"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Date Filter"; Date) { FieldClass = FlowFilter; }
        field(3; Totaling; Text[250]) { }
        field(10; Total; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."))); }
        field(11; Lines; Integer) { FieldClass = FlowField; CalcFormula = count("FF Line" where("Doc No." = field("No."))); }
        field(12; MinAmt; Decimal) { FieldClass = FlowField; CalcFormula = min("FF Line".Amount where("Doc No." = field("No."))); }
        field(13; MaxAmt; Decimal) { FieldClass = FlowField; CalcFormula = max("FF Line".Amount where("Doc No." = field("No."))); }
        field(14; AvgAmt; Decimal) { FieldClass = FlowField; CalcFormula = average("FF Line".Amount where("Doc No." = field("No."))); }
        field(15; AvgQty; Integer) { FieldClass = FlowField; CalcFormula = average("FF Line".Qty where("Doc No." = field("No."))); }
        field(16; BadAvg; Decimal) { FieldClass = FlowField; CalcFormula = average("FF Line".Qty where("Doc No." = field("No."))); }
        field(17; FirstDate; Date) { FieldClass = FlowField; CalcFormula = min("FF Line"."Posting Date" where("Doc No." = field("No."))); }
        field(18; HasLines; Boolean) { FieldClass = FlowField; CalcFormula = exist("FF Line" where("Doc No." = field("No."))); }
        field(19; NoLines; Boolean) { FieldClass = FlowField; CalcFormula = - exist("FF Line" where("Doc No." = field("No."))); }
        field(20; NegTotal; Decimal) { FieldClass = FlowField; CalcFormula = -sum("FF Line".Amount where("Doc No." = field("No."))); }
        field(21; OpenTotal; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), Open = const(true))); }
        field(22; ActiveTotal; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), Status = filter(Open | Released))); }
        field(23; Period; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), "Posting Date" = field("Date Filter"))); }
        field(24; Balance; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), "Posting Date" = field(upperlimit("Date Filter")))); }
        field(25; Totaled; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), Account = field(filter(Totaling)))); }
        field(26; Hours; Duration) { FieldClass = FlowField; CalcFormula = sum("FF Line".Elapsed where("Doc No." = field("No."))); }
        field(27; Unformulated; Decimal) { FieldClass = FlowField; }
        field(28; SelfRef; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where("Doc No." = field("No."), Amount = field(SelfRef))); }
        field(29; CycleA; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where(Amount = field(CycleB))); }
        field(30; CycleB; Decimal) { FieldClass = FlowField; CalcFormula = sum("FF Line".Amount where(Amount = field(CycleA))); }
        field(31; MinQty; Integer) { FieldClass = FlowField; CalcFormula = min("FF Line".Qty where("Doc No." = field("No."))); }
        field(32; LateDate; Date) { FieldClass = FlowField; CalcFormula = max("FF Line"."Posting Date" where("Doc No." = field("No."))); }
        field(33; Closed; Integer) { FieldClass = FlowField; CalcFormula = count("FF Line" where("Doc No." = field("No."), Status = const(Closed))); }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}
"#;

/// D1: 40, -10, 75, 20 (qty 3, 4, 6, 8; one month apart from 2024-01-10); D2: one line of 999.
const LINE_SEED: &str = "
    procedure Add(E: Integer; D: Code[20]; A: Decimal; Q: Integer; P: Date; IsOpen: Boolean; St: Option)
    var L: Record \"FF Line\";
    begin
        L.Init(); L.\"Entry No.\" := E; L.\"Doc No.\" := D; L.Amount := A; L.Qty := Q; L.\"Posting Date\" := P;
        L.Open := IsOpen; L.Status := St; L.Account := 'A' + Format(E mod 2);
        L.Insert();
    end;
    procedure SeedLines()
    var H: Record \"FF Head\"; L: Record \"FF Line\";
    begin
        H.\"No.\" := 'D1'; H.Insert();
        H.\"No.\" := 'D2'; H.Insert();
        H.\"No.\" := 'D3'; H.Insert();
        Add(1, 'D1', 40, 3, 20240110D, true, L.Status::Open);
        Add(2, 'D1', -10, 4, 20240210D, false, L.Status::Released);
        Add(3, 'D1', 75, 6, 20240310D, true, L.Status::Closed);
        Add(4, 'D1', 20, 8, 20240410D, true, L.Status::Open);
        Add(9, 'D2', 999, 99, 20240215D, true, L.Status::Open);
    end;
";

fn run_lines(body: &str) -> Result<Value, Signal> {
    run_with(LINES, &format!("{LINE_SEED}\n{body}"), "F")
}

// corpus record/TestCalcFormulaMinMaxAvgTests.Codeunit.al:140,271 (125 / 4 rows = 31.25, not 31).
#[test]
fn average_into_a_decimal_field_keeps_the_fraction() {
    let r = run_lines(
        "procedure F(): Text
        var H: Record \"FF Head\";
        begin
            SeedLines();
            H.Get('D1');
            H.CalcFields(AvgAmt, Total, Lines);
            exit(Format(H.AvgAmt) + '|' + Format(H.Total) + '|' + Format(H.Lines));
        end;",
    );
    assert_eq!(text(r), "31.25|125|4");
}
