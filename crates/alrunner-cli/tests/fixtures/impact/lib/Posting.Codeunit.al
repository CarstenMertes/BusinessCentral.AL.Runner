// The dependency app: a publisher the extension subscribes to (stands in for a Base App publisher).
codeunit 50500 "Imp Posting"
{
    procedure Post(Amount: Decimal): Decimal
    begin
        OnBeforePost(Amount);
        exit(Amount);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforePost(var Amount: Decimal)
    begin
    end;
}
