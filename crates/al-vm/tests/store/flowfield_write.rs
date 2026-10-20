//! Writes into a FlowField: Validate, TransferFields and a temporary record
//! (record/lt-flowfield-write/DiFwWriteTests.al).

use al_testkit::{run_in, text};

const SRC: &str = r#"
table 50300 P
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Name; Text[30]) { }
        field(3; "Child Count"; Integer) { FieldClass = FlowField; CalcFormula = count(C where("Parent No." = field("No."))); }
        field(4; "Child Amount"; Decimal) { FieldClass = FlowField; CalcFormula = sum(C.Amount where("Parent No." = field("No."), "Posting Date" = field("Date Filter"))); }
        field(5; "Has Children"; Boolean) { FieldClass = FlowField; CalcFormula = exist(C where("Parent No." = field("No."))); }
        field(6; "First Code"; Text[30]) { FieldClass = FlowField; CalcFormula = lookup(C.Code where("Parent No." = field("No."))); }
        field(7; "Date Filter"; Date) { FieldClass = FlowFilter; }
        field(8; "Max Amount"; Decimal) { FieldClass = FlowField; CalcFormula = max(C.Amount where("Parent No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50301 C
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Parent No."; Integer) { }
        field(3; Code; Text[30]) { }
        field(4; Amount; Decimal) { }
        field(5; "Posting Date"; Date) { }
    }
    keys { key(PK; "Entry No.") { } }
}

codeunit 50302 T
{
    local procedure Seed()
    var
        P: Record P;
        C: Record C;
    begin
        P."No." := 1; P.Insert();
        P."No." := 2; P.Insert();
        P."No." := 3; P.Insert();
        AddChild(1, 1, 'A', 10, 20260110D);
        AddChild(2, 1, 'B', 20, 20260210D);
        AddChild(3, 3, 'D', 5, 20260115D);
    end;

    local procedure AddChild(EntryNo: Integer; ParentNo: Integer; NewCode: Text[30]; NewAmount: Decimal; PostingDate: Date)
    var
        C: Record C;
    begin
        C."Entry No." := EntryNo;
        C."Parent No." := ParentNo;
        C.Code := NewCode;
        C.Amount := NewAmount;
        C."Posting Date" := PostingDate;
        C.Insert();
    end;

    local procedure Src(): Text
    var
        C: Record C;
        R: Text;
    begin
        if C.FindSet() then
            repeat
                R += Format(C."Entry No.") + '|' + Format(C."Parent No.") + '|' + C.Code + '|' + Format(C.Amount, 0, 9) + '|' + Format(C."Posting Date", 0, 9) + ';';
            until C.Next() = 0;
        exit(R);
    end;

    procedure ValidateCount(): Text
    var
        P: Record P;
    begin
        Seed();
        P.Get(1);
        asserterror P.Validate("Child Count", 7);
        exit(GetLastErrorText() + '|' + GetLastErrorCode());
    end;

    procedure ValidateExistThroughFieldRef(): Text
    var
        P: Record P;
        R: RecordRef;
        F: FieldRef;
    begin
        Seed();
        P.Get(1);
        R.GetTable(P);
        F := R.Field(5);
        asserterror F.Validate(true);
        exit(GetLastErrorText() + '|' + GetLastErrorCode());
    end;

    procedure ValidateMax()
    var
        P: Record P;
    begin
        Seed();
        P.Get(1);
        P.Validate("Max Amount", 7);
    end;

    procedure ValidateSum(): Text
    var
        P: Record P;
        R: Text;
    begin
        Seed();
        P.Get(2);
        P.SetRange("Date Filter", 20260401D);
        P.Validate("Child Amount", 7);
        R := Format(P."Child Amount") + '#' + Src() + '#';
        P.Reset();
        P.Get(1);
        P."Child Amount" := 1000;
        P.Validate("Child Amount", 70);
        R += Src() + '#';
        P.Get(1);
        P.Validate("Child Amount", 70);
        R += Src() + '#';
        P.Get(3);
        P.SetRange("Date Filter", 20260101D, 20260131D);
        P.Validate("Child Amount", 2);
        exit(R + Src());
    end;

    procedure TransferSameTable(): Text
    var
        S: Record P;
        D: Record P;
    begin
        Seed();
        S.Get(1);
        S."Child Count" := 7;
        D.Get(2);
        D.TransferFields(S, false);
        exit(Format(D."No.") + '/' + Format(D."Child Count"));
    end;

    procedure TemporaryRow(): Text
    var
        T: Record P temporary;
        R: Text;
    begin
        Seed();
        T."No." := 1;
        T."Child Count" := 5;
        T.Insert();
        T.Get(1);
        R := Format(T."Child Count");
        T."Child Count" := 7;
        T.Modify();
        Clear(T."Child Count");
        T.Get(1);
        R += '/' + Format(T."Child Count");
        T.CalcFields("Child Count");
        R += '/' + Format(T."Child Count");
        T.Get(1);
        exit(R + '/' + Format(T."Child Count"));
    end;
}
"#;

// DiFwWriteTests.al:284, :439, :693-694: Validate on a Count, Exist or Lookup FlowField raises FlowFieldValidation.
#[test]
fn validate_on_a_count_or_exist_flowfield_raises() {
    assert_eq!(
        text(run_in(SRC, "T", "ValidateCount")),
        "The type of the Child Count FlowField in the P table must be Decimal.|FlowFieldValidation"
    );
    assert_eq!(
        text(run_in(SRC, "T", "ValidateExistThroughFieldRef")),
        "The type of the Has Children FlowField in the P table must be Decimal.|FlowFieldValidation"
    );
}

// Validate on a Max FlowField is not pinned.
#[test]
fn validate_on_a_max_flowfield_is_refused() {
    assert!(matches!(
        run_in(SRC, "T", "ValidateMax"),
        Err(al_builtins::native::Signal::Unsupported(_))
    ));
}

// DiFwWriteTests.al:812-819: Validate on a Sum FlowField inserts the difference into the source table.
#[test]
fn validate_on_a_sum_flowfield_writes_the_difference_into_the_source_table() {
    let base = "1|1|A|10|2026-01-10;2|1|B|20|2026-02-10;3|3|D|5|2026-01-15;";
    let p2 = format!("{base}4|2||7|2026-04-01;");
    let p1 = format!("{p2}5|1||40|;");
    assert_eq!(
        text(run_in(SRC, "T", "ValidateSum")),
        format!("7#{p2}#{p1}#{p1}#{p1}6|3||-3|2026-01-01;")
    );
}

// DiFwWriteTests.al:504-506: TransferFields within one table copies an assigned FlowField value.
#[test]
fn transferfields_within_a_table_copies_a_flowfield() {
    assert_eq!(text(run_in(SRC, "T", "TransferSameTable")), "2/7");
}

// DiFwWriteTests.al:625-630: a temporary row keeps the FlowField value a Modify wrote, not the one of an Insert.
#[test]
fn a_temporary_row_keeps_the_flowfield_value_of_a_modify() {
    assert_eq!(text(run_in(SRC, "T", "TemporaryRow")), "0/7/2/7");
}

const OR: &str = r#"
table 50340 O
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Name; Text[30]) { }
        field(3; Total; Integer) { FieldClass = FlowField; CalcFormula = count(O where(Id = field(Id))); }
    }
    keys { key(PK; Id) { } }
}

