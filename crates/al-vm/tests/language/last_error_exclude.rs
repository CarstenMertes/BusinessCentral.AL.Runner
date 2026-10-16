//! `GetLastErrorText(ExcludeCustomerContent)`: Microsoft's Marketing and Misc tests call it through `Library - Assert` helpers
//! (about 320 tests). A text that carries no customer content is the same text; a record's identification values are
//! customer content, so such a text is refused. Assumption get-last-error-text-exclude-customer-content.

use al_builtins::native::Signal;
use al_testkit::{run_in, text};

const SRC: &str = r#"
table 69840 "LE Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 69841 "LE C"
{
    procedure Plain(): Text
    begin
        asserterror Error('plain error');
        exit(GetLastErrorText(true));
    end;

    procedure WithValues(): Text
    var R: Record "LE Row";
    begin
        asserterror R.Get(5);
        exit(GetLastErrorText(true));
    end;
}
"#;

#[test]
fn the_flag_leaves_a_text_without_customer_content_alone() {
    assert_eq!(text(run_in(SRC, "LE C", "Plain")), "plain error");
}

#[test]
fn a_text_with_identification_values_is_refused() {
    match run_in(SRC, "LE C", "WithValues") {
        Err(Signal::Unsupported(m)) => assert!(m.contains("customer content"), "{m}"),
        other => panic!("{other:?}"),
    }
}
