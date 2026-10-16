//! Record behaviour pinned by the corpus probes of cdf315a (record/TestRecordRuntimeTextsAndViews.al,
//! record/TestTableEventSequenceContracts.al, record/TestCalcFieldsTruncateUniqueContracts.al) and
//! by the batch 2 probes (record/TestFilterTextBooleanFindContracts.al,
//! record/TestRecordViewCopyMarkContracts.al, record/TestValidateRaisesAndHandlerAsserterrorContracts.al).
//! Each test mirrors a probe test so the behaviour has a red/green
//! test here as well as in the corpus run.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_testkit::text;

fn run(src: &str, routine: &str) -> Result<Value, Signal> {
    al_testkit::run_in(src, "C", routine)
}

const EVT: &str = r#"
codeunit 50300 Log
{
    SingleInstance = true;
    var L: Text;
    procedure Add(E: Text) begin L += E + ';'; end;
    procedure Take(): Text var R: Text; begin R := L; L := ''; exit(R); end;
}
table 50301 Evt
{
    fields { field(1; Id; Integer) { } field(2; Val; Text[20]) { } field(4; Pay; Blob) { } }
    keys { key(PK; Id) { } }
    trigger OnInsert() var G: Codeunit Log; begin G.Add('T.OnInsert(' + Val + ')'); end;
    trigger OnModify() var G: Codeunit Log; begin G.Add('T.OnModify(' + Val + ',' + xRec.Val + ')'); end;
    trigger OnDelete() var G: Codeunit Log; begin G.Add('T.OnDelete(' + Format(Id) + ')'); end;
    trigger OnRename() var G: Codeunit Log; begin G.Add('T.OnRename(' + Format(Id) + ',' + Format(xRec.Id) + ')'); end;
}
codeunit 50302 Sub
{
    var Hits: Integer;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin Hits += 1; G.Add('S.BI(temp=' + Format(Rec.IsTemporary()) + ',hits=' + Format(Hits) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.AI(hits=' + Format(Hits) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeModifyEvent', '', false, false)]
    local procedure BM(var Rec: Record Evt; var xRec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BM(' + Rec.Val + ',' + xRec.Val + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeDeleteEvent', '', false, false)]
    local procedure BD(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BD(' + Format(Rec.Id) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnAfterDeleteEvent', '', false, false)]
    local procedure AD(var Rec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.AD(' + Format(Rec.Id) + ')'); end;
    [EventSubscriber(ObjectType::Table, Database::Evt, 'OnBeforeRenameEvent', '', false, false)]
    local procedure BR(var Rec: Record Evt; var xRec: Record Evt; RunTrigger: Boolean) var G: Codeunit Log; begin G.Add('S.BR(' + Format(Rec.Id) + ',' + Format(xRec.Id) + ')'); end;
}
"#;

fn with(body: &str) -> String {
    format!("{EVT}\ncodeunit 50303 C {{ {body} }}")
}

#[test]
fn failing_writes_run_before_event_and_trigger_then_raise() {
    let src = with(
        "procedure F(): Text var E: Record Evt; G: Codeunit Log; R: Text; begin
            E.Id := 1; E.Val := 'a'; E.Insert(true); G.Take();
            E.Id := 1; asserterror E.Insert(true); R += G.Take() + '|';
            E.Id := 99; E.Val := 'x'; asserterror E.Modify(true); R += G.Take() + '|';
            asserterror E.Delete(true); R += G.Take() + '|';
            asserterror E.Rename(100); R += G.Take();
            exit(R);
        end;",
    );
    assert_eq!(
        text(run(&src, "F")),
        "S.BI(temp=No,hits=1);T.OnInsert(a);|S.BM(x,x);T.OnModify(x,x);|S.BD(99);T.OnDelete(99);|S.BR(100,99);T.OnRename(100,99);"
    );
}

// ---- batch 2: filters, views, Copy, Mark (record/TestFilterTextBooleanFindContracts.al,
// record/TestRecordViewCopyMarkContracts.al) ----

const UNI: &str = r#"
table 50400 Uni
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Code Field"; Code[20]) { }
        field(3; "Text Field"; Text[100]) { }
        field(4; "Boolean Field"; Boolean) { }
    }
    keys { key(PK; "Entry No.") { } key(ByText; "Text Field") { } }
}
"#;