codeunit 50341 T
{
    procedure F(): Text
    var
        O: Record O;
    begin
        O.Id := 1; O.Name := 'one'; O.Insert();
        O.Id := 2; O.Name := ''; O.Insert();
        O.FilterGroup(-1);
        O.SetRange(Name, '');
        O.SetRange(Total, 0);
        O.SetFilter(Total, '>6');
        exit(O.GetFilters() + '|' + Format(O.Count()));
    end;
}
"#;

// record/PWFilterGroupOr.al:OrGroup_FlowFieldFilter_BQ: a FlowField filter in the OR group is dropped.
#[test]
fn a_flowfield_filter_in_the_or_group_is_dropped() {
    assert_eq!(text(run_in(OR, "T", "F")), "Name: ''|1");
}

const BULK: &str = r#"
table 50350 M
{
    fields { field(1; Id; Integer) { } field(2; Amount; Decimal) { } }
    keys { key(PK; Id) { } }
}

codeunit 50351 T
{
    procedure F(): Text
    var
        M: Record M;
        Before: DateTime;
        R: Text;
    begin
        M.Id := 1; M.Amount := 10; M.Insert();
        M.Get(1);
        Before := M.SystemModifiedAt;
        Sleep(50);
        M.ModifyAll(Amount, 10, true);
        M.Get(1);
        R := Format(M.SystemModifiedAt <> Before);
        Sleep(50);
        M.ModifyAll(Amount, 10, false);
        M.Get(1);
        exit(R + '/' + Format(M.SystemModifiedAt <> Before));
    end;
}
"#;

