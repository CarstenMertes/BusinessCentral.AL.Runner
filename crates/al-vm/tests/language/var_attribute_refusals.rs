//! Variable attributes the runtime does not implement are refused where the variable is used, never ignored
//! (hard rule 4): `[RunOnClient]` and `[WithEvents]` on DotNet variables (al-language-onprem-dotnet/lt-ac/AcDnAttributes.al:50,
//! :86, :145-146) and the read of `SecurityFiltering()` on a record declared with `[SecurityFiltering(...)]`
//! (al-language/lt-ac/ACAttributes.al:176-191).

use al_builtins::native::Signal;
use al_builtins::value::Value;

fn refused(r: Result<Value, Signal>, needle: &str) {
    match r {
        Err(Signal::Unsupported(why)) => assert!(why.contains(needle), "{why}"),
        other => panic!("expected a refusal naming {needle}, got {other:?}"),
    }
}

const DOTNET: &str = "dotnet { assembly(\"mscorlib\") { type(\"System.Text.StringBuilder\"; B) { } type(\"System.EventArgs\"; A) { } } assembly(\"System\") { type(\"System.Timers.Timer\"; T) { } } }";

#[test]
fn a_global_run_on_client_variable_is_refused_where_it_is_used() {
    let src = format!(
        "{DOTNET} codeunit 50100 C {{ var [RunOnClient] Cb: DotNet B; procedure F(): Integer begin Cb := Cb.StringBuilder(); exit(1); end; procedure G(): Integer begin exit(2); end; procedure K(Use: Boolean): Integer begin if Use then Cb := Cb.StringBuilder(); exit(3); end; }}"
    );
    refused(al_testkit::run_in(&src, "C", "F"), "RunOnClient");
    assert!(matches!(al_testkit::run_in(&src, "C", "G"), Ok(Value::Integer(2))));
    // The statement that uses the variable refuses when it runs; the rest of its routine runs (Base Application declares
    // such variables in routines whose other paths tests reach).
    let r = al_testkit::run_with(&src, "C", "K", vec![Value::Boolean(false)]).map(|(v, _)| v);
    assert!(matches!(r, Ok(Value::Integer(3))), "{r:?}");
    let r = al_testkit::run_with(&src, "C", "K", vec![Value::Boolean(true)]).map(|(v, _)| v);
    refused(r, "RunOnClient");
}

#[test]
fn a_local_run_on_client_variable_is_refused() {
    let src = format!(
        "{DOTNET} codeunit 50100 C {{ procedure F(): Integer var [RunOnClient] Cb: DotNet B; begin Cb := Cb.StringBuilder(); exit(1); end; }}"
    );
    refused(al_testkit::run_in(&src, "C", "F"), "RunOnClient");
}

#[test]
fn a_with_events_variable_is_refused_where_it_is_used() {
    let src = format!(
        "{DOTNET} codeunit 50100 C {{ var [WithEvents] Tm: DotNet T; N: Integer; trigger Tm::Disposed(sender: Variant; e: DotNet A) begin N += 1; end; procedure F(): Integer begin Tm := Tm.Timer(); Tm.Dispose(); exit(N); end; }}"
    );
    refused(al_testkit::run_in(&src, "C", "F"), "WithEvents");
}

#[test]
fn security_filtering_read_of_an_attributed_record_is_refused() {
    let src = "table 50100 T { fields { field(1; Id; Integer) { } } } \
        codeunit 50100 C { procedure F(): Text var [SecurityFiltering(SecurityFilter::Filtered)] R: Record T; begin exit(Format(R.SecurityFiltering())); end; \
        procedure G(): Text var P: Record T; begin exit(Format(P.SecurityFiltering())); end; \
        procedure H(): Integer var [SecurityFiltering(SecurityFilter::Ignored)] R: Record T; begin exit(R.Count()); end; }";
    refused(al_testkit::run_in(src, "C", "F"), "SecurityFiltering");
    // A record without the attribute, and other uses of an attributed one, still run.
    assert_eq!(al_testkit::text(al_testkit::run_in(src, "C", "G")), "Validated");
    assert!(matches!(al_testkit::run_in(src, "C", "H"), Ok(Value::Integer(0))));
}