fn uni(body: &str) -> String {
    format!(
        "{UNI}\ncodeunit 50401 C {{ {body}
        local procedure Seed() var U: Record Uni; begin
            U.\"Entry No.\" := 1; U.\"Code Field\" := 'APPLE'; U.\"Text Field\" := 'Apple'; U.\"Boolean Field\" := true; U.Insert();
            U.Init(); U.\"Entry No.\" := 2; U.\"Code Field\" := 'BANANA'; U.\"Text Field\" := 'apple'; U.\"Boolean Field\" := false; U.Insert();
            U.Init(); U.\"Entry No.\" := 3; U.\"Code Field\" := 'AP-2'; U.\"Text Field\" := 'A*B'; U.\"Boolean Field\" := true; U.Insert();
            U.Init(); U.\"Entry No.\" := 4; U.\"Code Field\" := 'X?Y'; U.\"Text Field\" := '@home'; U.\"Boolean Field\" := false; U.Insert();
            U.Init(); U.\"Entry No.\" := 5; U.\"Code Field\" := ''; U.\"Text Field\" := ''; U.\"Boolean Field\" := true; U.Insert();
        end;
        }}"
    )
}

#[test]
fn text_filter_is_case_sensitive_unless_it_starts_with_at() {
    let src = uni("procedure F(): Text var U: Record Uni; R: Text; begin Seed();
            U.SetFilter(\"Text Field\", 'apple'); R += Format(U.Count()) + ',';
            U.SetFilter(\"Text Field\", '@apple'); R += Format(U.Count()) + ',';
            U.SetFilter(\"Text Field\", 'ap*'); R += Format(U.Count()) + ',';
            U.SetFilter(\"Text Field\", '@ap*'); R += Format(U.Count()) + ',';
            U.SetFilter(\"Text Field\", '''A*B'''); R += Format(U.Count()) + ',';
            U.SetFilter(\"Text Field\", '''@home'''); R += Format(U.Count()) + ',';
            U.SetFilter(\"Code Field\", 'ap*'); R += Format(U.Count()) + ',';
            U.Reset(); U.SetFilter(\"Code Field\", 'apple'); R += Format(U.Count());
            exit(R); end;");
    // record/TestFilterTextBooleanFindContracts.al:58 (SetFilter_CodeField_IsCaseInsensitive), :73
    // (SetFilter_TextField_IsCaseSensitive_UnlessItStartsWithAt), :90 (SetFilter_QuotedValue_...)
    assert_eq!(text(run(&src, "F")), "1,2,1,2,1,0,0,1");
}

#[test]
fn boolean_filter_words_and_error() {
    let src = uni(
        "procedure F(): Text var U: Record Uni; R: Text; w: Text; begin Seed();
            foreach w in 'Yes,yes,true,1' .Split(',') do begin U.SetFilter(\"Boolean Field\", w); R += Format(U.Count()); end;
            foreach w in 'No,false,0' .Split(',') do begin U.SetFilter(\"Boolean Field\", w); R += Format(U.Count()); end;
            asserterror U.SetFilter(\"Boolean Field\", 'Ja');
            exit(R + '|' + GetLastErrorText() + '|' + GetLastErrorCode()); end;",
    );
    assert_eq!(
        text(run(&src, "F")),
        "3333222|The filter \"Ja\" is not valid for the Boolean Field field on the Uni table. The value \"Ja\" can't be evaluated into type Boolean.|Filter:CSide"
    );
}

#[test]
fn find_honours_the_active_filters() {
    let src = uni("procedure F(): Text var U: Record Uni; R: Text; begin Seed();
            U.SetRange(\"Code Field\", 'APPLE');
            U.\"Entry No.\" := 2; R += Format(U.Find()) + ',';
            U.\"Entry No.\" := 2; R += Format(U.Find('=')) + ',';
            U.\"Entry No.\" := 2; R += Format(U.Find('>=')) + ',';
            U.\"Entry No.\" := 0; R += Format(U.Find('>')) + ',' + Format(U.\"Entry No.\") + ',';
            U.\"Entry No.\" := 1; R += Format(U.Find());
            exit(R); end;");
    assert_eq!(text(run(&src, "F")), "No,No,No,Yes,1,Yes");
}

const KD: &str = r#"
table 50410 Kd
{
    fields { field(1; "Entry No."; Integer) { } field(2; Name; Text[30]) { } field(3; Amount; Decimal) { } }
    keys { key(PK; "Entry No.") { } key(ByAmount; Amount) { } }
}
"#;

fn kd(body: &str) -> String {
    format!(
        "{KD}\ncodeunit 50411 C {{ {body}
        local procedure Seed() var K: Record Kd; begin
            K.\"Entry No.\" := 1; K.Name := 'a'; K.Amount := 5; K.Insert();
            K.Init(); K.\"Entry No.\" := 2; K.Name := 'b'; K.Amount := 6; K.Insert(); Commit();
        end;
        }}"
    )
}

#[test]
fn set_rec_filter_keeps_filters_on_other_fields() {
    // record/TestRecordViewCopyMarkContracts.al:41
    // (SetRecFilter_KeepsFiltersOnOtherFields_AndFiltersOnThePrimaryKey)
    let src = kd("procedure F(): Text var K: Record Kd; R: Text; begin Seed();
            K.Get(1); K.SetRange(Name, 'zzz'); K.SetRecFilter();
            R += K.GetFilters() + '|' + Format(K.Count()) + '|';
            K.Reset(); K.SetRange(Amount, 5); K.Get(2); K.SetRecFilter();
            exit(R + K.GetFilters()); end;");
    assert_eq!(
        text(run(&src, "F")),
        "Entry No.: 1, Name: zzz|0|Entry No.: 2, Amount: 5"
    );
}

#[test]
fn copy_carries_filters_of_all_groups_and_checks_share_table() {
    // record/TestRecordViewCopyMarkContracts.al:Copy_CarriesBufferFiltersOfAllGroupsKeyAndDirection,
    // Copy_ShareTable_RequiresBothRecordsToBeTemporary
    let src = kd(
        "procedure F(): Text var K: Record Kd; C: Record Kd; T: Record Kd temporary; R: Text; begin Seed();
            K.SetCurrentKey(Amount); K.Ascending(false); K.SetRange(\"Entry No.\", 1, 2);
            K.FilterGroup(2); K.SetRange(Name, 'a'); K.FilterGroup(0); K.Get(1);
            C.Copy(K);
            R += C.GetFilters() + '|' + C.CurrentKey() + '|' + Format(C.Ascending()) + '|' + Format(C.\"Entry No.\") + '|';
            C.FilterGroup(2); R += C.GetFilters() + '|'; C.FilterGroup(0); R += Format(C.FilterGroup()) + '|';
            T.\"Entry No.\" := 99; T.Insert();
            asserterror T.Copy(K, true); R += GetLastErrorText() + '|' + GetLastErrorCode() + '|';
            asserterror C.Copy(T, true); R += GetLastErrorText() + '|';
            Clear(C); C.Copy(T); R += Format(C.IsTemporary()) + Format(C.Count());
            exit(R); end;",
    );
    assert_eq!(
        text(run(&src, "F")),
        "Entry No.: 1..2|Amount|No|1|Name: a|0|The COPY function can only be used with the shareTable argument set to true if both records are temporary.|Argument|The COPY function can only be used with the shareTable argument set to true if both records are temporary.|No2"
    );
}

#[test]
fn mark_returns_the_new_state() {
    // record/TestRecordViewCopyMarkContracts.al:Mark_ReturnsTheNewState
    let src = kd("procedure F(): Text var K: Record Kd; R: Text; begin Seed(); K.Get(1);
            R += Format(K.Mark(true)) + Format(K.Mark(true)) + Format(K.Mark(false)) + Format(K.Mark());
            K.Reset(); K.MarkedOnly(true); exit(R + Format(K.Count())); end;");
    assert_eq!(text(run(&src, "F")), "YesYesNoNo0");
}

#[test]
fn set_view_replaces_filters_keeps_direction_and_rejects_garbage() {
    // record/TestRecordViewCopyMarkContracts.al:SetView_ReplacesTheFilters_KeepsTheDirection_AndRejectsGarbage
    let src = kd(
        "procedure F(): Text var K: Record Kd; R: Text; Cr: Text[1]; begin Seed();
            K.SetView('SORTING(Amount) ORDER(Descending) WHERE(Entry No.=FILTER(1..2))'); R += K.GetView() + '|';
            K.SetRange(Name, 'a'); K.SetView('WHERE(Amount=FILTER(6))');
            R += K.GetFilters() + '|' + K.GetView() + '|';
            asserterror K.SetView('GARBAGE('); Cr[1] := 13;
            exit(R + GetLastErrorText() + '|' + GetLastErrorCode()); end;",
    );
    assert_eq!(
        text(run(&src, "F")),
        "VERSION(1) SORTING(Amount) ORDER(Descending) WHERE(Entry No.=FILTER(1..2))|Amount: 6|VERSION(1) SORTING(Entry No.) ORDER(Descending) WHERE(Amount=FILTER(6))|Invalid expression of type: table view.\\GARBAGE(|DB:Ok"
    );
}

#[test]
fn a_validate_that_raises_restores_the_whole_record() {
    // record/TestValidateRaisesAndHandlerAsserterrorContracts.al:68
    // (Validate_ThatRaises_RestoresTheWholeRecord)
    let src = "table 50420 VR
        {
            fields
            {
                field(1; Id; Integer) { }
                field(2; A; Text[20]) { trigger OnValidate() begin B := 'set-by-A-trigger'; C := C + 1; if A = 'bad' then Error('A rejected'); end; }
                field(3; B; Text[20]) { }
                field(4; C; Integer) { }
            }
            keys { key(PK; Id) { } }
        }
        codeunit 50421 C { procedure F(): Text var R: Record VR; begin
            R.Id := 1; R.A := 'old'; R.B := 'orig-B'; R.C := 10;
            asserterror R.Validate(A, 'bad');
            exit(GetLastErrorText() + '|' + R.A + '|' + R.B + '|' + Format(R.C)); end; }";
    assert_eq!(text(run(src, "F")), "A rejected|old|orig-B|10");
}

#[test]
fn a_temporary_record_has_no_system_id_and_the_duplicate_texts_are_pinned() {
    // record/TestSystemFieldStampsContracts.al:TemporaryRecord_HasNoStampsAndNoSystemId,
    // SystemId_Duplicate_And_ModifyAll_Texts
    let src = "table 50430 SF
        {
            fields { field(1; Id; Integer) { } field(2; Txt; Text[20]) { } }
            keys { key(PK; Id) { } }
        }
        codeunit 50431 C { procedure F(): Text var T: Record SF temporary; S: Record SF; Rows: Record SF; G: Guid; R: Text; begin
            T.Id := 1; T.Insert(true); T.Get(1); R += Format(IsNullGuid(T.SystemId)) + '|';
            G := CreateGuid(); S.Id := 3; S.SystemId := G; S.Insert(false, true); Commit();
            Clear(S); S.Id := 4; S.SystemId := G;
            asserterror S.Insert(false, true); R += GetLastErrorText() + '|' + GetLastErrorCode() + '|';
            asserterror Rows.ModifyAll(SystemId, CreateGuid()); R += GetLastErrorText();
            exit(R); end; }";
    let r = text(run(src, "F"));
    assert!(r.starts_with("Yes|There is already a record in table SF that has the same values in a unique index for the following fields: System ID='{"), "{r}");
    assert!(
        r.ends_with("}'|DB:RecordExists|You cannot change the value of the SystemId field.\\\\Table: SF"),
        "{r}"
    );
}

#[test]
fn delete_all_with_marked_only_deletes_the_marked_rows_and_no_other() {
    // No corpus test pins DeleteAll under MarkedOnly (docs/probes-needed.md). Base Application's Post Inventory Cost to
    // G/L marks the value entries it posts and ends with `MarkedOnly(true); DeleteAll()`, and the capacity entries it
    // still has to post stay in the table for the loop after it. Microsoft Learn: MarkedOnly limits the records an
    // operation works on to the marked ones, DeleteAll included. The same must hold with a filter and with triggers.
    let src = kd("procedure F(): Text var K: Record Kd; R: Text; begin Seed();
            K.\"Entry No.\" := 3; K.Name := 'c'; K.Amount := 7; K.Insert();
            K.Get(1); K.Mark(true); K.Get(3); K.Mark(true);
            K.MarkedOnly(true); R += Format(K.Count()) + '|';
            K.DeleteAll();
            K.MarkedOnly(false); R += Format(K.Count()) + '|';
            if K.FindFirst() then R += Format(K.\"Entry No.\"); exit(R); end;
        procedure WithAFilter(): Text var K: Record Kd; R: Text; begin Seed();
            K.\"Entry No.\" := 3; K.Name := 'c'; K.Amount := 7; K.Insert();
            K.FindSet(); repeat K.Mark(true); until K.Next() = 0;
            K.SetFilter(\"Entry No.\", '2..3'); K.MarkedOnly(true); K.DeleteAll();
            K.Reset(); exit(Format(K.Count())); end;");
    assert_eq!(text(run(&src, "F")), "2|1|2");
    assert_eq!(text(run(&src, "WithAFilter")), "1");
}
