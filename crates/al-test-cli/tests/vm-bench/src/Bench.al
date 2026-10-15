// The benchmark app of the VM speed budget (ADR-031, crates/al-test-cli/tests/perf_vm.rs). Our own code, no Microsoft
// source. One test codeunit per benchmark, because a test codeunit is the unit that runs on a session of its own.
// The iteration counts below are the ones perf_vm.rs divides by; change both together.

table 70000 "Bench Row"
{
    TableType = Temporary;

    fields
    {
        field(1; "No."; Integer) { }
        field(2; Amount; Decimal) { }
        field(3; Description; Text[100]) { }
        field(4; Qty; Integer) { }
    }

    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

// What a run costs besides its loop: the session and the engine. perf_vm.rs subtracts it.
codeunit 70001 "Bench Empty"
{
    Subtype = Test;

    [Test]
    procedure Empty()
    begin
    end;
}

// 300000 iterations
codeunit 70002 "Bench Int Loop"
{
    Subtype = Test;

    [Test]
    procedure IntLoop()
    var
        i: Integer;
        s: Integer;
    begin
        for i := 1 to 300000 do
            s := (s + i) mod 1000003;
    end;
}

// 100000 iterations
codeunit 70003 "Bench Decimal Loop"
{
    Subtype = Test;

    [Test]
    procedure DecimalLoop()
    var
        i: Integer;
        d: Decimal;
    begin
        for i := 1 to 100000 do
            d := d + i * 1.5 - 0.25;
    end;
}

// 100000 iterations
codeunit 70004 "Bench Call Loop"
{
    Subtype = Test;

    [Test]
    procedure CallLoop()
    var
        i: Integer;
        s: Integer;
    begin
        for i := 1 to 100000 do
            s := Add(s, i);
    end;

    local procedure Add(a: Integer; b: Integer): Integer
    begin
        exit((a + b) mod 1000003);
    end;
}

// 30000 iterations
codeunit 70005 "Bench Text Loop"
{
    Subtype = Test;

    [Test]
    procedure TextLoop()
    var
        i: Integer;
        t: Text;
        n: Integer;
    begin
        for i := 1 to 30000 do begin
            t := 'Item ' + Format(i mod 97);
            n += StrLen(CopyStr(t, 2, 3));
            if StrPos(t, '5') > 0 then
                n += 1;
        end;
    end;
}

// 50000 iterations
codeunit 70006 "Bench Builtin Loop"
{
    Subtype = Test;

    [Test]
    procedure BuiltinLoop()
    var
        i: Integer;
        d: Decimal;
    begin
        for i := 1 to 50000 do
            d += Round(Abs(i / 3), 0.01) + Power(2, 3);
    end;
}

// 5000 rows: 5000 Init/Insert, one SetRange + FindSet/Next pass, 5000 Get/Modify (15000 record operations)
codeunit 70007 "Bench Record Loop"
{
    Subtype = Test;

    [Test]
    procedure RecordLoop()
    var
        R: Record "Bench Row";
        i: Integer;
        sum: Integer;
    begin
        for i := 1 to 5000 do begin
            R.Init();
            R."No." := i;
            R.Amount := i;
            R.Description := 'row';
            R.Qty := i mod 5;
            R.Insert();
        end;
        R.SetRange(Qty, 3);
        if R.FindSet() then
            repeat
                sum += R.Qty;
            until R.Next() = 0;
        R.Reset();
        for i := 1 to 5000 do begin
            R.Get(i);
            R.Amount := R.Amount + 1;
            R.Modify();
        end;
    end;
}
