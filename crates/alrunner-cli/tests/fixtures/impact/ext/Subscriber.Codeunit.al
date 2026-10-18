// The extension: a subscriber that changes what the publisher's caller gets back for large amounts.
codeunit 50520 "Imp Ext Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Imp Posting", 'OnBeforePost', '', false, false)]
    local procedure DoubleLargeAmounts(var Amount: Decimal)
    begin
        if Amount > 5 then
            Amount := Amount * 2;
    end;
}

// The extension's own tests are not part of an impact run.
codeunit 50521 "Imp Ext Own Tests"
{
    Subtype = Test;

    [Test]
    procedure OwnTest()
    begin
        Error('the extension''s own test ran');
    end;
}
