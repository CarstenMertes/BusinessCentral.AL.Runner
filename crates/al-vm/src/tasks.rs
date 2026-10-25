//! `TaskScheduler.CreateTask`, `TaskExists`, `CancelTask` and `SetTaskReady`. A test never sees a task run (it starts in a
//! session of its own after the creating transaction commits: platform/W6TaskScheduler.al:71-86
//! CreateTask_DoesTheTaskRun_AfterAPause), so a task is only an id the session knows until the transaction ends.
//! Pinned by platform/W6TaskScheduler.al:88-176 and platform/lt-platform-members/PlTTaskScheduler.al:40-215:
//! - a created task exists, also when created not ready, with a failure codeunit, a company, a start time or a record id;
//! - CancelTask of it answers true and the task no longer exists; a second CancelTask answers false;
//! - TaskExists and CancelTask of an unknown or the null id answer false; SetTaskReady of an unknown id returns, of a
//!   created task leaves it existing;
//! - a rollback removes the tasks of the transaction (PlTTaskScheduler.al:198-215 CreateTask_RolledBackWithTransaction);
//! - an unknown codeunit or failure codeunit raises the missing-object error (PlTTaskScheduler.al:150-160, 172-182);
//!   codeunit 0 and an unknown company do not (PlTTaskScheduler.al:162-170, 184-193).
//!
//! Whether a task whose creation was committed still exists is not pinned (it may have run): asking about one is refused.

use crate::Session;
use al_builtins::builtin_id::ids;
use al_builtins::describe::Describe;
use al_builtins::native::Signal;
use al_builtins::value::Value;

/// The tasks this session created.
#[derive(Default, Clone)]
pub(crate) struct Tasks {
    /// Created in the running transaction: a rollback drops them.
    open: std::collections::BTreeSet<[u8; 16]>,
    /// Created in a transaction that was committed: what became of them is not pinned.
    committed: std::collections::BTreeSet<[u8; 16]>,
}

impl Tasks {
    pub(crate) fn commit(&mut self) {
        let open = std::mem::take(&mut self.open);
        self.committed.extend(open);
    }

    pub(crate) fn rollback(&mut self) {
        self.open.clear();
    }
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

impl Session {
    /// A TaskScheduler builtin, or `None` for any other id.
    pub(crate) fn task_scheduler_op(&mut self, id: al_ir::BuiltinId, args: &[Value]) -> Option<Result<Value, Signal>> {
        let answer = match id {
            ids::TASKSCHEDULER_CREATETASK_1 | ids::TASKSCHEDULER_CREATETASK_2 => self.create_task(args),
            ids::TASKSCHEDULER_TASKEXISTS_1 => self
                .task_id(args, "TaskExists")
                .map(|t| t.map_or(Value::Boolean(false), |t| Value::Boolean(self.tasks.open.contains(&t)))),
            ids::TASKSCHEDULER_CANCELTASK_1 => self
                .task_id(args, "CancelTask")
                .map(|t| Value::Boolean(t.is_some_and(|t| self.tasks.open.remove(&t)))),
            ids::TASKSCHEDULER_SETTASKREADY_1 => self.task_id(args, "SetTaskReady").map(|_| Value::Null),
            _ => return None,
        };
        Some(answer)
    }

    /// The task id argument; refused for a task whose creation was committed.
    fn task_id(&self, args: &[Value], what: &str) -> Result<Option<[u8; 16]>, Signal> {
        let Some(Value::Guid(g)) = args.first() else {
            return unsupported(format!("TaskScheduler.{what} with id {}", args.first().describe()));
        };
        if self.tasks.committed.contains(g) {
            return unsupported(format!(
                "TaskScheduler.{what} of a task whose creation was committed (whether it still exists or already ran is not pinned)"
            ));
        }
        Ok(Some(*g))
    }

    fn create_task(&mut self, args: &[Value]) -> Result<Value, Signal> {
        for arg in args.iter().take(2) {
            let Value::Integer(n) = arg else {
                return unsupported(format!("TaskScheduler.CreateTask with codeunit {}", arg.describe()));
            };
            if *n != 0 && self.engine.codeunit_object(u32::try_from(*n).ok()).is_none() {
                return Err(self.missing_object("CodeUnit", i64::from(*n)));
            }
        }
        let id = self.fresh_guid();
        self.tasks.open.insert(id);
        Ok(Value::Guid(id))
    }
}
