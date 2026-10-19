//! Values and builtins the corpus pinned and the runtime got wrong (branch lane-values-builtins). Each test cites its case.

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn out(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(t) | Value::Code(t)) => t.to_string(),
        Ok(Value::Integer(i)) => i.to_string(),
        Ok(Value::Boolean(b)) => b.to_string(),
        Err(Signal::Error(e)) => format!("error:{}|{}", e.message, e.code),
        Err(Signal::Unsupported(why)) => format!("refused:{why}"),
        other => format!("{other:?}"),
    }
}

fn text(vars: &str, body: &str) -> String {
    let src = format!("codeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    out(al_testkit::run_in(&src, "C", "F"))
}

#[test]
fn last_index_of_on_an_empty_text_takes_start_zero_and_one() {
    // text/lt-text-instance/Search1.Codeunit.al:351,353,355 (LastIndexOf_StartIndex_Empty).
    let r = text(
        "E: Text;",
        "exit(Format(E.LastIndexOf('', 1)) + ',' + Format(E.LastIndexOf('a', 1)) + ',' + Format(E.LastIndexOf('a', 0)));",
    );
    assert_eq!(r, "1,0,0");
}

const NUM_ROW: &str = r#"
table 50110 "Num Row"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(5; "Code Value"; Code[20]) { }
        field(6; "Short Code"; Code[2]) { }
        field(7; "Text Value"; Text[20]) { }
    }
    keys { key(PK; "No.") { } }
}
codeunit 50111 C
{
    [TryFunction]
    local procedure TrySetValue(var RR: RecordRef; FieldNo: Integer; V: Variant)
    begin
        RR.Field(FieldNo).Value := V;
    end;

    procedure F(): Text
    var
        Row: Record "Num Row";
        RR: RecordRef;
        R: Text;
    begin
        Row."No." := 1;
        Row.Insert();
        RR.Open(Database::"Num Row");
        RR.FindFirst();
        R := Format(TrySetValue(RR, 5, 10)) + ':' + Format(RR.Field(5).Value());
        R += ',' + Format(TrySetValue(RR, 5, 2.5)) + ':' + Format(RR.Field(5).Value());
        R += ',' + Format(TrySetValue(RR, 7, -3)) + ':' + Format(RR.Field(7).Value());
        R += ',' + Format(TrySetValue(RR, 6, 12345)) + ':' + Format(RR.Field(6).Value());
        exit(R);
    end;
}
"#;

#[test]
fn fieldref_value_takes_a_number_into_a_code_or_text_field_as_its_format() {
    // types/lt-number-to-text/NumberToTextIsFormat.al:239-246 (FieldRefSetValue_NumberOnCodeAndText_Result).
    assert_eq!(out(al_testkit::run_in(NUM_ROW, "C", "F")), "Yes:10,Yes:2.5,Yes:-3,No:");
}

#[test]
fn session_settings_of_a_fresh_instance_formats_every_property() {
    // session/lt-session-settings-fresh/SessionSettingsFreshReads.al:57 (P161_FreshInstance_FormatText).
    let r = text("S: SessionSettings;", "exit(Format(S));");
    assert_eq!(
        r,
        "Company=, ProfileId=, LanguageId=0, LocaleId=0, TimeZone=, ProfileAppId={00000000-0000-0000-0000-000000000000}, ProfileSystemScope=False"
    );
}

const READ_AS: &str = r#"
codeunit 50120 C
{
    [TryFunction]
    local procedure TryReadAs(var Content: HttpContent; var Result: Text)
    begin
        Content.ReadAs(Result);
    end;

    procedure F(): Text
    var
        Req: HttpRequestMessage;
        Content: HttpContent;
        Result: Text;
    begin
        Content := Req.Content;
        if TryReadAs(Content, Result) then
            exit('ok:[' + Result + ']');
        exit('error:' + GetLastErrorText() + '|' + GetLastErrorCode());
    end;
}
"#;

#[test]
fn read_as_of_an_uninitialized_content_as_a_statement_raises() {
    // misc/W4XmlJsonHttp.al:343 (RequestWithoutContent_ReadAs).
    assert_eq!(
        out(al_testkit::run_in(READ_AS, "C", "F")),
        "error:Microsoft.Dynamics.Nav.Runtime.NavHttpContent variable not initialized.|NotInitialized"
    );
}

