//! A test codeunit with `SingleInstance = true` is refused. Whether BC runs its test methods on the session's single
//! instance (so a variable of its own type, or its own event subscribers, see the globals the test method writes) is not
//! pinned by the corpus (docs/probes-needed.md). Microsoft's Tests-ERM "Cost Adj. Sch. Notif. Tests" (139844) binds such
//! a variable of itself and reads a global its subscriber sets; running it on a second instance failed four tests.

use al_testrun::Outcome;

#[test]
fn a_single_instance_test_codeunit_is_refused() {
    let src = r#"
codeunit 62600 T
{
    Subtype = Test;
    SingleInstance = true;
    var Seen: Boolean;
    [Test]
    procedure ReadsItsOwnGlobal()
    var Me: Codeunit T;
    begin
        Me.SetSeen();
        if not Seen then Error('a second instance');
    end;
    procedure SetSeen() begin Seen := true; end;
}
"#;
    let o = al_testkit::outcomes_in(src, "T");
    assert!(
        matches!(&o[0].1, Outcome::NotRun(why) if why.contains("SingleInstance")),
        "{o:?}"
    );
}
