//! CaptionClass areas other than 3 on table fields and page controls (record/lt-qd-captionclass-areas/QdCaptionClassAreas.al,
//! writer QD, corpus bump 34). The subscriber stands in for Base Application's VAT resolver (area 2).

use al_testkit::text;

const OBJECTS: &str = r#"
codeunit 42 "Caption Class" { [IntegrationEvent(false, false)] procedure OnResolveCaptionClass(CaptionArea: Text; CaptionExpr: Text; Language: Integer; var Caption: Text; var Resolved: Boolean) begin end; }
codeunit 50401 Vat
{
    SingleInstance = true;
    var Log: Text;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Caption Class", 'OnResolveCaptionClass', '', false, false)]
    local procedure OnResolve(CaptionArea: Text; CaptionExpr: Text; Language: Integer; var Caption: Text; var Resolved: Boolean)
    begin
        Log += 'resolve(' + CaptionArea + ',' + CaptionExpr + ')';
        if (CaptionArea = '2') and CaptionExpr.StartsWith('0,') then begin
            Caption := CopyStr(CaptionExpr, 3) + ' Excl. VAT';
            Resolved := true;
        end;
    end;
    procedure Take(): Text var T: Text; begin T := Log; Log := ''; exit(T); end;
}
codeunit 50402 Calls
{
    SingleInstance = true;
    var Log: Text; Count: Integer;
    procedure Hit(FieldNumber: Integer; RecId: Integer) begin Count += 1; Log += Format(FieldNumber) + '@' + Format(RecId) + ';'; end;
    procedure Take(): Text var T: Text; begin T := Log; Log := ''; exit(T); end;
    procedure Total(): Integer begin exit(Count); end;
}
table 50400 CapRow
{
    fields
    {
        field(1; Id; Integer) { }
        field(10; Lit; Decimal) { Caption = 'Cap Lit'; CaptionClass = '2,0,Inv. Discount Amount'; }
        field(11; Unresolved; Decimal) { Caption = 'Cap Unresolved'; CaptionClass = '9,x'; }
        field(20; ExprVat; Decimal) { Caption = 'Cap Expr Vat'; CaptionClass = Cc(20); }
        field(21; ExprEmpty; Integer) { Caption = 'Cap Expr Empty'; CaptionClass = Cc(21); }
        field(22; Expr3; Integer) { Caption = 'Cap Expr 3'; CaptionClass = Cc(22); }
        field(23; ExprArea; Integer) { Caption = 'Cap Expr Area'; CaptionClass = Cc(23); }
        field(24; ExprId; Integer) { Caption = 'Cap Expr Id'; CaptionClass = Cc(24); }
    }
    keys { key(PK; Id) { } }
    procedure Cc(FieldNumber: Integer): Text[80]
    var C: Codeunit Calls;
    begin
        C.Hit(FieldNumber, Id);
        case FieldNumber of
            20: exit('2,0,Amount');
            21: exit('');
            22: exit('3,');
            23: exit('AB,x');
            24: exit('3,Id ' + Format(Id) + ' call ' + Format(C.Total()));
        end;
    end;
}
page 50403 CapCard
{
    PageType = Card;
    SourceTable = CapRow;
    layout
    {
        area(Content)
        {
            field(Id; Rec.Id) { }
            field(Lit; Rec.Lit) { }
            field(ExprVat; Rec.ExprVat) { }
            field(ExprEmpty; Rec.ExprEmpty) { }
            field(ExprId; Rec.ExprId) { }
        }
    }
}
"#;

fn run(body: &str) -> String {
    let src = format!("{OBJECTS}\ncodeunit 50409 T\n{{\n{body}\n}}\n");
    text(al_testkit::run_in(&src, "T", "F"))
}

