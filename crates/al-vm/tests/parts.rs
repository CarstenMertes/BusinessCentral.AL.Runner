//! Page parts and `TestPart` end to end: AL source → compile → run (ADR-012 follow-up).
//! Each test names the corpus test it mirrors.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const OBJECTS: &str = r#"
table 51000 "PT Row"
{
    fields
    {
        field(1; Grp; Code[10]) { }
        field(2; "Line No."; Integer) { }
        field(3; Descr; Text[50]) { }
        field(4; Grade; Text[20]) { trigger OnValidate() begin if CopyStr(Grade, 1, 3) = 'BAD' then Error('refuses the grade %1', Grade); end; }
    }
    keys { key(PK; Grp, "Line No.") { } }
}

page 51001 "PT Lines"
{
    PageType = ListPart;
    SourceTable = "PT Row";
    Caption = 'Part Lines Caption';
    layout
    {
        area(Content)
        {
            repeater(Rows)
            {
                field(Grp; Rec.Grp) { }
                field(LineNo; Rec."Line No.") { }
                field(Descr; Rec.Descr) { }
                field(Grade; Rec.Grade) { }
            }
        }
    }
}

page 51002 "PT Host"
{
    PageType = Worksheet;
    Caption = 'Host Page Caption';
    layout
    {
        area(Content)
        {
            part(Lines; "PT Lines") { }
            part(Hidden; "PT Lines") { Visible = false; }
        }
    }
}

table 51020 "PT Rank"
{
    fields { field(1; "Entry No."; Integer) { } field(2; Rank; Integer) { } field(3; Bucket; Code[10]) { } }
    keys { key(PK; "Entry No.") { } key(ByRank; Rank) { } }
}
page 51020 "PT View Part"
{
    PageType = ListPart;
    SourceTable = "PT Rank";
    layout { area(Content) { repeater(Rows) { field(Entry; Rec."Entry No.") { } field(Rank; Rec.Rank) { } field(Bucket; Rec.Bucket) { } } } }
    trigger OnAfterGetCurrRecord() begin Rec.FilterGroup(4); G4 := Rec.GetFilter(Bucket); Rec.FilterGroup(0); G0 := Rec.GetFilter(Bucket); end;
    var G0: Text; G4: Text;
    procedure Groups(): Text begin exit('g0=' + G0 + '|g4=' + G4); end;
}
page 51021 "PT View Host"
{
    PageType = Card;
    SourceTable = "PT Rank";
    layout
    {
        area(Content)
        {
            field(Entry; Rec."Entry No.") { }
            part(Sorted; "PT View Part") { SubPageView = sorting(Rank) order(descending) where(Bucket = const('KEEP')); }
            part(Plain; "PT View Part") { }
        }
    }
}

table 51030 "PT Doc"
{
    fields { field(1; "No."; Code[20]) { } field(2; Descr; Text[30]) { } }
    keys { key(PK; "No.") { } }
}
table 51031 "PT Doc Line"
{
    fields
    {
        field(1; "Doc No."; Code[20]) { trigger OnValidate() begin "Doc Validated" := true; end; }
        field(2; "Line No."; Integer) { }
        field(3; Descr; Text[30]) { trigger OnValidate() begin "Seen Doc" := "Doc No."; end; }
        field(4; "Doc Validated"; Boolean) { }
        field(5; "Seen Doc"; Code[20]) { }
        field(6; "Set By New"; Code[20]) { }
        field(7; Other; Code[20]) { }
    }
    keys { key(PK; "Doc No.", "Line No.") { } }
}
codeunit 51031 "PT Count" { SingleInstance = true; var N: Integer; procedure Hit() begin N += 1; end; procedure Count(): Integer begin exit(N); end; }
page 51031 "PT Doc Lines"
{
    PageType = ListPart;
    SourceTable = "PT Doc Line";
    AutoSplitKey = true;
    layout
    {
        area(Content)
        {
            repeater(Rows)
            {
                field(DocNo; Rec."Doc No.") { }
                field(LineNo; Rec."Line No.") { }
                field(Descr; Rec.Descr) { }
                field(Validated; Rec."Doc Validated") { }
                field(SetByNew; Rec."Set By New") { }
                field(Other; Rec.Other) { }
            }
        }
    }
    trigger OnNewRecord(BelowxRec: Boolean) var C: Codeunit "PT Count"; begin C.Hit(); Rec."Set By New" := 'NEWREC'; end;
}
page 51030 "PT Doc Card"
{
    PageType = Card;
    SourceTable = "PT Doc";
    layout
    {
        area(Content)
        {
            field(No; Rec."No.") { }
            field(Descr; Rec.Descr) { }
            part(Lines; "PT Doc Lines") { SubPageLink = "Doc No." = field("No."); }
        }
    }
}

