//! `Format(Variant, 0, N)` with a standard format number 3 to 8 raises an error that names the type 'Variant', whatever the
//! variant holds (corpus types/lt-scalars/ScT70052.al:Integer_Numbers, n3 to n8; 0 to 2 and 9 format the value).

use al_testkit::text;

const SRC: &str = r#"
codeunit 69720 C
{
    [TryFunction]
    local procedure TryF(V: Variant; N: Integer; var R: Text)
    begin
        R := Format(V, 0, N);
    end;

    procedure F(): Text
    var T: Text; R: Text; N: Integer;
    begin
        for N := 2 to 4 do begin
            if TryF(12, N, R) then T += R + ';' else T += GetLastErrorText() + ';';
        end;
        if TryF(12, 9, R) then T += R + ';';
        exit(T);
    end;
}
"#;

#[test]
fn a_standard_format_number_a_variant_lacks_names_the_variant() {
    assert_eq!(
        text(al_testkit::run_in(SRC, "C", "F")),
        "12;Standard format number 3 does not exist for the type 'Variant'.;Standard format number 4 does not exist for the type 'Variant'.;12;"
    );
}
