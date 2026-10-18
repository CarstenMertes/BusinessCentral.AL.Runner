// The test app: stands in for a Microsoft test app.
codeunit 50510 "Imp Tests"
{
    Subtype = Test;

    [Test]
    procedure PostKeepsALargeAmount()
    var
        Posting: Codeunit "Imp Posting";
        Amount: Decimal;
    begin
        Amount := Posting.Post(10);
        if Amount <> 10 then
            Error('amount %1', Amount);
    end;

    [Test]
    procedure PostKeepsASmallAmount()
    var
        Posting: Codeunit "Imp Posting";
        Amount: Decimal;
    begin
        Amount := Posting.Post(1);
        if Amount <> 1 then
            Error('amount %1', Amount);
        Amount := Posting.Post(2);
        if Amount <> 2 then
            Error('amount %1', Amount);
    end;

    [Test]
    procedure PostsNothing()
    begin
    end;

    [Test]
    procedure FailsAlready()
    begin
        Error('fails without the extension');
    end;
}

codeunit 50511 "Imp Other Tests"
{
    Subtype = Test;

    [Test]
    procedure AlsoPostsNothing()
    begin
    end;
}