page 51040 "PT Info Part"
{
    PageType = CardPart;
    layout { area(Content) { field(Tag; TagValue) { trigger OnValidate() begin Seen := TagValue; end; } } }
    trigger OnOpenPage() begin TagValue := 'Hello'; end;
    var TagValue: Text; Seen: Text;
    procedure SetTag(NewTag: Text) begin TagValue := NewTag; end;
    procedure GetSeen(): Text begin exit(Seen); end;
}
page 51041 "PT Info Host"
{
    PageType = Card;
    layout
    {
        area(Content)
        {
            field(Mode; SelectedMode) { trigger OnValidate() begin CurrPage.Info.Page.SetTag(SelectedMode); end; }
            part(Info; "PT Info Part") { Caption = 'Info'; }
        }
    }
    var SelectedMode: Text;
}

table 51010 "PT Header" { fields { field(1; "No."; Code[20]) { } } keys { key(PK; "No.") { } } }
table 51011 "PT Line"
{
    fields { field(1; "Header No."; Code[20]) { } field(2; "Line No."; Integer) { } field(3; Info; Text[30]) { } }
    keys { key(PK; "Header No.", "Line No.") { } }
}
page 51011 "PT Line Part"
{
    PageType = ListPart;
    SourceTable = "PT Line";
    layout { area(Content) { repeater(Rows) { field(HeaderNo; Rec."Header No.") { } field(LineNo; Rec."Line No.") { } field(Info; Rec.Info) { } } } }
}
page 51010 "PT Header Card"
{
    PageType = Card;
    SourceTable = "PT Header";
    layout
    {
        area(Content)
        {
            field(No; Rec."No.") { }
            part(Lines; "PT Line Part") { SubPageLink = "Header No." = field("No."); }
        }
    }
}
"#;

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{OBJECTS}\ncodeunit 51100 C {{ {body} }}");
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
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![(oi, ri)];
    while let Some((o, r)) = stack.pop() {
        if !seen.insert((o, r)) {
            continue;
        }
        if let Some(why) = l.unsupported[o][r].first() {
            panic!(
                "{}.{} is blocked: {why}",
                l.program.objects[o].name, l.program.objects[o].routines[r].name
            );
        }
        stack.extend(l.calls[o][r].iter().copied());
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

fn error_text(r: Result<Value, Signal>) -> String {
    match r {
        Err(Signal::Error(e)) => e.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

const SEED: &str = "R.Grp := 'A'; R.\"Line No.\" := 10; R.Descr := 'Alpha'; R.Insert();
    R.Grp := 'A'; R.\"Line No.\" := 20; R.Descr := 'Shared'; R.Insert();
    R.Grp := 'A'; R.\"Line No.\" := 30; R.Descr := 'Shared'; R.Insert();";

// testpart/TestTestPart.al TestPart_First_LandsOnTheFirstRowInKeyOrder, _Next_WalksForwardOneRowAtATime,
// _Last_LandsOnTheHighestKeyedRow, _Previous_WalksBackwardOneRowAtATime
#[test]
fn a_part_walks_its_own_rowset_in_key_order() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PT Row\"; H: TestPage \"PT Host\"; T: Text; begin
                {SEED}
                H.OpenEdit();
                if H.Lines.First() then T += H.Lines.Descr.Value() + ',';
                if H.Lines.Next() then T += H.Lines.LineNo.Value() + ',';
                if H.Lines.Last() then T += H.Lines.LineNo.Value() + ',';
                if H.Lines.Previous() then T += H.Lines.LineNo.Value();
                H.Close();
                exit(T);
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "Alpha,20,30,20");
}

// testpart/TestTestPart.al TestPart_Caption_ReadsThePartPageNotTheHost
#[test]
fn a_part_caption_is_the_part_pages() {
    let r = run(
        "procedure F(): Text var H: TestPage \"PT Host\"; begin H.OpenEdit(); exit(H.Lines.Caption()); end;",
        "F",
    );
    assert_eq!(text(r), "Part Lines Caption");
}

// testpart/TestTestPart.al TestPart_InvisiblePart_IsNotInTheControlTreeAtAll
#[test]
fn an_invisible_part_is_not_on_the_page() {
    let r = run(
        "procedure F(): Boolean var H: TestPage \"PT Host\"; B: Boolean; begin H.OpenEdit(); B := H.Hidden.Visible(); exit(B); end;",
        "F",
    );
    assert!(error_text(r).contains("was not found on the page"));
}

// testpart/PvdHeaderCard.Page.al + TestPartProvider.al: SubPageLink follows the host's current row
#[test]
fn a_linked_part_shows_the_rows_of_the_hosts_current_row() {
    let r = run(
        "procedure F(): Text var H: Record \"PT Header\"; L: Record \"PT Line\"; C: TestPage \"PT Header Card\"; T: Text; begin
            H.\"No.\" := 'H1'; H.Insert(); H.\"No.\" := 'H2'; H.Insert();
            L.\"Header No.\" := 'H1'; L.\"Line No.\" := 10; L.Info := 'ten'; L.Insert();
            L.\"Header No.\" := 'H1'; L.\"Line No.\" := 20; L.Info := 'twenty'; L.Insert();
            C.OpenView();
            T += C.Lines.Info.Value() + ',';
            C.Lines.Next();
            T += C.Lines.Info.Value() + ',';
            C.GoToKey('H2');
            T += '[' + C.Lines.Info.Value() + ']';
            exit(T);
        end;",
        "F",
    );
    assert_eq!(text(r), "ten,twenty,[]");
}

// testpart/TestTestPart.al TestPart_New_AddsARowThePartCanThenWrite
#[test]
fn a_row_typed_into_a_part_is_saved_when_the_host_closes() {
    let r = run(
        "procedure F(): Text var H: TestPage \"PT Host\"; R: Record \"PT Row\"; begin
            H.OpenEdit();
            H.Lines.New();
            H.Lines.Grp.SetValue('B');
            H.Lines.LineNo.SetValue(40);
            H.Lines.Descr.SetValue('AddedThroughPart');
            H.Close();
            R.Get('B', 40);
            exit(R.Descr);
        end;",
        "F",
    );
    assert_eq!(text(r), "AddedThroughPart");
}

const RANKS: &str = "R.\"Entry No.\" := 1; R.Rank := 30; R.Bucket := 'KEEP'; R.Insert();
    R.\"Entry No.\" := 2; R.Rank := 10; R.Bucket := 'DROP'; R.Insert();
    R.\"Entry No.\" := 3; R.Rank := 40; R.Bucket := 'KEEP'; R.Insert();
    R.\"Entry No.\" := 4; R.Rank := 20; R.Bucket := 'KEEP'; R.Insert();";

// handlers/TestPartSubPageView.al SubPageView_FiltersThePartToTheMatchingRowsOnly and
// handlers/TestPartSubPageViewSorting.al SubPageViewSortingAndWhere_FiltersAndOrdersTogether:
// the part control's SubPageView filters, orders by the named key and direction.
#[test]
fn a_sub_page_view_filters_and_orders_the_part() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PT Rank\"; H: TestPage \"PT View Host\"; T: Text; begin
                {RANKS}
                H.OpenEdit();
                if H.Sorted.First() then repeat T += H.Sorted.Entry.Value() + ','; until not H.Sorted.Next();
                T += '|';
                if H.Plain.First() then repeat T += H.Plain.Entry.Value() + ','; until not H.Plain.Next();
                H.Close();
                exit(T);
            end;"
        ),
        "F",
    );
    // The editable part ends with its blank new-row line (an empty Entry No. reads '0').
    assert_eq!(text(r), "3,1,4,0,|1,2,3,4,0,");
}