#[test]
fn an_interface_assigned_from_an_unassigned_one_raises() {
    // codeunit/lt-enums-interfaces/OjT71047.al:250-251 (Uninitialized_AssignedFromUninitialized, af224).
    let src = r#"
interface IShape { procedure GetArea(): Integer; }
codeunit 50130 Sq implements IShape { procedure GetArea(): Integer begin exit(4); end; }
codeunit 50131 C
{
    procedure F(): Text
    var
        A: Interface IShape;
        B: Interface IShape;
    begin
        B := A;
        exit('no error');
    end;
}
"#;
    assert_eq!(
        out(al_testkit::run_in(src, "C", "F")),
        "error:Interface not initialized|InvalidOperation"
    );
}

#[test]
fn a_variant_beyond_integer_into_an_integer_raises_a_type_conversion_overflow() {
    // types/lt-filter-enum-relation-duration-guid/D_Duration.al:226 (a Duration); types/lt-variant/VaTConvert.al:183-184
    // (a BigInteger).
    let msg = "error:The expression Variant cannot be type-converted to a Integer value.|TypeConversion:Overflow";
    let r = text(
        "V: Variant; B: BigInteger; D: Duration; I: Integer;",
        "Evaluate(B, '5000000000'); D := B; V := D; I := V; exit(Format(I));",
    );
    assert_eq!(r, msg);
    let r = text(
        "V: Variant; B: BigInteger; I: Integer;",
        "Evaluate(B, '3000000000'); V := B; I := V; exit(Format(I));",
    );
    assert_eq!(r, msg);
    // In range the value converts (VaTConvert.al:181).
    let r = text(
        "V: Variant; B: BigInteger; I: Integer;",
        "B := 5; V := B; I := V; exit(Format(I));",
    );
    assert_eq!(r, "5");
}

const CURR: &str = r#"
table 50140 "Curr Row"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            begin
                SeenInValidate := CurrFieldNo;
                SetN(CurrFieldNo);
                SeenAfterInValidate := CurrFieldNo;
            end;
        }
        field(3; Note; Text[50]) { }
    }
    keys { key(PK; Id) { } }
    var
        SeenInValidate: Integer;
        SeenAfterInValidate: Integer;

    local procedure SetN(var N: Integer)
    begin
        N := 99;
    end;

    local procedure SetNAndPassOn(var N: Integer)
    begin
        SetN(N);
    end;

    procedure PassAndAssign(): Text
    var
        Before: Integer;
        After: Integer;
    begin
        Before := CurrFieldNo;
        SetN(CurrFieldNo);
        After := CurrFieldNo;
        exit(Format(Before) + '>' + Format(After));
    end;

    procedure PassTwoLevels(): Text
    begin
        SetNAndPassOn(CurrFieldNo);
        exit(Format(CurrFieldNo));
    end;

    procedure ValidateAmountSeen(): Text
    begin
        CurrFieldNo := 0;
        Validate(Amount, 5);
        exit(Format(SeenInValidate) + '/' + Format(SeenAfterInValidate) + '/' + Format(CurrFieldNo));
    end;

    procedure CopyThenAssign(): Text
    var
        CopyVar: Integer;
    begin
        CopyVar := CurrFieldNo;
        SetN(CopyVar);
        exit(Format(CopyVar) + '/' + Format(CurrFieldNo));
    end;
}
codeunit 50141 C
{
    procedure Pass(): Text
    var
        Row: Record "Curr Row";
    begin
        exit(Row.PassAndAssign() + ',' + Row.PassAndAssign());
    end;

    procedure TwoLevels(): Text
    var
        Row: Record "Curr Row";
    begin
        exit(Row.PassTwoLevels());
    end;

    procedure InValidate(): Text
    var
        Row: Record "Curr Row";
    begin
        exit(Row.ValidateAmountSeen());
    end;

    procedure Copy(): Text
    var
        Row: Record "Curr Row";
    begin
        exit(Row.CopyThenAssign());
    end;
}
"#;

#[test]
fn curr_field_no_passed_to_a_var_parameter_takes_what_the_callee_assigns() {
    // record/lt-curr-field-no/CurrFieldNoVarParam.al:103-106 (CalleeAssigns), :117 (InValidate), :128 (CopyIsNotTheProperty).
    let mut b = al_testkit::Built::from_source(CURR);
    assert_eq!(out(b.run_in("C", "Pass")), "0>99,99>99");
    assert_eq!(out(b.run_in("C", "TwoLevels")), "99");
    assert_eq!(out(b.run_in("C", "InValidate")), "0/99/99");
    assert_eq!(out(b.run_in("C", "Copy")), "99/0");
}

