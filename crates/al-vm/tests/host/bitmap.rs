//! ADR-044 option 3: the eight `System.Drawing` members Microsoft's `O365 Brand Color`.MakePicture uses
//! (O365BrandColor.Table.al:113-137) are served by the runtime. Nothing is drawn. The picture the stream and the
//! Media field hold has a value, and any look at its content is refused. The only behavior a test observes is
//! `HasValue` (Microsoft's O365BrandColorTests.Codeunit.al:74 and :158); it is a registered assumption.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_testkit::text;

const SRC: &str = r#"
dotnet { assembly("System.Drawing") {
    type("System.Drawing.Bitmap"; Bitmap) { }
    type("System.Drawing.Graphics"; Graphics) { }
    type("System.Drawing.Color"; Color) { }
    type("System.Drawing.ColorTranslator"; ColorTranslator) { }
    type("System.Drawing.SolidBrush"; SolidBrush) { }
    type("System.Drawing.Imaging.ImageFormat"; ImageFormat) { }
} }
table 50160 "BM Brand" { fields { field(1; Id; Integer) { } field(15; Pic; Media) { } field(16; Raw; Blob) { } } keys { key(PK; Id) { } } }
codeunit 50160 C
{
    // The body of O365 Brand Color.MakePicture, with the Temp Blob replaced by a Blob field.
    procedure MakePicture(var R: Record "BM Brand"; Hex: Text)
    var
        Bitmap: DotNet Bitmap;
        Graphics: DotNet Graphics;
        Color: DotNet Color;
        ColorTranslator: DotNet ColorTranslator;
        SolidColorBrush: DotNet SolidBrush;
        ImageFormat: DotNet ImageFormat;
        InStr: InStream;
    begin
        Bitmap := Bitmap.Bitmap(100, 100);
        Graphics := Graphics.FromImage(Bitmap);
        Color := ColorTranslator.FromHtml(Hex);
        SolidColorBrush := SolidColorBrush.SolidBrush(Color);
        Graphics.FillEllipse(SolidColorBrush, 0, 0, 100, 100);
        Graphics.Dispose();
        R.Raw.CreateInStream(InStr);
        Bitmap.Save(InStr, ImageFormat.Png);
        R.Pic.ImportStream(InStr, '');
        Bitmap.Dispose();
    end;
    BODY
}
"#;

fn run(body: &str) -> Result<Value, Signal> {
    let src = SRC.replace("BODY", body);
    al_testkit::run_in(&src, "C", "F")
}

#[track_caller]
fn refused(body: &str, needle: &str) {
    match run(body) {
        Err(Signal::Unsupported(why)) => assert!(why.contains(needle), "{body}: {why}"),
        other => panic!("{body}: expected a refusal containing {needle:?}, got {other:?}"),
    }
}

const PENDING: &str = "picture drawn by System.Drawing.Bitmap is not produced";

/// O365BrandColorTests.Codeunit.al:74, :158: `Sample Picture.HasValue` after MakePicture.
#[test]
fn bitmap_make_picture_runs_and_the_media_has_a_value() {
    let r = text(run(
        "procedure F(): Text var R: Record \"BM Brand\"; begin MakePicture(R, '#B51725'); exit(Format(R.Pic.HasValue())); end;",
    ));
    assert_eq!(r, "Yes");
    assert!(al_builtins::assumptions::hit(
        al_builtins::assumptions::Assumption::BitmapPictureNotDrawn
    ));
}

#[test]
fn bitmap_the_media_survives_insert_and_get() {
    let r = text(run(
        "procedure F(): Text var R: Record \"BM Brand\"; G: Record \"BM Brand\"; begin R.Id := 1; MakePicture(R, '#008DD3'); R.Insert(); G.Get(1); exit(Format(G.Pic.HasValue())); end;",
    ));
    assert_eq!(r, "Yes");
}

#[test]
fn bitmap_a_read_of_the_stream_after_save_is_refused() {
    refused(
        "procedure F(): Integer var R: Record \"BM Brand\"; I: InStream; begin MakePicture(R, '#B51725'); R.Raw.CreateInStream(I); exit(I.Length()); end;",
        PENDING,
    );
    refused(
        "procedure F(): Integer var R: Record \"BM Brand\"; begin MakePicture(R, '#B51725'); exit(R.Raw.Length()); end;",
        PENDING,
    );
}

#[test]
fn bitmap_an_export_of_the_media_is_refused() {
    refused(
        "procedure F(): Boolean var R: Record \"BM Brand\"; O: OutStream; begin MakePicture(R, '#B51725'); R.Raw.CreateOutStream(O); exit(R.Pic.ExportStream(O)); end;",
        PENDING,
    );
}

#[test]
fn bitmap_only_the_members_make_picture_uses_are_served() {
    // A member that is not one of the eight stays refused, with the registry's reason.
    for body in [
        "procedure F(): Integer var B: DotNet Bitmap; begin B := B.Bitmap(10, 10); exit(B.Width); end;",
        "procedure F() var B: DotNet Bitmap; G: DotNet Graphics; begin B := B.Bitmap(10, 10); G := G.FromImage(B); G.Clear(G); end;",
        "procedure F() var B: DotNet Bitmap; I: DotNet ImageFormat; begin B := B.Bitmap(10, 10); I := I.Jpeg; end;",
        "procedure F() var C: DotNet Color; T: DotNet ColorTranslator; begin C := T.FromHtml('#B51725'); C := C.Red; end;",
    ] {
        refused(body, "does not implement");
    }
}

#[test]
fn bitmap_argument_kinds_other_than_the_ones_used_are_refused() {
    refused(
        "procedure F() var B: DotNet Bitmap; begin B := B.Bitmap('x'); end;",
        "Bitmap",
    );
    refused(
        "procedure F() var B: DotNet Bitmap; begin B := B.Bitmap(0, 10); end;",
        "Bitmap",
    );
    refused(
        "procedure F() var T: DotNet ColorTranslator; C: DotNet Color; begin C := T.FromHtml('red'); end;",
        "FromHtml",
    );
    // Save to an OutStream is not what MakePicture does.
    refused(
        "procedure F() var R: Record \"BM Brand\"; B: DotNet Bitmap; I: DotNet ImageFormat; O: OutStream; begin B := B.Bitmap(10, 10); R.Raw.CreateOutStream(O); B.Save(O, I.Png); end;",
        "Save",
    );
}
