//! What BC's AL test runner does around every test method, as Microsoft's Test Runner app (BC 27.5,
//! `TestFramework/TestRunner/Test Runner.Source.zip`) does it. The platform calls the runner codeunit's
//! OnBeforeTestRun and OnAfterTestRun around each test method; for the OnRun trigger of a test codeunit nothing
//! below happens (TestRunnerMgt.Codeunit.al:92-96, ALTestRunnerResetEnvironment.Codeunit.al:28-29).
//!
//! Before the method, in this order:
//! 1. `Test Runner - Mgt`.PlatformBeforeTestRun runs codeunit 131006 "Permissions Mock" when it exists, which starts
//!    the mock, or stops it when it is started (TestRunnerMgt.Codeunit.al:104-105, :198-208).
//! 2. Its OnBeforeTestMethodRun event reaches "ALTestRunner Reset Environment".BeforeTestMethod
//!    (ALTestRunnerResetEnvironment.Codeunit.al:23-33): ClearLastError and ApplicationArea('') (:26-27), then
//!    ClearLegacyLibraries (:42-63): codeunit 130301 "Reset State Before Test Run" when it exists, then, unless the
//!    method's TestPermissions is Disabled, codeunit 130302 "Set State Test Run" when it exists; then
//!    BindStopSystemTableChanges (:65-75): codeunit 132553 "Block Changes to System Tables" when it exists.
//!
//! After the method, PlatformAfterTestRun runs codeunit 131006 again (TestRunnerMgt.Codeunit.al:127).
//!
//! Each codeunit is Microsoft's own code from the compiled apps (Tests-TestLibraries, Permissions Mock, Prevent
//! Metadata Updates), run by id with `Codeunit.Run` as the runner does. "Exists" is the runner's `AllObj` check: a
//! codeunit with that id in the loaded apps. When none of them is loaded (the corpus apps load only Permissions Mock)
//! only what is loaded runs. The steps of the Test Runner app itself are done here: our runner replaces it.

use super::*;

/// The codeunits the runner runs, found once per program.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunnerReset {
    /// 131006 "Permissions Mock".
    mock: Option<(u32, String)>,
    /// 130301 "Reset State Before Test Run".
    reset_state: Option<(u32, String)>,
    /// 130302 "Set State Test Run".
    set_state: Option<(u32, String)>,
    /// 132553 "Block Changes to System Tables".
    block_changes: Option<(u32, String)>,
}

impl RunnerReset {
    pub(crate) fn of(program: &al_ir::Program) -> RunnerReset {
        let [mock, reset_state, set_state, block_changes] = al_sema::check::demand::TEST_RUNNER_CODEUNITS.map(|id| {
            program
                .objects
                .iter()
                .find(|o| {
                    (o.is_kind(al_ir::ObjKind::Codeunit) || o.is_shell_of(al_ir::ObjKind::Codeunit)) && o.id == Some(id)
                })
                .map(|o| (id, o.name.clone()))
        });
        RunnerReset {
            mock,
            reset_state,
            set_state,
            block_changes,
        }
    }

    /// Steps 1 and 2 before test method `routine` of test codeunit `obj`. The error is why the test is not run.
    pub(crate) fn before(
        &self,
        session: &mut Session,
        obj: &al_ir::Object,
        routine: &al_ir::Routine,
    ) -> Result<(), String> {
        run(session, &self.mock, "TestRunnerMgt.Codeunit.al:105")?;
        // ClearLastError and ApplicationArea('') (ALTestRunnerResetEnvironment.Codeunit.al:26-27).
        session.last_error = Default::default();
        session.start_test_method();
        run(
            session,
            &self.reset_state,
            "ALTestRunnerResetEnvironment.Codeunit.al:48-52",
        )?;
        if !test_permissions(obj, routine).eq_ignore_ascii_case("Disabled") {
            run(
                session,
                &self.set_state,
                "ALTestRunnerResetEnvironment.Codeunit.al:54-62",
            )?;
        }
        run(
            session,
            &self.block_changes,
            "ALTestRunnerResetEnvironment.Codeunit.al:70-74",
        )
    }

    /// The step after the test method.
    pub(crate) fn after(&self, session: &mut Session) -> Result<(), String> {
        session.clear_stop();
        run(session, &self.mock, "TestRunnerMgt.Codeunit.al:127")
    }
}

fn run(session: &mut Session, codeunit: &Option<(u32, String)>, at: &str) -> Result<(), String> {
    let Some((id, name)) = codeunit else {
        return Ok(());
    };
    match session.run_codeunit_by_id(*id) {
        Ok(()) => Ok(()),
        Err(Signal::Unsupported(r)) => Err(format!(
            "BC's test runner runs codeunit {id} \"{name}\" around each test method (Test Runner app, {at}), and it was refused: {r}"
        )),
        Err(Signal::Error(e)) => Err(format!(
            "BC's test runner runs codeunit {id} \"{name}\" around each test method (Test Runner app, {at}), and it failed (what the runner does then is not pinned): {}",
            e.message
        )),
        Err(other) => Err(format!(
            "BC's test runner runs codeunit {id} \"{name}\" around each test method (Test Runner app, {at}), and it ended with {}",
            other.describe()
        )),
    }
}

/// The TestPermissions the platform passes for a test method: the method's `[TestPermissions]` attribute unless it is
/// InheritFromTestCodeunit, else the codeunit's property. With neither, BC passes Restrictive (AL Runner v2,
/// AlRunner/TestRunnerMgtEvents.cs:164-169, read from the platform's NavTestExecution.BeforeTestRunAsync; not pinned by
/// the corpus). Only "Disabled or not" matters here.
pub(crate) fn test_permissions(obj: &al_ir::Object, routine: &al_ir::Routine) -> String {
    routine
        .attributes
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case("TestPermissions"))
        .filter_map(|a| {
            a.text
                .split_once("::")
                .map(|(_, v)| v.trim_end_matches([')', ']', ' ']).trim().to_string())
        })
        .find(|v| !v.eq_ignore_ascii_case("InheritFromTestCodeunit"))
        .or_else(|| {
            obj.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("TestPermissions"))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "Restrictive".into())
}
