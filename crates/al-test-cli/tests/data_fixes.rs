//! Behaviour of queries and XmlPorts pinned by probe tests that are not in the corpus submodule yet
//! (corpus branch al-runtime-rs, pending push: query/TestQueryOuterJoinDescendingSaveAsContracts.al,
//! xmlport/TestXmlPortTriggersReplaceViewContracts.al, xmlport/TestXmlPortTextFormatsContracts.al).
//! AL source, compiled and run as a test codeunit. Each test names the probe it mirrors.

use al_sema::workspace::AppSource;
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;

const OBJECTS: &str = r#"
table 64000 "DF Parent" { fields { field(1; "Entry No."; Integer) { } } keys { key(PK; "Entry No.") { } } }
table 64001 "DF Child"
{
    fields { field(1; "Entry No."; Integer) { } field(2; "Parent Entry No."; Integer) { } field(3; Amount; Decimal) { } }
    keys { key(PK; "Entry No.") { } }
}
table 64002 "DF Universal"
{
    fields { field(1; "Entry No."; Integer) { } field(2; "Integer Field"; Integer) { } field(3; "Text Field"; Text[100]) { } }
    keys { key(PK; "Entry No.") { } }
}
table 64003 "DF Blob" { fields { field(1; Code; Integer) { } field(2; Data; Blob) { } } keys { key(PK; Code) { } } }
query 64010 "DF Q Desc"
{
    QueryType = Normal;
    OrderBy = descending(IntV);
    elements { dataitem(U; "DF Universal") { column(IntV; "Integer Field") { } column(Entry; "Entry No.") { } } }
}
query 64011 "DF Q Group Desc"
{
    QueryType = Normal;
    OrderBy = descending(Grp);
    elements { dataitem(U; "DF Universal") { column(Grp; "Integer Field") { } column(Cnt) { Method = Count; } column(Mx; "Entry No.") { Method = Max; } } }
}
query 64012 "DF Q Outer"
{
    QueryType = Normal;
    OrderBy = ascending(PNo);
    elements
    {
        dataitem(P; "DF Parent")
        {
            column(PNo; "Entry No.") { }
            dataitem(C; "DF Child")
            {
                DataItemLink = "Parent Entry No." = P."Entry No.";
                SqlJoinType = LeftOuterJoin;
                column(CNo; "Entry No.") { }
                column(CAmt; Amount) { }
            }
        }
    }
}
query 64013 "DF Q Outer Sum"
{
    QueryType = Normal;
    OrderBy = ascending(PNo);
    elements
    {
        dataitem(P; "DF Parent")
        {
            column(PNo; "Entry No.") { }
            dataitem(C; "DF Child")
            {
                DataItemLink = "Parent Entry No." = P."Entry No.";
                SqlJoinType = LeftOuterJoin;
                column(SumAmt; Amount) { Method = Sum; }
                column(CntC) { Method = Count; }
            }
        }
    }
}
"#;

fn outcomes(tests: &str) -> Vec<(String, Outcome)> {
    let src = format!("{OBJECTS}\ncodeunit 64100 T {{ Subtype = Test; {tests} }}");
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
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    run_codeunit(&engine, &l, oi)
}

fn passes(tests: &str) {
    let o = outcomes(tests);
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
}

const HELPERS: &str = "procedure Check(Expected: Text; Actual: Text) begin if Expected <> Actual then Error('expected <%1> got <%2>', Expected, Actual); end;
procedure InsU(No: Integer; Int: Integer) var U: Record \"DF Universal\"; begin U.Init(); U.\"Entry No.\" := No; U.\"Integer Field\" := Int; U.Insert(); end;
procedure Seed() var P: Record \"DF Parent\"; C: Record \"DF Child\"; begin
    P.\"Entry No.\" := 1; P.Insert(); P.\"Entry No.\" := 2; P.Insert(); P.\"Entry No.\" := 3; P.Insert();
    C.\"Entry No.\" := 10; C.\"Parent Entry No.\" := 1; C.Amount := 5; C.Insert();
    C.\"Entry No.\" := 11; C.Amount := 7; C.Insert();
    C.\"Entry No.\" := 12; C.\"Parent Entry No.\" := 3; C.Amount := 1; C.Insert();
    InsU(1, 2); InsU(2, 1); InsU(3, 2); InsU(4, 1); end;";

// query/TestQueryOuterJoinDescendingSaveAsContracts.al:Query_DescendingOrderBy_TiesStayInAscendingPrimaryKeyOrder
#[test]
fn descending_order_by_keeps_ties_in_ascending_primary_key_order_and_groups_descend() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure Desc() var Q: Query \"DF Q Desc\"; G: Query \"DF Q Group Desc\"; Rows: Text; begin
            Seed();
            Q.Open(); while Q.Read() do Rows += Format(Q.IntV) + '/' + Format(Q.Entry) + ' '; Q.Close();
            Check('2/1 2/3 1/2 1/4 ', Rows);
            Rows := '';
            G.Open(); while G.Read() do Rows += Format(G.Grp) + 'x' + Format(G.Cnt) + 'max' + Format(G.Mx) + ' '; G.Close();
            Check('2x2max3 1x2max4 ', Rows); end;"
    ));
}

