//! A host control's OnValidate that calls `CurrPage.SaveRecord()`: the host's linked parts read their rows again
//! (assumption host-save-record-refreshes-linked-parts, needed by Microsoft's Tests-ERM ERM Purchase Subform 134394:
//! Purchase Order "Currency Code" OnValidate calls CurrPage.SaveRecord() and the subform's totals must follow).
//! pageupdate/TestPageUpdate.al:116-139 pins the arm without SaveRecord: nothing more runs.

use al_testkit::text;

const SRC: &str = r#"
table 51450 "SR Head" { fields { field(1; "Code"; Code[20]) { } field(2; Descr; Text[30]) { } } keys { key(PK; "Code") { } } }
table 51451 "SR Line"
{
    fields { field(1; "Head Code"; Code[20]) { } field(2; "Line No."; Integer) { } }
    keys { key(PK; "Head Code", "Line No.") { } }
}
codeunit 51452 "SR Log"
{
    SingleInstance = true;
    var T: Text;
    procedure Add(S: Text) begin T += S + ';'; end;
    procedure Reset() begin T := ''; end;
    procedure Dump(): Text begin exit(T); end;
}
page 51453 "SR Linked"
{
    PageType = ListPart;
    SourceTable = "SR Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetCurrRecord() var Log: Codeunit "SR Log"; begin Log.Add('L:' + Format(Rec."Line No.")); end;
}
page 51454 "SR Plain"
{
    PageType = ListPart;
    SourceTable = "SR Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetCurrRecord() var Log: Codeunit "SR Log"; begin Log.Add('P:' + Format(Rec."Line No.")); end;
}
page 51455 "SR Host"
{
    PageType = Card;
    SourceTable = "SR Head";
    layout
    {
        area(Content)
        {
            field(CodeCtl; Rec."Code") { }
            field(Saved; Rec.Descr) { trigger OnValidate() var Log: Codeunit "SR Log"; begin Log.Add('validate'); CurrPage.SaveRecord(); end; }
            field(NotSaved; Rec.Descr) { trigger OnValidate() var Log: Codeunit "SR Log"; begin Log.Add('validate'); end; }
            part(LinkedP; "SR Linked") { SubPageLink = "Head Code" = field("Code"); }
            part(PlainP; "SR Plain") { }
        }
    }
    trigger OnAfterGetCurrRecord() var Log: Codeunit "SR Log"; begin Log.Add('host'); end;
}
codeunit 51456 "SR Test"
{
    procedure Run(): Text
    var H: Record "SR Head"; L: Record "SR Line"; Log: Codeunit "SR Log"; Doc: TestPage "SR Host"; T: Text;
    begin
        H."Code" := 'H1'; H.Insert();
        L."Head Code" := 'H1'; L."Line No." := 10000; L.Insert(); L."Line No." := 20000; L.Insert();
        Doc.OpenEdit();
        Log.Reset(); Doc.NotSaved.SetValue('a'); T := 'plain=' + Log.Dump();
        Log.Reset(); Doc.Saved.SetValue('b'); T += '|saved=' + Log.Dump();
        Doc.Close();
        exit(T);
    end;
}
"#;

#[test]
fn a_save_record_in_a_host_validate_rereads_the_linked_parts() {
    al_builtins::assumptions::clear_hits();
    let r = al_testkit::run_in(SRC, "SR Test", "Run");
    assert_eq!(text(r), "plain=validate;|saved=validate;L:10000;");
    assert!(
        al_builtins::assumptions::take_hits()
            .contains(&al_builtins::assumptions::Assumption::HostSaveRecordRefreshesLinkedParts)
    );
}