// record/lt-record-write/RcBTriggers.al:ModifyAll_Equal_Value_Rows_BQ: ModifyAll(.., false) stamps a row that keeps its value.
#[test]
fn modifyall_without_triggers_stamps_a_row_that_keeps_its_value() {
    assert_eq!(text(run_in(BULK, "T", "F")), "No/Yes");
}

const ORDER: &str = r#"
table 50360 F
{
    fields
    {
        field(1; A; Integer) { }
        field(2; DF; Date) { FieldClass = FlowFilter; }
        field(3; B; Integer) { }
        field(4; N; Integer) { FieldClass = FlowField; CalcFormula = count(F); }
    }
    keys { key(PK; A) { } }
}

codeunit 50361 T
{
    procedure F(): Text
    var
        RR: RecordRef;
        i: Integer;
        R: Text;
    begin
        RR.Open(Database::F);
        for i := 1 to RR.FieldCount() do
            R += Format(RR.FieldIndex(i).Number()) + ';';
        exit(R);
    end;
}
"#;

// metadata/lt-custom-report-layout-columns/CustomReportLayoutColumns.al:PW_FieldIndex_FlowFilterAmongNormalFields.
#[test]
fn fieldindex_lists_normal_fields_then_flowfilters_then_flowfields() {
    assert_eq!(text(run_in(ORDER, "T", "F")), "1;3;2;4;");
}

const VARIANT: &str = r#"
table 50370 VR { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
table 2000000006 Company
{
    DataPerCompany = false;
    fields { field(1; Name; Text[30]) { } field(8000; Id; Guid) { } }
    keys { key(PK; Name) { } }
}

codeunit 50371 T
{
    procedure F(): Text
    var
        C: Record Company;
        R: Record VR;
        RecRef: RecordRef;
        V: Variant;
        Out: Text;
    begin
        C.Name := 'OTHER';
        C.Insert();
        R.ChangeCompany('OTHER');
        R.SetFilter(Id, '>=2');
        RecRef.GetTable(R);
        Out := Format(RecRef.CurrentCompany() = 'OTHER');
        V := R;
        RecRef.GetTable(V);
        exit(Out + '|' + Format(RecRef.CurrentCompany() = CompanyName()) + '|' + RecRef.GetFilters());
    end;
}
"#;

// types/lt-cw-variant-leads/CwVariantChangeCompany.al:CW_Variant_KeepsOrDropsChangeCompany: through a Variant, GetTable drops the company.
#[test]
fn gettable_through_a_variant_drops_the_company_and_keeps_the_filters() {
    assert_eq!(text(run_in(VARIANT, "T", "F")), "Yes|Yes|Id: >=2");
}

const TEMP_PARAM: &str = r#"
table 50380 TP { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }

codeunit 50381 T
{
    local procedure Look(R: Record TP temporary): Text
    begin
        exit(Format(R.IsTemporary()) + ' ' + Format(R.Count()) + ' ' + Format(R.Id));
    end;

    procedure F(): Text
    var
        R: Record TP;
    begin
        R.Id := 1; R.Insert();
        R.Id := 2; R.Insert();
        exit(Look(R));
    end;
}
"#;

// events/lt-cl-event-leads/ClEventLeads.al:70: a stored record handed by value to a `temporary` parameter is a temporary copy.
#[test]
fn a_stored_record_handed_to_a_temporary_parameter_is_an_empty_temporary_copy() {
    assert_eq!(text(run_in(TEMP_PARAM, "T", "F")), "Yes 0 2");
}
