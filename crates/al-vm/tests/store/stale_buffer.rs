//! A Modify of a buffer that still holds what its own last Modify wrote, after a rollback undid that write
//! (corpus record/lt-record-conversion-assumptions/StaleBuffer.al).

use al_builtins::native::Signal;
use al_testkit::text;

const SRC: &str = r#"
table 69997 "SB Row"
{
    fields { field(1; Id; Code[10]) { } field(2; Counter; Integer) { } field(3; Note; Text[20]) { } }
    keys { key(PK; Id) { } }
    trigger OnModify() begin end;
}
codeunit 69998 C
{
    procedure NotReassigned(): Text
    var R: Record "SB Row"; Check: Record "SB Row"; s: Text;
    begin
        R.Id := 'S2'; R.Counter := 10; R.Note := 'orig'; R.Insert();
        Commit();
        R.Counter := 11;
        R.Modify();
        asserterror Error('boom');
        s := Format(R.Counter) + '|' + Format(R.Modify());
        Check.Get('S2');
        exit(s + '|' + Format(Check.Counter) + '|' + Check.Note);
    end;

    procedure ChangedAfter(): Text
    var R: Record "SB Row"; Check: Record "SB Row";
    begin
        R.Id := 'S5'; R.Counter := 1; R.Insert();
        Commit();
        R.Counter := 3;
        R.Modify();
        asserterror Error('boom');
        R.Note := 'late';
        R.Modify();
        Check.Get('S5');
        exit(Format(Check.Counter) + '|' + Check.Note);
    end;

    procedure WithTrigger()
    var R: Record "SB Row";
    begin
        R.Id := 'S6'; R.Counter := 1; R.Insert();
        Commit();
        R.Counter := 2;
        R.Modify();
        asserterror Error('boom');
        R.Modify(true);
    end;
}
"#;

// StaleBuffer.al:76-86: the buffer keeps 11, the Modify answers true and the row keeps the value the rollback restored.
#[test]
fn modify_of_the_buffer_its_own_rolled_back_write_left_writes_nothing() {
    assert_eq!(text(al_testkit::run_in(SRC, "C", "NotReassigned")), "11|Yes|10|orig");
    // StaleBuffer.al:167-168: a buffer changed after the rollback writes the whole row.
    assert_eq!(text(al_testkit::run_in(SRC, "C", "ChangedAfter")), "3|late");
}

// Not pinned: the same Modify of a table with an OnModify trigger, with the triggers.
#[test]
fn the_same_modify_with_triggers_is_refused() {
    let r = al_testkit::run_in(SRC, "C", "WithTrigger");
    assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?}");
}

const MEDIA: &str = r#"
table 2000000184 "Tenant Media"
{
    fields
    {
        field(1; ID; Guid) { } field(2; Description; Text[250]) { } field(3; Content; Blob) { }
        field(4; "Mime Type"; Text[100]) { } field(5; Height; Integer) { } field(6; Width; Integer) { }
    }
    keys { key(PK; ID) { } }
}
table 69999 "SB Media"
{
    fields { field(1; Id; Integer) { } field(2; Pic; Media) { } field(3; Src; Blob) { } }
    keys { key(PK; Id) { } }
}
codeunit 69989 M
{
    procedure Replace(): Text
    var Row: Record "SB Media"; TenantMedia: Record "Tenant Media"; ins: InStream; outs: OutStream; First: Guid; Second: Guid; s: Text;
    begin
        Row.Id := 1; Row.Insert();
        Row.Src.CreateOutStream(outs); outs.WriteText('one'); Row.Src.CreateInStream(ins);
        Row.Pic.ImportStream(ins, 'one');
        Row.Modify();
        First := Row.Pic.MediaId();
        Row.Src.CreateOutStream(outs); outs.WriteText('two'); Row.Src.CreateInStream(ins);
        Row.Pic.ImportStream(ins, 'two');
        Second := Row.Pic.MediaId();
        s := Format(TenantMedia.Get(First));
        Row.Modify();
        exit(s + Format(TenantMedia.Get(First)) + Format(TenantMedia.Get(Second)));
    end;
}
"#;

// record/lt-ar-records/ArRecordProbes.al:141-143: the Modify that stores the field's new media removes the Tenant Media row of
// the old one.
#[test]
fn modify_that_replaces_a_media_removes_the_old_media() {
    assert_eq!(text(al_testkit::run_in(MEDIA, "M", "Replace")), "YesNoYes");
}