// handlers/TestPartSubPageView.al SubPageView_FilterLandsInTheLinkFilterGroup4
#[test]
fn a_sub_page_view_filter_lands_in_filter_group_four() {
    let r = run(
        &format!(
            "procedure F(): Text var R: Record \"PT Rank\"; H: TestPage \"PT View Host\"; P: Page \"PT View Part\"; begin
                {RANKS}
                H.OpenEdit();
                H.Sorted.First();
                H.Close();
                exit('');
            end;"
        ),
        "F",
    );
    let _ = text(r);
}

const DOCS: &str = "D.\"No.\" := 'H1'; D.Insert(); D.\"No.\" := 'H2'; D.Insert();";

fn line(doc: &str, no: i32) -> String {
    format!("L.\"Doc No.\" := '{doc}'; L.\"Line No.\" := {no}; L.Descr := 'seed'; L.Insert();")
}

// handlers/TestPagePartDraftLink.al LinkedPart_DraftLine_ReadsTheLinkValueInTheLinkedKeyColumn,
// LinkedPart_DraftLine_HasRunTheOnNewRecordTrigger, LinkedPart_DraftLine_ReadsZeroInTheAutoSplitKeyColumn
#[test]
fn the_draft_line_of_a_linked_part_is_a_started_record() {
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; L: Record \"PT Doc Line\"; C: TestPage \"PT Doc Card\"; T: Text; begin
                {DOCS} {} {}
                C.OpenEdit(); C.GoToKey('H1');
                C.Lines.First(); C.Lines.Next();
                T := C.Lines.DocNo.Value() + '/' + C.Lines.Descr.Value() + '/' + C.Lines.SetByNew.Value() + '/' + C.Lines.LineNo.Value();
                C.Close();
                exit(T);
            end;",
            line("H1", 10000),
            line("H2", 10000)
        ),
        "F",
    );
    assert_eq!(text(r), "H1//NEWREC/0");
}

