//! The AL call stack of an error (`GetLastErrorCallStack`).
//!
//! The VM keeps the routine and the current statement position of every frame on the stack
//! (`Session::call_stack`, set by `Stmt::Pos` markers). An error is not given its stack when it
//! is raised: each frame it unwinds through adds its own line, innermost first, and the frame
//! that catches it (`asserterror`, a TryFunction call, a guarded `Codeunit.Run`) adds the last.
//!
//! The corpus pins only parts of the text (codeunit/TestCodeunitAlCallStack.al: `(CodeUnit N)`,
//! ` line `, ` by `, ` version `, `OnRun(Trigger)` and no `(Trigger)` on a procedure). The frame
//! format and the line counting follow AL Runner v2, which measured them on real BC
//! (`AlCallStackCapture.FormatFrame`, `AlSourceSpanCodec.RelativeLine`):
//!
//! ```text
//! "Object Name"(CodeUnit 60212).RaiseError line 2 - App Name by Publisher version 1.0.0.0
//! ```
//!
//! One frame per line, innermost first; a trigger is `OnRun(Trigger)`; `line` is the line of the
//! statement that was running minus the line of the `procedure` or `trigger` keyword. A frame the
//! format is not known for (an object type that has not been measured, a field trigger, a routine
//! with no source) puts `UNTRACKED_CALL_STACK` in the stack, and no text function reads a text
//! that holds it.

use crate::{CallFrame, Session};
use al_builtins::native::{AlError, UNTRACKED_CALL_STACK};
use al_ir as ir;

impl Session {
    /// Adds the frame at the top of the call stack to the error's stack. No frame, no change.
    pub(crate) fn add_top_frame(&self, e: &mut AlError) {
        if let (Some(&object), Some(&frame)) = (self.frames.last(), self.call_stack.last()) {
            self.add_frame(e, object, frame);
        }
    }

    /// Adds the line of routine `frame` of `object` to the error's stack.
    pub(crate) fn add_frame(&self, e: &mut AlError, object: usize, frame: CallFrame) {
        if !e.call_stack.is_empty() {
            e.call_stack.push('\n');
        }
        match frame_line(&self.engine.program, object, frame) {
            Some(line) => e.call_stack.push_str(&line),
            None => e.call_stack.push_str(UNTRACKED_CALL_STACK),
        }
    }
}

/// The text of one frame; `None` where the format is not known.
fn frame_line(program: &ir::Program, object: usize, frame: CallFrame) -> Option<String> {
    let obj = &program.objects[object];
    let routine = obj.routines.get(frame.routine)?;
    // Only the object types the format was measured for (v2: CodeUnit, Table).
    let kind = match obj.kind.as_str() {
        "codeunit" => "CodeUnit",
        "table" => "Table",
        _ => return None,
    };
    // A field trigger (`Name - OnValidate`) and a routine with an unusual name are not measured.
    if routine.field.is_some() || !routine.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let stmt = program.pos(frame.pos)?;
    let signature = program.pos(routine.pos)?;
    let line = stmt.line.checked_sub(signature.line)?;
    let mut out = format!(
        "\"{}\"({kind} {}).{}{} line {line}",
        obj.name.replace('"', "\"\""),
        obj.id?,
        routine.name,
        if routine.is_trigger { "(Trigger)" } else { "" },
    );
    if let Some(app) = program.apps.get(obj.app as usize) {
        if !app.publisher.is_empty() && !app.version.is_empty() {
            out.push_str(&format!(" - {} by {} version {}", app.name, app.publisher, app.version));
        }
    }
    Some(out)
}
