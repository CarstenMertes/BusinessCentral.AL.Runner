//! TaskScheduler tasks (al-vm tasks.rs). Pins: platform/W6TaskScheduler.al:88-176, platform/lt-platform-members/
//! PlTTaskScheduler.al:40-215.

use al_builtins::native::Signal;

const SRC: &str = r#"
codeunit 50100 "TS Target" { trigger OnRun() begin end; }
codeunit 50101 "TS Probe"
{
    procedure Flow(): Text
    var
        Id: Guid;
        Id2: Guid;
        Empty: Guid;
        R: Text;
    begin
        Id := TaskScheduler.CreateTask(Codeunit::"TS Target", 0, false);
        Id2 := TaskScheduler.CreateTask(Codeunit::"TS Target", 0, true, CompanyName(), CurrentDateTime() + 1000);
        R := Format(IsNullGuid(Id)) + Format(Id <> Id2) + Format(TaskScheduler.TaskExists(Id)) + Format(TaskScheduler.TaskExists(Id2));
        TaskScheduler.SetTaskReady(Id);
        TaskScheduler.SetTaskReady(CreateGuid());
        R += Format(TaskScheduler.TaskExists(Id));
        R += Format(TaskScheduler.CancelTask(Id)) + Format(TaskScheduler.TaskExists(Id)) + Format(TaskScheduler.CancelTask(Id));
        R += Format(TaskScheduler.TaskExists(CreateGuid())) + Format(TaskScheduler.TaskExists(Empty)) + Format(TaskScheduler.CancelTask(Empty));
        TaskScheduler.CreateTask(0, 0, false);
        exit(R);
    end;

    procedure AfterCommit(): Boolean
    var
        Id: Guid;
    begin
        Id := TaskScheduler.CreateTask(Codeunit::"TS Target", 0, false);
        Commit();
        exit(TaskScheduler.TaskExists(Id));
    end;

    procedure UnknownCodeunit()
    begin
        TaskScheduler.CreateTask(99999999, 0, false);
    end;
}
"#;

#[test]
fn created_tasks_exist_until_cancelled() {
    let r = al_testkit::text(al_testkit::run_in(SRC, "TS Probe", "Flow"));
    assert_eq!(r, "NoYesYesYesYesYesNoNoNoNoNo");
}

#[test]
fn a_task_whose_creation_was_committed_is_refused() {
    let r = al_testkit::run_in(SRC, "TS Probe", "AfterCommit");
    assert!(
        matches!(&r, Err(Signal::Unsupported(w)) if w.contains("committed")),
        "{r:?}"
    );
}

#[test]
fn an_unknown_codeunit_raises_the_missing_object_error() {
    let m = al_testkit::error_message(al_testkit::run_in(SRC, "TS Probe", "UnknownCodeunit"));
    assert!(
        m.starts_with("You tried to invoke the CodeUnit object with the ID 99999999 from the object"),
        "{m}"
    );
}
