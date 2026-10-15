//! Lowering shapes that kept Tests-ERM tests from running once CaptionClassTranslate ran (issue 48).
//! Each test names where the shape comes from and the corpus test that pins its meaning.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const SRC: &str = r#"
table 50900 "ALT Head"
{
    fields { field(1; Id; Integer) { } field(2; Flag; Boolean) { } field(3; Descr; Text[30]) { } }
    keys { key(PK; Id) { } }
    procedure Lookup(Picked: Boolean): Boolean
    begin
        if Picked then begin
            CurrFieldNo := FieldNo(Flag);
            Validate(Flag, true);
        end;
        exit(Picked);
    end;
    procedure Search(): Integer
    begin
        OnBeforeSearch(CurrFieldNo);
        exit(CurrFieldNo);
    end;
    [IntegrationEvent(false, false)]
    local procedure OnBeforeSearch(var CallingFieldNo: Integer)
    begin
    end;
}
table 50901 "ALT Line"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
    procedure GetHead() Head: Record "ALT Head"
    begin
        Head.Id := Id;
        Head.Flag := true;
        Head.Descr := 'from line';
    end;
}
"#;

fn run(body: &str) -> Result<Value, Signal> {
    let src = format!("{SRC}\ncodeunit 50100 C {{ {body} }}");
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
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect::<Vec<Vec<Option<String>>>>();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

/// Base Application's "Sales Alloc. Acc. Mgt." reads `SalesLine.GetSalesHeader()."Prices Including VAT"`
/// (587 Tests-ERM tests). A procedure that returns a record hands back a record value
/// (corpus tableglobals/TestTableGlobalsRecordVariable.al:248, `Row := Maker.Make()`); a field of it reads
/// that value's field.
#[test]
fn a_field_of_a_record_a_procedure_returns_is_read() {
    let r = run(r#"
        procedure F(): Text
        var L: Record "ALT Line";
        begin
            L.Id := 7;
            if L.GetHead().Flag then
                exit(Format(L.GetHead().Id) + ' ' + L.GetHead().Descr);
            exit('no');
        end;"#);
    match r {
        Ok(Value::Text(s)) => assert_eq!(s, "7 from line"),
        other => panic!("{other:?}"),
    }
}

/// Why `routine` of object `object` in `src` cannot be lowered ("" when it lowers).
fn refusal(src: &str, object: &str, routine: &str) -> String {
    lowered(src, object, routine).0
}

/// The lowering refusal of `routine` ("" when it lowers) and its lowered body, as debug text.
fn lowered(src: &str, object: &str, routine: &str) -> (String, String) {
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), format!("{SRC}\n{src}"))],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let oi = l.program.objects.iter().position(|o| o.name == object).unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case(routine))
        .unwrap();
    (
        l.unsupported[oi][ri].first().cloned().unwrap_or_default(),
        format!("{:?}", l.program.objects[oi].routines[ri].body),
    )
}

/// Base Application's "Purchase Invoice" calls `CurrPage.PurchLines.Page.ForceTotalsCalculation()`, and
/// its part page "Purch. Invoice Subform" has an action group named `Page` (243 Tests-ERM tests). From
/// page code, `CurrPage.<Part>.Page` is the page the part shows (corpus
/// handlers/TestPageTempPart_Host.al:32, `CurrPage.Lines.Page.SetRows(TempRow)`); a control of the part
/// page named `Page` does not hide it.
#[test]
fn curr_page_part_page_is_the_part_page_even_beside_a_control_named_page() {
    let src = r#"
page 50201 "ALT Sub"
{
    PageType = ListPart;
    SourceTable = "ALT Line";
    layout { area(Content) { repeater(R) { field(Id; Rec.Id) { } } } }
    actions { area(Processing) { group("Page") { action(Act) { trigger OnAction() begin end; } } } }
    procedure Force() begin end;
}
page 50200 "ALT Host"
{
    PageType = Document;
    SourceTable = "ALT Head";
    layout { area(Content) { field(Id; Rec.Id) { } part(Lines; "ALT Sub") { } } }
    local procedure Recalc() begin CurrPage.Lines.Page.Force(); end;
}
"#;
    assert_eq!(refusal(src, "ALT Host", "Recalc"), "");
}

