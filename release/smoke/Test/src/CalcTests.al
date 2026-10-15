codeunit 50100 "Smoke Calc Tests"
{
    Subtype = Test;

    [Test]
    procedure AddAddsTwoNumbers()
    var
        Calc: Codeunit "Smoke Calc";
    begin
        if Calc.Add(2, 3) <> 5 then
            Error('Add(2, 3) returned the wrong sum');
    end;

    [Test]
    procedure TextAndDecimalWork()
    var
        Amount: Decimal;
    begin
        Amount := 10 / 4;
        if Format(Amount) <> '2.5' then
            Error('Format(2.5) returned %1', Format(Amount));
    end;
}
