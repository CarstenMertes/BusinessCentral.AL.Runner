//! What the parts of a host read when the host moves to another row (lane L3).
//! Mirrors pages/lt-part-reads/PartReadsOpen.al (Next_SixParts_ReadsPerPartAfterHostMove,
//! LastPrevious_SixParts_ReadsPerPartAfterHostMove) and pages/lt-part-subpageview-filter/PartViewFilter.al:381.

use al_testkit::text;

const SRC: &str = r#"
table 51400 "MV Head" { fields { field(1; "Code"; Code[20]) { } } keys { key(PK; "Code") { } } }
table 51401 "MV Line"
{
    fields { field(1; "Head Code"; Code[20]) { } field(2; "Line No."; Integer) { } }
    keys { key(PK; "Head Code", "Line No.") { } }
}
codeunit 51402 "MV Log"
{
    SingleInstance = true;
    var T: Text;
    procedure Add(S: Text) begin T += S + ';'; end;
    procedure Reset() begin T := ''; end;
    procedure Dump(): Text begin exit(T); end;
}
page 51403 "MV Plain"
{
    PageType = ListPart;
    SourceTable = "MV Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetRecord() var Log: Codeunit "MV Log"; begin Log.Add('P:' + Rec."Head Code" + '/' + Format(Rec."Line No.")); end;
}
page 51404 "MV Linked"
{
    PageType = ListPart;
    SourceTable = "MV Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetRecord() var Log: Codeunit "MV Log"; begin Log.Add('L:' + Rec."Head Code" + '/' + Format(Rec."Line No.")); end;
}
page 51405 "MV Filtered"
{
    PageType = ListPart;
    SourceTable = "MV Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetRecord() var Log: Codeunit "MV Log"; begin Log.Add('F:' + Rec."Head Code" + '/' + Format(Rec."Line No.")); end;
}
page 51406 "MV Hidden"
{
    PageType = ListPart;
    SourceTable = "MV Line";
    layout { area(Content) { repeater(R) { field(L; Rec."Line No.") { } } } }
    trigger OnAfterGetRecord() var Log: Codeunit "MV Log"; begin Log.Add('H:' + Rec."Head Code" + '/' + Format(Rec."Line No.")); end;
}
page 51407 "MV Host"
{
    PageType = Card;
    SourceTable = "MV Head";
    layout
    {
        area(Content)
        {
            field(CodeCtl; Rec."Code") { }
            part(PlainP; "MV Plain") { }
            part(LinkedP; "MV Linked") { SubPageLink = "Head Code" = field("Code"); }
            part(FilteredP; "MV Filtered") { SubPageView = where("Line No." = filter('>10000')); SubPageLink = "Head Code" = field("Code"); }
            part(HiddenP; "MV Hidden") { Visible = ShowHidden; SubPageLink = "Head Code" = field("Code"); }
        }
    }
    var ShowHidden: Boolean;
}
codeunit 51408 "MV Test"
{
    local procedure Seed()
    var H: Record "MV Head"; L: Record "MV Line";
    begin
        H."Code" := 'H1'; H.Insert(); H."Code" := 'H2'; H.Insert(); H."Code" := 'H3'; H.Insert();
        L."Head Code" := 'H1'; L."Line No." := 10000; L.Insert(); L."Line No." := 20000; L.Insert(); L."Line No." := 30000; L.Insert();
        L."Head Code" := 'H2'; L."Line No." := 10000; L.Insert(); L."Line No." := 20000; L.Insert();
        L."Head Code" := 'H3'; L."Line No." := 10000; L.Insert();
    end;
    procedure Moves(): Text
    var Log: Codeunit "MV Log"; Doc: TestPage "MV Host"; T: Text;
    begin
        Seed();
        Doc.OpenView();
        Log.Reset(); Doc.Next(); T := 'next=' + Log.Dump();
        Log.Reset(); Doc.Last(); T += '|last=' + Log.Dump();
        Log.Reset(); Doc.Previous(); T += '|previous=' + Log.Dump();
        Log.Reset(); Doc.First(); T += '|first=' + Log.Dump();
        Doc.Close();
        exit(T);
    end;
}
"#;

/// PartReadsOpen.al:269-283 (Next: no Plain reads, linked parts read forward and put the first row back), :336-379
/// (Last forward; Previous and First read the linked parts last to first, a one-row part once).
#[test]
fn a_host_move_reads_only_its_linked_parts() {
    let r = al_testkit::run_in(SRC, "MV Test", "Moves");
    assert_eq!(
        text(r),
        "next=L:H2/10000;L:H2/20000;L:H2/10000;F:H2/20000;H:H2/10000;H:H2/20000;H:H2/10000;\
         |last=L:H3/10000;H:H3/10000;\
         |previous=L:H2/20000;L:H2/10000;F:H2/20000;H:H2/20000;H:H2/10000;\
         |first=L:H1/30000;L:H1/20000;L:H1/10000;F:H1/30000;F:H1/20000;H:H1/30000;H:H1/20000;H:H1/10000;"
    );
}

