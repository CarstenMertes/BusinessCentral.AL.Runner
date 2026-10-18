// An app that touches nothing the tests reach.
codeunit 50530 "Imp Unrelated"
{
    procedure Answer(): Integer
    begin
        exit(42);
    end;
}