/// QdCaptionClassAreas.al:233-258 Literals_FieldCaption (`lit20`): a literal area 2 goes to OnResolveCaptionClass; P84Captions.al
/// (Table_Log_A after Table_NoSubscriber): a text the session translated raises no event again; a text nobody resolves stays.
#[test]
fn a_literal_numeric_area_is_translated_once_per_session() {
    let got = run(r#"
    procedure F(): Text
    var R: Record CapRow; V: Codeunit Vat; S: Text;
    begin
        V.Take();
        S := R.FieldCaption(Lit) + '/' + V.Take() + '|';
        S += R.FieldCaption(Lit) + '/' + V.Take() + '|';
        S += R.FieldCaption(Unresolved) + '/' + V.Take();
        exit(S);
    end;"#);
    assert_eq!(
        got,
        "Inv. Discount Amount Excl. VAT/resolve(2,0,Inv. Discount Amount)|Inv. Discount Amount Excl. VAT/|9,x/resolve(9,x)"
    );
}

/// QdCaptionClassAreas.al:262-296 Expressions_FieldCaption and :323-347 Expression_PerRead_CallCount: the expression runs once
/// per read with the reading record; its result goes through the resolvers; '' and '3,' read as the field's Caption.
#[test]
fn an_expression_runs_per_read_and_its_result_is_resolved() {
    let got = run(r#"
    procedure F(): Text
    var R: Record CapRow; C: Codeunit Calls; S: Text;
    begin
        R.Id := 7;
        S := R.FieldCaption(ExprVat) + '|' + R.FieldCaption(ExprEmpty) + '|' + R.FieldCaption(Expr3) + '|';
        S += R.FieldCaption(ExprId) + '|' + R.FieldCaption(ExprId) + '|' + C.Take();
        exit(S);
    end;"#);
    assert_eq!(
        got,
        "Amount Excl. VAT|Cap Expr Empty|Cap Expr 3|Id 7 call 4|Id 7 call 5|20@7;21@7;22@7;24@7;24@7;"
    );
}

/// An expression result whose area is not a number is not pinned: refused.
#[test]
fn an_expression_result_with_an_area_that_is_not_a_number_is_refused() {
    let src = format!(
        "{OBJECTS}\ncodeunit 50409 T {{ procedure F(): Text var R: Record CapRow; begin exit(R.FieldCaption(ExprArea)); end; }}"
    );
    let r = al_testkit::run_in(&src, "T", "F");
    assert!(
        matches!(&r, Err(al_builtins::native::Signal::Unsupported(w)) if w.contains("not a number")),
        "{r:?}"
    );
}

/// QdCaptionClassAreas.al:422-481 Page_ControlCaptions: OpenView of a card runs every expression field 20 times with an empty
/// record, then once per control with the current row; Caption() runs nothing; an empty result reads as '' on a control.
#[test]
fn a_card_open_runs_the_expressions_and_caption_reads_what_it_filled() {
    al_builtins::assumptions::clear_hits();
    let got = run(r#"
    procedure F(): Text
    var R: Record CapRow; C: Codeunit Calls; P: TestPage CapCard; Calls: Text; Empty: Integer; S: Text;
    begin
        R.Id := 3; R.Insert();
        C.Take();
        P.OpenView();
        Calls := C.Take();
        while Calls.Contains('@0;') do begin
            Empty += 1;
            Calls := Calls.Remove(Calls.IndexOf('@0;') - 2, 5);
        end;
        S := Format(Empty) + '|' + Calls + '|';
        S += P.Lit.Caption() + '|' + P.ExprVat.Caption() + '|' + P.ExprEmpty.Caption() + '|' + P.ExprId.Caption() + '|';
        S += P.ExprId.Caption() + '|' + C.Take();
        P.Close();
        exit(S);
    end;"#);
    assert_eq!(
        got,
        "100|20@3;21@3;24@3;|Inv. Discount Amount Excl. VAT|Amount Excl. VAT||Id 3 call 103|Id 3 call 103|"
    );
    assert_eq!(al_builtins::assumptions::take_hits(), vec![]);
}