const UPDATE_SRC: &str = r#"
table 51410 "UP Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 51411 "UP Log"
{
    SingleInstance = true;
    var T: Text;
    procedure Add(S: Text) begin T += S + ';'; end;
    procedure Reset() begin T := ''; end;
    procedure Dump(): Text begin exit(T); end;
}
page 51412 "UP Part"
{
    PageType = ListPart;
    SourceTable = "UP Row";
    layout { area(Content) { repeater(R) { field(IdCtl; Rec.Id) { } } } }
    procedure RunUpdate() var Log: Codeunit "UP Log"; begin Log.Add('proc'); CurrPage.Update(false); end;
    trigger OnOpenPage() var Log: Codeunit "UP Log"; begin Log.Add('part-open'); end;
    trigger OnAfterGetRecord() var Log: Codeunit "UP Log"; begin Log.Add('agr' + Format(Rec.Id)); end;
}
page 51413 "UP Host"
{
    PageType = Card;
    SourceTable = "UP Row";
    layout { area(Content) { field(IdCtl; Rec.Id) { } part(PartCtl; "UP Part") { Visible = ShowPart; } } }
    var ShowPart: Boolean;
    trigger OnAfterGetCurrRecord() var Log: Codeunit "UP Log"; begin Log.Add('host'); CurrPage.PartCtl.Page.RunUpdate(); end;
}
codeunit 51414 "UP Test"
{
    procedure Run(): Text
    var R: Record "UP Row"; Log: Codeunit "UP Log"; Host: TestPage "UP Host"; T: Text;
    begin
        R.Id := 1; R.Insert(); R.Id := 2; R.Insert();
        Log.Reset();
        Host.OpenEdit();
        T := 'open=' + Log.Dump();
        Log.Reset();
        Host.Next();
        T += '|next=' + Log.Dump();
        Host.Close();
        exit(T);
    end;
}
"#;

/// A part hidden by a Visible expression whose procedure, called from the host's OnAfterGetCurrRecord, runs
/// `CurrPage.Update`: the part fills its rows again after the host's code (pages/lt-hidden-part-currpage/
/// HiddenPartCurrPageUpdateFalse.al:196-198).
#[test]
fn a_part_update_asked_by_host_code_refills_the_part() {
    let r = al_testkit::run_in(UPDATE_SRC, "UP Test", "Run");
    assert_eq!(
        text(r),
        "open=part-open;host;proc;agr1;agr2;agr1;agr1;agr2;agr1;|next=host;proc;agr1;agr2;agr1;"
    );
}

const CLOSE_SRC: &str = r#"
table 51420 "CL Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 51421 "CL Log"
{
    SingleInstance = true;
    var T: Text;
    procedure Add(S: Text) begin T += S + ';'; end;
    procedure Take(): Text var R: Text; begin R := T; T := ''; exit(R); end;
}
page 51422 "CL Part"
{
    PageType = ListPart;
    SourceTable = "CL Row";
    layout { area(Content) { repeater(R) { field(IdCtl; Rec.Id) { } } } }
    trigger OnQueryClosePage(CloseAction: Action): Boolean var Log: Codeunit "CL Log"; begin Log.Add('p.OnQueryClosePage(' + Format(CloseAction) + ')'); exit(true); end;
    trigger OnClosePage() var Log: Codeunit "CL Log"; begin Log.Add('p.OnClosePage'); end;
}
page 51423 "CL Host Expr"
{
    PageType = Card;
    SourceTable = "CL Row";
    layout { area(Content) { field(IdCtl; Rec.Id) { } part(PartCtl; "CL Part") { Visible = ShowPart; } } }
    var ShowPart: Boolean;
}
page 51424 "CL Host Static"
{
    PageType = Card;
    SourceTable = "CL Row";
    layout { area(Content) { field(IdCtl; Rec.Id) { } part(PartCtl; "CL Part") { Visible = false; } } }
}
codeunit 51425 "CL Test"
{
    procedure Run(): Text
    var R: Record "CL Row"; Log: Codeunit "CL Log"; E: TestPage "CL Host Expr"; S: TestPage "CL Host Static"; T: Text;
    begin
        R.Id := 1; R.Insert();
        E.OpenEdit(); Log.Take(); E.Close(); T := 'expr=' + Log.Take();
        S.OpenEdit(); Log.Take(); S.Close(); T += '|static=' + Log.Take();
        exit(T);
    end;
}
"#;

/// A part hidden by a Visible expression is open, so it runs its close triggers with its host; a literal
/// `Visible = false` part never opened and runs none (pages/W1Parts.al:408-411).
#[test]
fn a_part_hidden_by_an_expression_closes_with_its_host() {
    let r = al_testkit::run_in(CLOSE_SRC, "CL Test", "Run");
    assert_eq!(text(r), "expr=p.OnQueryClosePage(None);p.OnClosePage;|static=");
}