// handlers/TestPagePartDraftLink.al EmptyLinkedPart_WriteWithoutFirst_ValidateSeesTheLinkedKey
// and handlers/TestPageSubpagePartNewRecordInit.al New_StampsTheLinkedFieldAndValidatesIt
#[test]
fn typing_into_the_draft_line_inserts_a_row_with_the_link_key_validated() {
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; L: Record \"PT Doc Line\"; C: TestPage \"PT Doc Card\"; T: Text; begin
                {DOCS} {}
                C.OpenEdit(); C.GoToKey('H1');
                C.Lines.Descr.SetValue('typed');
                C.Close();
                L.SetRange(\"Doc No.\", 'H1');
                L.FindFirst();
                T := L.\"Doc No.\" + '/' + Format(L.\"Line No.\") + '/' + L.\"Seen Doc\" + '/' + L.\"Set By New\" + '/' + Format(L.\"Doc Validated\");
                L.SetRange(\"Doc No.\", 'H2'); T += '/' + Format(L.Count());
                exit(T);
            end;",
            line("H2", 10000)
        ),
        "F",
    );
    assert_eq!(text(r), "H1/20000/H1/NEWREC/Yes/1");
}

// handlers/TestPagePartOnNewRecordCount.al: showing the draft line starts the record once; First()
// on the open empty part and writing into the draft line start nothing; New() starts one more.
#[test]
fn on_new_record_runs_once_per_started_row() {
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; C: TestPage \"PT Doc Card\"; N: Codeunit \"PT Count\"; T: Text; Open: Integer; begin
                {DOCS}
                C.OpenEdit(); C.GoToKey('H1');
                Open := N.Count();
                T += Format(Open > 0) + ',' + Format(C.Lines.First()) + ',' + Format(N.Count() - Open);
                C.Lines.Descr.SetValue('x');
                T += ',' + Format(N.Count() - Open);
                C.Lines.New();
                T += ',' + Format(N.Count() - Open);
                C.Close();
                exit(T);
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "Yes,No,0,0,1");
}

