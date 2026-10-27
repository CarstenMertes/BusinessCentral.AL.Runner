//! `TestField.Lookup` on a control whose own OnLookup writes Rec directly and answers false: the row counts as edited, so
//! the close that follows saves it. Microsoft's Tests-Monitor Sensitive Fields needs this on real BC (codeunit 139066
//! "Monitor Field Pages Test", AddFieldUsingFieldLookup and AddFieldUsingTableAndFieldLookup: page 1369 "Monitored Fields
//! Worksheet", control "Field No", `Rec.Validate("Field No.", ...)` in OnLookup, then `Close()`, then the row is read back).
//! Not pinned by the corpus yet: the assumption `control-onlookup-edit-saved-at-close` (docs/probes-needed.md).

use al_testkit::{blocked, compile};
use al_testrun::Outcome;
use al_testrun::assumptions::Assumption;
use al_testrun::isolation::TestIsolation;

const OBJECTS: &str = r#"
table 64700 "COL Line"
{
    fields
    {
        field(1; Code; Code[20]) { }
        field(2; Inserted; Boolean) { }
        field(3; Descr; Text[30]) { }
        field(4; Modified; Integer) { }
        field(5; Data; Blob) { }
    }
    keys { key(PK; Code) { } }
}
page 64710 "COL Worksheet"
{
    PageType = Worksheet; SourceTable = "COL Line";
    layout
    {
        area(Content)
        {
            repeater(R)
            {
                field(Picks; Rec.Code)
                {
                    trigger OnLookup(var Text: Text): Boolean
                    begin
                        Rec.Validate(Code, 'PICKED');
                    end;
                }
                field(Looks; Rec.Code)
                {
                    trigger OnLookup(var Text: Text): Boolean
                    begin
                    end;
                }
                field(SetsDescr; Rec.Descr)
                {
                    trigger OnLookup(var Text: Text): Boolean
                    begin
                        Rec.Descr := 'looked up';
                    end;
                }
                field(Reverts; Rec.Descr)
                {
                    trigger OnLookup(var Text: Text): Boolean
                    var
                        Was: Text[30];
                    begin
                        Was := Rec.Descr;
                        Rec.Descr := 'changed';
                        Rec.Descr := Was;
                    end;
                }
                field(Moves; Rec.Code)
                {
                    trigger OnLookup(var Text: Text): Boolean
                    begin
                        Rec.Get('OTHER');
                        Rec.Descr := 'moved';
                    end;
                }
            }
        }
    }
    trigger OnInsertRecord(BelowxRec: Boolean): Boolean
    begin
        Rec.Inserted := true;
    end;
    trigger OnModifyRecord(): Boolean
    begin
        Rec.Modified += 1;
    end;
}
"#;

/// The outcomes, and the assumptions the tests ran through.
fn outcomes(tests: &str) -> (Vec<(String, Outcome)>, Vec<Assumption>) {
    let src = format!("{OBJECTS}\ncodeunit 64720 T {{ Subtype = Test; {tests} }}");
    let l = compile(&src);
    let engine = al_vm::Engine::new(l.program.clone(), blocked(&l));
    let oi = l.program.objects.iter().position(|o| o.name == "T").unwrap();
    let mut hits = Vec::new();
    let out = al_testrun::runtime::run_codeunit_full(
        &engine,
        &l,
        oi,
        None,
        TestIsolation::Codeunit,
        None,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut hits,
        None,
    );
    (out, hits.into_iter().flat_map(|(_, a)| a).collect())
}

fn passes(tests: &str) -> Vec<Assumption> {
    let (o, hits) = outcomes(tests);
    assert!(!o.is_empty());
    for (n, r) in &o {
        assert_eq!(r, &Outcome::Pass, "{n}");
    }
    hits
}

const CHECK: &str = "local procedure Check(Want: Text; Got: Text) begin if Want <> Got then Error('want [%1] got [%2]', Want, Got); end;
local procedure SeedTwo() var L: Record \"COL Line\"; begin
    L.DeleteAll(); L.Code := 'A'; L.Descr := 'stored'; L.Insert(); L.Code := 'OTHER'; L.Descr := 'stored'; L.Insert();
end;";

