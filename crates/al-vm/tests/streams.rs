//! In-memory streams over Blob fields, end to end. Each test names the corpus test that pins it.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 60008 "ALT Blob"
{
    fields
    {
        field(1; "Code"; Code[20]) { }
        field(2; Description; Text[100]) { }
        field(3; Data; Blob) { }
    }
    keys { key(PK; "Code") { } }
}
"#;

fn run(vars: &str, body: &str) -> Result<Value, Signal> {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
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

fn text(vars: &str, body: &str) -> String {
    match run(vars, body) {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn refused(vars: &str, body: &str) -> bool {
    matches!(run(vars, body), Err(Signal::Unsupported(_)))
}

const BASE: &str = "B: Record \"ALT Blob\"; O: OutStream; I: InStream; T: Text;";

macro_rules! base {
    ($extra:expr) => {
        concat!("B: Record \"ALT Blob\"; O: OutStream; I: InStream; T: Text; ", $extra)
    };
}

#[test]
fn write_text_then_read_text_round_trips_through_the_table() {
    // streams/TestInOutStream.al InOutStream_WriteText_ReadText_Roundtrips.
    let t = text(
        BASE,
        "B.Code := 'T1'; B.Data.CreateOutStream(O); O.WriteText('Hello World'); B.Insert();
         B.Get('T1'); B.CalcFields(Data); B.Data.CreateInStream(I); I.ReadText(T); exit(T);",
    );
    assert_eq!(t, "Hello World");
}

#[test]
fn sequential_write_text_calls_are_concatenated() {
    // streams/TestArrayStreamContracts.al InStream_Sequential_WriteText_ConcatenatesContent.
    let t = text(
        BASE,
        "B.Code := 'S1'; B.Data.CreateOutStream(O); O.WriteText('First'); O.WriteText('Second'); B.Insert();
         B.Get('S1'); B.CalcFields(Data); B.Data.CreateInStream(I); I.ReadText(T); exit(T);",
    );
    assert_eq!(t, "FirstSecond");
}

#[test]
fn an_unloaded_blob_reads_empty_until_calc_fields() {
    // streams/TestBlob.al Blob_Get_WithoutCalcFields_InStreamReadsEmpty; TestInOutStream.al
    // InOutStream_WriteText_BlobHasValue (HasValue is true after Get without CalcFields).
    let t = text(
        BASE,
        "B.Code := 'LAZY'; B.Data.CreateOutStream(O); O.WriteText('NotLoaded'); B.Insert();
         B.Get('LAZY'); B.Data.CreateInStream(I); I.ReadText(T);
         exit('[' + T + ']' + Format(B.Data.HasValue()));",
    );
    assert_eq!(t, "[]Yes");
}

#[test]
fn has_value_and_length() {
    // streams/TestBlob.al Blob_HasValue_Empty_ReturnsFalse, _AfterWrite_ReturnsTrue, Blob_Length_AfterWrite_GreaterThanZero.
    let t = text(
        BASE,
        "B.Code := 'B1'; B.Insert(); B.Get('B1'); exit(Format(B.Data.HasValue()));",
    );
    assert_eq!(t, "No");
    let t = text(
        BASE,
        "B.Code := 'B4'; B.Data.CreateOutStream(O); O.WriteText('LengthTest'); B.Insert(); B.Get('B4'); B.CalcFields(Data);
         exit(Format(B.Data.Length() > 0) + Format(B.Data.HasValue()));",
    );
    assert_eq!(t, "YesYes");
}

#[test]
fn eos_reset_position_and_binary_integers() {
    // streams/TestInOutStream.al InOutStream_EOS_EmptyStream_ReturnsTrue, InOutStream_Write_Integer_ReadInteger;
    // TestArrayStreamContracts.al InStream_EOS_AfterFullRead and InStream_ResetPosition_AllowsReread.
    let t = text(
        BASE,
        "B.Code := 'T4'; B.Data.CreateOutStream(O); O.WriteText(''); B.Insert(); B.Get('T4'); B.CalcFields(Data);
         B.Data.CreateInStream(I); I.ReadText(T); exit(Format(I.EOS()));",
    );
    assert_eq!(t, "Yes");
    let t = text(
        base!("N: Integer;"),
        "B.Code := 'T3'; B.Data.CreateOutStream(O); O.Write(42); B.Insert(); B.Get('T3'); B.CalcFields(Data);
         B.Data.CreateInStream(I); I.Read(N); exit(Format(N) + Format(I.EOS()));",
    );
    assert_eq!(t, "42Yes");
    let t = text(
        base!("T2: Text;"),
        "B.Code := 'R1'; B.Data.CreateOutStream(O); O.WriteText('ReadMe'); B.Insert(); B.Get('R1'); B.CalcFields(Data);
         B.Data.CreateInStream(I); I.ReadText(T); I.ResetPosition(); I.ReadText(T2); exit(T + '|' + T2);",
    );
    assert_eq!(t, "ReadMe|ReadMe");
}

#[test]
fn copy_stream_copies_what_the_source_stream_holds() {
    // streams/TestBlob.al Blob_CopyStream_WithoutCalcFields_DestinationIsEmpty.
    let t = text(
        base!("Src: Record \"ALT Blob\"; D: OutStream; DI: InStream; SI: InStream;"),
        "Src.Code := 'CS1'; Src.Data.CreateOutStream(O); O.WriteText('OriginalContent'); Src.Insert();
         Src.Get('CS1'); B.Code := 'CS2'; B.Data.CreateOutStream(D); Src.Data.CreateInStream(SI); CopyStream(D, SI); B.Insert();
         B.Get('CS2'); B.CalcFields(Data); B.Data.CreateInStream(DI); DI.ReadText(T); exit('[' + T + ']');",
    );
    assert_eq!(t, "[]");
    let t = text(
        base!("Src: Record \"ALT Blob\"; D: OutStream; DI: InStream; SI: InStream;"),
        "Src.Code := 'CS1'; Src.Data.CreateOutStream(O); O.WriteText('OriginalContent'); Src.Insert();
         Src.Get('CS1'); Src.CalcFields(Data); B.Code := 'CS2'; B.Data.CreateOutStream(D); Src.Data.CreateInStream(SI); CopyStream(D, SI); B.Insert();
         B.Get('CS2'); B.CalcFields(Data); B.Data.CreateInStream(DI); DI.ReadText(T); exit('[' + T + ']');",
    );
    assert_eq!(t, "[OriginalContent]");
}

#[test]
fn an_uncommitted_blob_write_stays_local_to_a_database_record() {
    // streams/TestBlobUncommittedWriteIsolation.al: second instance reads empty, re-Get discards,
    // Modify persists.
    let t = text(
        base!("R: Record \"ALT Blob\"; RI: InStream; RT: Text;"),
        "B.Init(); B.Code := 'LEAK'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('LEAKED-BYTES');
         R.Get('LEAK'); R.CalcFields(Data); R.Data.CreateInStream(RI); RI.ReadText(RT);
         exit(Format(R.Data.HasValue()) + '[' + RT + ']');",
    );
    assert_eq!(t, "No[]");
    let t = text(
        BASE,
        "B.Init(); B.Code := 'REGET'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('REGET-BYTES');
         B.Get('REGET'); B.CalcFields(Data); B.Data.CreateInStream(I); I.ReadText(T); exit(Format(B.Data.HasValue()) + '[' + T + ']');",
    );
    assert_eq!(t, "No[]");
    let t = text(
        base!("R: Record \"ALT Blob\"; RI: InStream; RT: Text;"),
        "B.Init(); B.Code := 'COMMIT'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('COMMITTED-BYTES'); B.Modify(true);
         R.Get('COMMIT'); R.CalcFields(Data); R.Data.CreateInStream(RI); RI.ReadText(RT);
         exit(Format(R.Data.HasValue()) + RT + Format(StrLen(RT)));",
    );
    assert_eq!(t, "YesCOMMITTED-BYTES15");
}

#[test]
fn a_temporary_record_holds_the_blob_by_reference() {
    // streams/TestBlobUncommittedWriteIsolation.al TempBlob_UncommittedWrite_SameInstanceReGet_KeepsWrite:
    // the temporary store holds the record's own Blob, so an unpersisted write is visible after Get.
    let t = text(
        "B: Record \"ALT Blob\" temporary; O: OutStream; I: InStream; T: Text;",
        "B.Init(); B.Code := 'TREGET'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('TEMP-BYTES');
         B.Get('TREGET'); B.CalcFields(Data); B.Data.CreateInStream(I); I.ReadText(T); exit(Format(B.Data.HasValue()) + T);",
    );
    assert_eq!(t, "YesTEMP-BYTES");
}

#[test]
fn calc_fields_keeps_an_uncommitted_in_memory_write() {
    // streams/TestBlobCalcFieldsUncommittedWrite.al.
    let t = text(
        BASE,
        "B.Init(); B.Code := 'UNCOMMITTED'; B.Insert(true); Clear(B.Data); B.Data.CreateOutStream(O); O.WriteText('HELLO-BYTES');
         B.CalcFields(Data); B.Data.CreateInStream(I); I.ReadText(T); exit(T);",
    );
    assert_eq!(t, "HELLO-BYTES");
    let t = text(
        base!("R: Record \"ALT Blob\"; RI: InStream; RT: Text;"),
        "B.Init(); B.Code := 'EMPTY'; B.Insert(true); R.Get('EMPTY'); R.CalcFields(Data);
         R.Data.CreateInStream(RI); RI.ReadText(RT); exit(Format(R.Data.HasValue()) + '[' + RT + ']');",
    );
    assert_eq!(t, "No[]");
}

#[test]
fn what_the_corpus_does_not_pin_about_streams_is_refused() {
    // Reading from an unloaded blob's Length, WriteText with no text, binary types other than Integer.
    assert!(refused(
        BASE,
        "B.Code := 'X'; B.Data.CreateOutStream(O); O.WriteText('a'); B.Insert(); B.Get('X'); exit(Format(B.Data.Length()));"
    ));
    assert!(refused(BASE, "B.Data.CreateOutStream(O); O.WriteText(); exit('');"));
}

#[test]
fn array_helpers() {
    // streams/TestArrayStreamContracts.al CompressArray / CopyArray / ArrayLen.
    let t = text(
        "A: array[5] of Text[50]; N: Integer;",
        "A[1] := 'first'; A[3] := 'third'; A[5] := 'fifth'; N := CompressArray(A); exit(Format(N) + A[1] + A[2] + A[3] + '[' + A[4] + A[5] + ']');",
    );
    assert_eq!(t, "3firstthirdfifth[]");
    let t = text(
        "A: array[3] of Text[50]; N: Integer;",
        "A[1] := 'Zebra'; A[2] := 'Apple'; N := CompressArray(A); exit(A[1] + A[2]);",
    );
    assert_eq!(t, "ZebraApple");
    let t = text(
        "S: array[5] of Integer; D: array[3] of Integer; I: Integer;",
        "for I := 1 to 5 do S[I] := I * 10; CopyArray(D, S, 2, 3); exit(Format(D[1]) + Format(D[2]) + Format(D[3]));",
    );
    assert_eq!(t, "203040");
    let t = text("A: array[7] of Integer;", "exit(Format(ArrayLen(A)));");
    assert_eq!(t, "7");
    // streams/TestArrayStreamContracts.al ArrayLen_2D_FirstDimension / _SecondDimension.
    let t = text(
        "A: array[3, 4] of Integer;",
        "exit(Format(ArrayLen(A, 1)) + Format(ArrayLen(A, 2)));",
    );
    assert_eq!(t, "34");
    // Without a dimension the answer for a multi-dimensional array is not pinned.
    assert!(refused("A: array[3, 4] of Integer;", "exit(Format(ArrayLen(A)));"));
    assert!(refused("A: array[3, 4] of Integer;", "exit(Format(ArrayLen(A, 3)));"));
}

#[test]
fn pad_str_pads_on_the_right() {
    // streams/TestInOutStream.al InOutStream_LongText_PreservesContent: PadStr('', 200, 'X').
    let t = text(
        "S: Text;",
        "S := PadStr('', 5, 'X'); exit(S + Format(StrLen(PadStr('ab', 4))));",
    );
    assert_eq!(t, "XXXXX4");
}

#[test]
fn renaming_a_temporary_record_with_a_blob_is_refused() {
    // streams/TestBlobRenameIsolation.al TempBlob_CommittedWrite_Rename_SameInstanceReGet_LosesBlobValue
    // measured that BC loses a Blob persisted with Modify across a temporary Rename, while an
    // unpersisted write survives: no model here reproduces both, so the Rename is refused.
    assert!(refused(
        "B: Record \"ALT Blob\" temporary; O: OutStream;",
        "B.Init(); B.Code := 'A'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('x'); B.Modify(true); B.Rename('B'); exit('');"
    ));
    // A database record keeps what Modify persisted across Rename.
    let t = text(
        base!("R: Record \"ALT Blob\"; RI: InStream; RT: Text;"),
        "B.Init(); B.Code := 'RNC1'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('RENAME-COMMITTED-BYTES'); B.Modify(true); B.Rename('RNC2');
         R.Get('RNC2'); R.CalcFields(Data); R.Data.CreateInStream(RI); RI.ReadText(RT); exit(RT);",
    );
    assert_eq!(t, "RENAME-COMMITTED-BYTES");
    // Rename persists the whole buffer, so an unpersisted Blob write reaches the renamed row.
    let t = text(
        base!("R: Record \"ALT Blob\"; RI: InStream; RT: Text;"),
        "B.Init(); B.Code := 'RNL1'; B.Insert(true); B.Data.CreateOutStream(O); O.WriteText('RENAME-LEAKED-BYTES'); B.Rename('RNL2');
         R.Get('RNL2'); R.CalcFields(Data); R.Data.CreateInStream(RI); RI.ReadText(RT); exit(RT);",
    );
    assert_eq!(t, "RENAME-LEAKED-BYTES");
}

#[test]
fn text_encodings_decide_the_bytes_per_character() {
    // text/TestTextEncodingWindows.al: TextEncoding::Windows writes one byte per representable character
    // (cp1252: em dash, euro, e acute, no-break space) and round-trips them; UTF8 writes more bytes for the
    // same text and round-trips; ASCII costs one byte either way.
    let sample = "A' + Format(E1) + Format(E2) + Format(E3) + Format(E4) + 'Z";
    let vars = base!("E1: Char; E2: Char; E3: Char; E4: Char; WI: InStream; Len: Integer; Back: Text;");
    let prologue = "E1 := 8212; E2 := 8364; E3 := 233; E4 := 160;";
    let write = |enc: &str| {
        format!(
            "{prologue} B.Init(); B.Code := 'ROW'; B.Insert(); B.Data.CreateOutStream(O, TextEncoding::{enc}); O.WriteText('{sample}'); B.Modify();
             B.Get('ROW'); B.CalcFields(Data); Len := B.Data.Length(); B.Data.CreateInStream(WI, TextEncoding::{enc}); WI.ReadText(Back);
             exit(Format(Len) + '|' + Format(Back = '{sample}'));"
        )
    };
    assert_eq!(text(vars, &write("Windows")), "6|Yes");
    assert_eq!(text(vars, &write("UTF8")), "12|Yes");
    // Encodings other than these two are not pinned.
    assert!(refused(vars, &write("UTF16")));
}