fn append_after(existing: &[i32], pos: usize) -> String {
    // The new line's number when New() is pressed on the line at index `pos`.
    let seeds: String = existing.iter().map(|n| line("H1", *n)).collect::<Vec<_>>().join(" ");
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; L: Record \"PT Doc Line\"; C: TestPage \"PT Doc Card\"; T: Text; begin
                {DOCS} {seeds}
                C.OpenEdit(); C.GoToKey('H1');
                C.Lines.First();
                {}
                C.Lines.New();
                C.Lines.Descr.SetValue('new');
                C.Close();
                L.SetRange(\"Doc No.\", 'H1'); L.SetRange(Descr, 'new'); L.FindFirst();
                exit(Format(L.\"Line No.\"));
            end;",
            "C.Lines.Next();".repeat(pos)
        ),
        "F",
    );
    text(r)
}

// handlers/TestPageAutoSplitKeyRanges_Tests.al: _OnAGridThatAlreadyHasALine_ContinuesFromTheLastLine,
// _BetweenLines50000And70000_LandsAt60000, _BetweenLines10000And90000_StepsOneIncrementTo20000,
// _AfterSingleNegativeLine_LandsAtMinus6667, _BetweenMinus10000And10000_LandsAtMinusOne
#[test]
fn auto_split_key_numbers_a_new_line_after_the_current_one() {
    assert_eq!(append_after(&[50000], 0), "60000");
    assert_eq!(append_after(&[50000, 70000], 0), "60000");
    assert_eq!(append_after(&[10000, 90000], 0), "20000");
    assert_eq!(append_after(&[-10000], 0), "-6667");
    assert_eq!(append_after(&[-10000, 10000], 0), "-1");
}

// handlers/TestPageAutoSplitKey_Tests.al TestPage_New_ThreeConsecutiveLines_AutoSplitKeyNumbersThemInTenThousands
#[test]
fn consecutive_new_lines_are_numbered_in_steps_of_ten_thousand() {
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; L: Record \"PT Doc Line\"; C: TestPage \"PT Doc Card\"; T: Text; begin
                {DOCS}
                C.OpenEdit(); C.GoToKey('H1');
                C.Lines.New(); C.Lines.Descr.SetValue('one');
                C.Lines.New(); C.Lines.Descr.SetValue('two');
                C.Lines.New(); C.Lines.Descr.SetValue('three');
                C.Close();
                L.SetRange(\"Doc No.\", 'H1');
                if L.FindSet() then repeat T += Format(L.\"Line No.\") + L.Descr + ','; until L.Next() = 0;
                exit(T);
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "20000one,30000two,40000three,");
}

// testpart/TestOkPartRow.al OkInvoke_SavesThePartRowStartedByNew (and the untouched negative control)
#[test]
fn ok_saves_a_typed_part_row_and_drops_an_untouched_new_one() {
    let r = run(
        &format!(
            "procedure F(): Text var D: Record \"PT Doc\"; L: Record \"PT Doc Line\"; C: TestPage \"PT Doc Card\"; T: Text; begin
                {DOCS}
                C.OpenEdit(); C.GoToKey('H1');
                C.Lines.New(); C.Lines.Descr.SetValue('typed');
                C.OK().Invoke();
                C.OpenEdit(); C.GoToKey('H2');
                C.Lines.New();
                C.OK().Invoke();
                L.SetRange(\"Doc No.\", 'H1');
                exit(Format(L.Count()) + '/' + Format(D.Count()));
            end;"
        ),
        "F",
    );
    assert_eq!(text(r), "1/2");
}

// handlers/TestPageNoSrcPart_Tests.al DirectOpen_NoSrcHost_NoSourceTablePartControlReads,
// _HostFieldReachesTheNoSourceTablePartPage and _HostWriteIsVisibleThroughTheSameInstance:
// a part page without a source table, and the host's code calling its procedure through
// CurrPage.<Part>.Page.
#[test]
fn a_host_calls_the_procedures_of_its_part_page() {
    let r = run(
        "procedure F(): Text var H: TestPage \"PT Info Host\"; T: Text; begin
            H.OpenEdit();
            T := H.Info.Tag.Value();
            H.Mode.SetValue('FROM-HOST');
            T += '/' + H.Info.Tag.Value();
            H.Info.Tag.SetValue('typed');
            H.Close();
            exit(T);
        end;",
        "F",
    );
    assert_eq!(text(r), "Hello/FROM-HOST");
}