// Tests-Monitor Sensitive Fields 139066 AddFieldUsingFieldLookup: a new row that only the control's OnLookup changed is
// inserted at Close, through OnInsertRecord.
#[test]
fn a_new_row_changed_only_by_the_controls_on_lookup_is_inserted_at_close() {
    let hits = passes(&format!(
        "{CHECK}
        [Test]
        procedure LookupThenClose() var L: Record \"COL Line\"; W: TestPage \"COL Worksheet\"; begin
            L.DeleteAll(); W.OpenEdit(); W.New();
            W.Picks.Lookup(); Check('PICKED', W.Picks.Value());
            W.Close();
            if not L.Get('PICKED') then Error('the row the OnLookup wrote was not inserted at Close');
            if not L.Inserted then Error('OnInsertRecord did not run');
        end;"
    ));
    assert!(hits.contains(&Assumption::ControlOnLookupEditSavedAtClose), "{hits:?}");
}

// An OnLookup that writes nothing leaves the new row as it was: nothing is inserted, and no assumption is used. The table
// has a Blob field, which compares unequal to its own copy; it must not count as a change.
#[test]
fn an_on_lookup_that_changes_nothing_inserts_nothing() {
    let hits = passes(&format!(
        "{CHECK}
        [Test]
        procedure LookupNothingThenClose() var L: Record \"COL Line\"; W: TestPage \"COL Worksheet\"; begin
            L.DeleteAll(); W.OpenEdit(); W.New();
            W.Looks.Lookup();
            W.Close();
            if not L.IsEmpty() then Error('a row was inserted');
        end;"
    ));
    assert!(!hits.contains(&Assumption::ControlOnLookupEditSavedAtClose), "{hits:?}");
}

// The same on a stored row: the field the OnLookup wrote is saved at Close, through OnModifyRecord.
#[test]
fn a_stored_row_changed_only_by_the_controls_on_lookup_is_modified_at_close() {
    let hits = passes(&format!(
        "{CHECK}
        [Test]
        procedure LookupStoredThenClose() var L: Record \"COL Line\"; W: TestPage \"COL Worksheet\"; begin
            SeedTwo(); W.OpenEdit(); W.GoToKey('A');
            W.SetsDescr.Lookup(); Check('looked up', W.SetsDescr.Value());
            W.Close();
            L.Get('A'); Check('looked up', L.Descr); Check('1', Format(L.Modified));
        end;"
    ));
    assert!(hits.contains(&Assumption::ControlOnLookupEditSavedAtClose), "{hits:?}");
}

// An OnLookup that changes a field and sets it back leaves the stored row unchanged: no OnModifyRecord, no assumption.
#[test]
fn an_on_lookup_that_reverts_its_change_saves_nothing() {
    let hits = passes(&format!(
        "{CHECK}
        [Test]
        procedure RevertThenClose() var L: Record \"COL Line\"; W: TestPage \"COL Worksheet\"; begin
            SeedTwo(); W.OpenEdit(); W.GoToKey('A');
            W.Reverts.Lookup();
            W.Close();
            L.Get('A'); Check('stored', L.Descr); Check('0', Format(L.Modified));
        end;"
    ));
    assert!(!hits.contains(&Assumption::ControlOnLookupEditSavedAtClose), "{hits:?}");
}

// An OnLookup that puts another stored row into Rec is not marked: the other row is not written at Close, as before this
// change (an action that does the same saves nothing, misc/lt-probes-needed-7/PnGRefresh.al:387).
#[test]
fn an_on_lookup_that_moves_rec_to_another_row_saves_nothing() {
    let hits = passes(&format!(
        "{CHECK}
        [Test]
        procedure MoveThenClose() var L: Record \"COL Line\"; W: TestPage \"COL Worksheet\"; begin
            SeedTwo(); W.OpenEdit(); W.GoToKey('A');
            W.Moves.Lookup();
            W.Close();
            L.Get('OTHER'); Check('stored', L.Descr); Check('0', Format(L.Modified));
            L.Get('A'); Check('stored', L.Descr); Check('0', Format(L.Modified));
        end;"
    ));
    assert!(!hits.contains(&Assumption::ControlOnLookupEditSavedAtClose), "{hits:?}");
}