/// Base Application's "Purchase Header".BuyfromContactLookup (and "Sales Header".SelltoContactLookup) set
/// `CurrFieldNo := FieldNo(..)` before a Validate, after the user picked a contact (191 Tests-ERM tests reach
/// it). No corpus test assigns CurrFieldNo (handlers/TestPageCurrFieldNo_Tests.al only reads it), so what a
/// later OnValidate reads is not pinned: the assignment refuses when it runs, and code that does not reach it
/// runs (as a control add-in call does, `ir::Expr::Refused`).
#[test]
fn assigning_curr_field_no_refuses_only_when_it_runs() {
    let not_reached = run(r#"
        procedure F(): Boolean
        var H: Record "ALT Head";
        begin
            exit(H.Lookup(false));
        end;"#);
    assert!(matches!(not_reached, Ok(Value::Boolean(false))), "{not_reached:?}");
    let reached = run(r#"
        procedure F(): Boolean
        var H: Record "ALT Head";
        begin
            exit(H.Lookup(true));
        end;"#);
    match reached {
        Err(Signal::Unsupported(why)) => assert!(why.contains("CurrFieldNo") && why.contains("not pinned"), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// Base Application's "Account Schedule" report sets `RequestOptionsPage.Caption := ..` when a
/// financial report is named (86 Tests-ERM tests reach it). The corpus pins the Caption setter of a page
/// (handlers/TestPageCaption_Page.al:44), not of a request page: the assignment lowers and refuses when it
/// runs, so code that does not reach it runs.
#[test]
fn a_request_page_caption_assignment_refuses_only_when_it_runs() {
    let src = r#"
report 50300 "ALT RQ"
{
    ProcessingOnly = true;
    requestpage { layout { area(Content) { field(OptF; Opt) { ApplicationArea = All; } } } }
    var Opt: Integer;
    local procedure Named(Name: Text) begin if Name <> '' then RequestOptionsPage.Caption := Name; end;
}
"#;
    let (why, body) = lowered(src, "ALT RQ", "Named");
    assert_eq!(why, "");
    assert!(body.contains("Refused(") && body.contains("not pinned"), "{body}");
}

/// Base Application's "Sales Header".ShouldSearchForCustomerByName passes `CurrFieldNo` to the `var
/// CallingFieldNo` parameter of its OnBeforeShouldSearchForCustomerByName publisher (174 Tests-ERM tests
/// reach it). When nothing changes the parameter, the call is the same as with any variable; a subscriber
/// that writes it would write CurrFieldNo, which no corpus test pins (docs/probes-needed.md): that refuses
/// when it happens.
#[test]
fn curr_field_no_passed_to_a_var_parameter_runs_until_a_subscriber_changes_it() {
    let unchanged = run(r#"
        procedure F(): Integer
        var H: Record "ALT Head";
        begin
            exit(H.Search());
        end;"#);
    assert!(matches!(unchanged, Ok(Value::Integer(0))), "{unchanged:?}");
    let changed = run(r#"
        procedure F(): Integer
        var H: Record "ALT Head";
        begin
            exit(H.Search());
        end;
        [EventSubscriber(ObjectType::Table, Database::"ALT Head", 'OnBeforeSearch', '', false, false)]
        local procedure Sub(var CallingFieldNo: Integer)
        begin
            CallingFieldNo := 5;
        end;"#);
    match changed {
        Err(Signal::Unsupported(why)) => assert!(why.contains("CurrFieldNo") && why.contains("not pinned"), "{why}"),
        other => panic!("{other:?}"),
    }
}
