// A test app whose tests the runtime refuses: the impact index records nothing for them, so only static reach can
// say whether they reach an impact point.
codeunit 50520 "Imp Refused Tests"
{
    Subtype = Test;

    [Test]
    procedure PostsAfterARefusal()
    var
        Posting: Codeunit "Imp Posting";
    begin
        Refuse();
        Posting.Post(10);
    end;

    [Test]
    procedure RefusesOnly()
    begin
        Refuse();
    end;

    local procedure Refuse()
    begin
        Codeunit.Run(Codeunit::"Not There");
    end;
}