// query/TestQueryOuterJoinDescendingSaveAsContracts.al:Query_LeftOuterJoin_RowsFiltersAndAggregates
#[test]
fn left_outer_join_rows_filters_and_aggregates() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure Outer() var Q: Query \"DF Q Outer\"; S: Query \"DF Q Outer Sum\"; Rows: Text; begin
            Seed();
            Q.Open(); while Q.Read() do Rows += Format(Q.PNo) + '/' + Format(Q.CNo) + '/' + Format(Q.CAmt) + ' '; Q.Close();
            Check('1/10/5 1/11/7 2/0/0 3/12/1 ', Rows);
            Rows := ''; Q.SetRange(CNo, 10);
            Q.Open(); while Q.Read() do Rows += Format(Q.PNo) + '/' + Format(Q.CNo) + ' '; Q.Close();
            Check('1/10 ', Rows);
            Rows := ''; Q.SetRange(CNo); Q.SetRange(CAmt, 0);
            Q.Open(); while Q.Read() do Rows += Format(Q.PNo) + '/' + Format(Q.CNo) + ' '; Q.Close();
            Check('2/0 ', Rows);
            Rows := '';
            S.Open(); while S.Read() do Rows += Format(S.PNo) + '/sum' + Format(S.SumAmt) + '/cnt' + Format(S.CntC) + ' '; S.Close();
            Check('1/sum12/cnt2 2/sum0/cnt0 3/sum1/cnt1 ', Rows); end;"
    ));
}

// query/TestQueryOuterJoinDescendingSaveAsContracts.al:Query_SumOfAnIntegerColumn_AndARowInsertedBeforeTheFirstRead
#[test]
fn open_does_not_run_the_query() {
    passes(&format!(
        "{HELPERS}
        [Test]
        procedure Lazy() var G: Query \"DF Q Group Desc\"; Rows: Text; begin
            Seed();
            G.Open();
            InsU(99, 1);
            while G.Read() do Rows += Format(G.Grp) + 'x' + Format(G.Cnt) + ' '; G.Close();
            Check('2x2 1x3 ', Rows); end;"
    ));
}

const STREAM_TEXT: &str =
    "procedure StreamText(var B: Record \"DF Blob\"): Text var InStr: InStream; Line: Text; Result: Text; begin
    B.CalcFields(Data); B.Data.CreateInStream(InStr, TextEncoding::UTF8);
    while not InStr.EOS() do begin InStr.ReadText(Line); Result += Line.Trim(); end; exit(Result); end;";

// query/TestQueryOuterJoinDescendingSaveAsContracts.al:Query_SaveAsXml_And_SaveAsJson_Output
#[test]
fn save_as_xml_writes_a_dataset_and_save_as_json_an_array_of_objects() {
    passes(&format!(
        "{HELPERS} {STREAM_TEXT}
        [Test]
        procedure SaveAs() var Q: Query \"DF Q Desc\"; B: Record \"DF Blob\"; OutStr: OutStream; begin
            Seed();
            B.Code := 1; B.Insert(); B.Data.CreateOutStream(OutStr);
            if not Q.SaveAsXml(OutStr) then Error('SaveAsXml');
            B.Modify(); Check('<?xml version=\"1.0\" standalone=\"yes\"?><DataSet><xs:schema id=\"NewDataSet\" xmlns=\"\" xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" xmlns:msdata=\"urn:schemas-microsoft-com:xml-msdata\"><xs:element name=\"NewDataSet\" msdata:IsDataSet=\"true\" msdata:MainDataTable=\"Result\" msdata:Locale=\"\"><xs:complexType><xs:choice minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:element name=\"Result\" msdata:Locale=\"\"><xs:complexType><xs:sequence><xs:element name=\"IntV\" type=\"xs:int\" minOccurs=\"0\" /><xs:element name=\"Entry\" type=\"xs:int\" minOccurs=\"0\" /></xs:sequence></xs:complexType></xs:element></xs:choice></xs:complexType></xs:element></xs:schema><Result><IntV>2</IntV><Entry>1</Entry></Result><Result><IntV>2</IntV><Entry>3</Entry></Result><Result><IntV>1</IntV><Entry>2</Entry></Result><Result><IntV>1</IntV><Entry>4</Entry></Result></DataSet>', StreamText(B));
            B.Data.CreateOutStream(OutStr);
            if not Q.SaveAsJson(OutStr) then Error('SaveAsJson');
            B.Modify(); Check('[{{\"IntV\":2,\"Entry\":1}},{{\"IntV\":2,\"Entry\":3}},{{\"IntV\":1,\"Entry\":2}},{{\"IntV\":1,\"Entry\":4}}]', StreamText(B)); end;"
    ));
}
