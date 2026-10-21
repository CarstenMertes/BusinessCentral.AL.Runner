// An example extension for a manual impact run against Microsoft's resource tests (not run by `cargo test`: it needs
// the Base App). It doubles the quantity of every resource ledger entry that a resource journal line posts.
//   al-runner test "<artifact>/Applications/BaseApp/Test/Tests-Resource.Source.zip" --codeunit 136403 \
//     --impact-of crates/alrunner-cli/tests/fixtures/impact/baseapp-ext --impact all
codeunit 50540 "Impact Example Res. Qty"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Res. Jnl.-Post Line", 'OnBeforeResLedgEntryInsert', '', false, false)]
    local procedure DoubleQuantity(var ResLedgerEntry: Record "Res. Ledger Entry"; ResJournalLine: Record "Res. Journal Line")
    begin
        ResLedgerEntry.Quantity := ResLedgerEntry.Quantity * 2;
    end;
}
