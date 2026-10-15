//! Fixes found by running Tests-SCM (round 2). Each test names what pins it.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

fn compile(src: &str) -> (al_sema::workspace::Workspace, al_sema::lower::Lowered) {
    al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            // `//--file` starts another file (a namespace is declared once per file).
            files: src
                .split("//--file\n")
                .enumerate()
                .map(|(i, f)| (format!("f{i}.al"), f.to_string()))
                .collect(),
        }],
        &Default::default(),
    )
}

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
    let (ws, l) = compile(src);
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
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn reasons(src: &str) -> Vec<String> {
    let (_, l) = compile(src);
    l.unsupported.iter().flatten().flatten().cloned().collect()
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_dataitem_of_a_namespace_qualified_table_resolves_its_fields() {
    let src = r#"
namespace Microsoft.Test.Forecast;
table 69300 "Fc Entry" { fields { field(1; Name; Code[10]) { } field(2; Qty; Integer) { } } keys { key(PK; Name) { } } }
//--file
namespace Microsoft.Test.Reports;
using Microsoft.Test.Forecast;
report 69301 "Fc Report"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem("Fc Entry"; Microsoft.Test.Forecast."Fc Entry")
        {
            trigger OnPreDataItem() begin SetRange(Name, 'A'); SetRange("Name", 'A'); end;
        }
    }
}
"#;
    let r = reasons(src);
    assert!(r.is_empty(), "{r:?}");
}

#[test]
fn control_without_namespaces() {
    let src = r#"
table 69300 "Fc Entry" { fields { field(1; Name; Code[10]) { } field(2; Qty; Integer) { } } keys { key(PK; Name) { } } }
report 69301 "Fc Report"
{
    ProcessingOnly = true;
    dataset
    {
        dataitem("Fc Entry"; "Fc Entry")
        {
            trigger OnPreDataItem() begin SetRange(Name, 'A'); end;
        }
    }
}
"#;
    let r = reasons(src);
    assert!(r.is_empty(), "{r:?}");
}

const ROWS: &str = r#"
table 69310 "Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69311 C
{
    procedure Steps(): Text
    var R: Record "Row"; Freq: Integer; Days: Decimal; D: Date;
    begin
        for Freq := 1 to 6 do begin R.Id := Freq; R.Insert(); end;
        Freq := 2;
        R.FindFirst();
        R.Next(4 / Freq);
        Days := 6 div 2;
        D := 20240101D;
        exit(Format(R.Id) + '|' + Format(D + Days) + '|' + Format(D - Days));
    end;
    procedure Select(): Text
    var T: Option "Standard Cost","Unit Price";
    begin
        T := T::"Unit Price";
        exit(SelectStr(T, 'zero,one,two'));
    end;
}
"#;

// Next(Decimal) and Date +/- Decimal with a whole number, SelectStr with an Option: the same conversions as an
// assignment to an Integer (ops::convert); a fraction stays refused.
#[test]
fn whole_decimals_step_and_an_option_selects() {
    assert_eq!(text(run(ROWS, "Steps")), "3|01/04/24|12/29/23");
    // SelectStr counts from 1 and the Option member `Unit Price` is ordinal 1.
    assert_eq!(text(run(ROWS, "Select")), "zero");
}

#[test]
fn an_empty_set_filter_removes_the_filter_of_the_field() {
    let src = r#"
table 69310 "Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69311 C
{
    procedure F(): Text
    var R: Record "Row"; s: Text;
    begin
        R.Id := 1; R.Insert(); R.Id := 2; R.Insert();
        R.SetFilter(Id, '>1');
        s := Format(R.Count()) + '|' + R.GetFilters();
        R.SetFilter(Id, '');
        exit(s + '|' + Format(R.Count()) + '|' + R.GetFilters());
    end;
}
"#;
    assert_eq!(text(run(src, "F")), "1|Id: >1|2|");
}

// A table's OnValidate hands `Rec` to a codeunit by var and the codeunit modifies it (Base Application's Item
// Type validation runs ItemCostManagement.UpdateUnitCost, which ends in Item.Modify): the caller's buffer is the
// one that was modified, so its own Modify afterwards is not stale (SMB Service Item TestItem_Reserve).
#[test]
fn a_modify_through_a_var_rec_in_validate_leaves_the_callers_buffer_current() {
    let src = r#"
table 69320 "Doc" { fields { field(1; Id; Integer) { } field(2; Kind; Integer) { trigger OnValidate() var M: Codeunit Mgt; begin M.Touch(Rec); end; } field(3; Cost; Decimal) { } }
  keys { key(PK; Id) { } } }
codeunit 69321 Mgt { procedure Touch(var D: Record Doc) var Check: Record Doc; begin D.Cost := 5; if Check.Get(D.Id) then D.Modify(true); end; }
codeunit 69322 C
{
    procedure F(): Text
    var D: Record Doc;
    begin
        D.Id := 1; D.Insert(true);
        D.Validate(Kind, 3);
        D.Modify(true);
        exit(Format(D.Cost) + '|' + Format(D.Kind));
    end;
}
"#;
    assert_eq!(text(run(src, "F")), "5|3");
}

// Reservation Management has SetReservSource(Variant) and SetReservSource(RecordRef); a RecordRef argument picks the
// RecordRef overload, otherwise the Variant one calls itself with the RecordRef and GetTable fails on it.
#[test]
fn an_overload_taking_the_argument_type_wins_over_a_variant_overload() {
    let src = r#"
table 69330 "Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69331 Mgt
{
    var Seen: Text;
    procedure Take(V: Variant) var RR: RecordRef; begin RR.GetTable(V); Take(RR); end;
    procedure Take(RR: RecordRef) begin Seen := 'recordref ' + Format(RR.Number()); end;
    procedure Get(): Text begin exit(Seen); end;
}
codeunit 69332 C
{
    procedure F(): Text
    var R: Record "Row"; M: Codeunit Mgt;
    begin
        M.Take(R);
        exit(M.Get());
    end;
}
"#;
    assert_eq!(text(run(src, "F")), "recordref 69330");
}