#[test]
fn field_index_lists_flowfields_after_the_other_fields() {
    // metadata/lt-custom-report-layout-columns/CustomReportLayoutColumns.al:33-44 (Columns_Schema: field 3, a FlowField,
    // comes after field 15).
    let src = r#"
table 50150 "Ff Row"
{
    fields
    {
        field(1; A; Integer) { }
        field(2; B; Integer) { }
        field(3; C; Integer) { FieldClass = FlowField; CalcFormula = count("Ff Row"); }
        field(4; D; Integer) { }
    }
    keys { key(PK; A) { } }
}
codeunit 50151 C
{
    procedure F(): Text
    var
        RR: RecordRef;
        i: Integer;
        R: Text;
    begin
        RR.Open(Database::"Ff Row");
        for i := 1 to RR.FieldCount() do
            R += Format(RR.FieldIndex(i).Number()) + ' ';
        exit(R);
    end;
}
"#;
    assert_eq!(out(al_testkit::run_in(src, "C", "F")), "1 2 4 3 ");
}

const STREAM: &str = r#"
table 50160 "St Row"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Data; Blob) { }
    }
    keys { key(PK; Id) { } }
}
codeunit 50161 C
{
    local procedure Put(var Rec: Record "St Row"; Bytes: List of [Integer]; Enc: TextEncoding)
    var
        Out: OutStream;
        B: Byte;
        I: Integer;
    begin
        Rec.DeleteAll();
        Rec.Init();
        Rec.Id := 1;
        Rec.Data.CreateOutStream(Out, Enc);
        foreach I in Bytes do begin
            B := I;
            Out.Write(B);
        end;
        Rec.Insert();
    end;

    procedure Terminated(): Text
    var
        Rec: Record "St Row";
        Ins: InStream;
        T: Text;
        N: Integer;
        R: Text;
        L: List of [Integer];
    begin
        L.AddRange(104, 101, 108, 108, 111, 0, 119, 111, 114, 108, 100, 0);
        Put(Rec, L, TextEncoding::UTF8);
        Rec.CalcFields(Data);
        Rec.Data.CreateInStream(Ins, TextEncoding::UTF8);
        N := Ins.Read(T);
        R := Format(N) + '|' + T;
        N := Ins.Read(T);
        R += ',' + Format(N) + '|' + T;
        N := Ins.Read(T);
        R += ',' + Format(N) + '|' + T;
        exit(R);
    end;

    procedure Unterminated(): Text
    var
        Rec: Record "St Row";
        Ins: InStream;
        T: Text;
        N: Integer;
        R: Text;
        L: List of [Integer];
    begin
        L.AddRange(104, 101, 108, 108, 111);
        Put(Rec, L, TextEncoding::UTF8);
        Rec.CalcFields(Data);
        Rec.Data.CreateInStream(Ins, TextEncoding::UTF8);
        N := Ins.Read(T);
        R := Format(N) + '|' + T;
        N := Ins.Read(T);
        R += ',' + Format(N) + '|' + T;
        exit(R);
    end;
}
"#;

#[test]
fn reading_text_on_a_utf8_stream_counts_one_more_than_the_bytes_and_one_at_the_end() {
    // misc/lt-probes-needed-5/PnE04Streams.al:86-89 (ReadText_Utf8_WithTerminator), :111-113 (ReadText_Utf8_NoTerminator).
    let mut b = al_testkit::Built::from_source(STREAM);
    assert_eq!(out(b.run_in("C", "Terminated")), "6|hello,6|world,1|");
    assert_eq!(out(b.run_in("C", "Unterminated")), "6|hello,1|");
}

fn run_on(bc: &str, src: &str, routine: &str) -> Result<Value, Signal> {
    let target = al_version::Target::for_bc(al_version::BcVersion::parse(bc).unwrap());
    let apps = vec![al_sema::workspace::AppSource {
        id: "a".into(),
        name: "a".into(),
        dependencies: vec![],
        files: vec![("f.al".into(), src.into())],
    }];
    let (ws, l) = al_sema::lower::compile_for(apps, &Default::default(), &target);
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
    let engine = al_vm::Engine::with_target(l.program, blocked, Some(target));
    let mut s = al_vm::Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

#[test]
fn a_header_name_of_blanks_is_an_argument_error_on_bc_28_and_a_format_error_from_29() {
    // network/lt-http/HttpHeaders.al:304-308 (InvalidNames, `blankName`); the test is BC 28 and later (:282).
    let src = "codeunit 50170 C { procedure F(): Text var H: HttpHeaders; R: HttpRequestMessage; begin R.GetHeaders(H); H.Add(' ', 'v'); end; }";
    let code = |bc: &str| match run_on(bc, src, "F") {
        Err(Signal::Error(e)) => e.code,
        Err(Signal::Unsupported(_)) => "refused".into(),
        other => format!("{other:?}"),
    };
    assert_eq!(code("29.0"), "InvalidHeader:Format");
    assert_eq!(code("28.5"), "InvalidHeader:Argument");
    assert_eq!(code("27.5"), "refused");
}
