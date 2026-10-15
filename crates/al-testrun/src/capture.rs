//! The capture of a test (`RunOptions::capture`): the records of the VM, with the places of the statements they name.

use crate::events::{CapturedAt, Loc};
use al_ir::Program;
use al_vm::Captured;

/// `captured` with the file, line and column of every statement it names. `order` maps an app index of the program to
/// the caller's.
pub fn resolve(program: &Program, order: &[usize], captured: Captured) -> CapturedAt {
    let mut ids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    ids.extend(captured.values.iter().map(|v| v.statement));
    for l in &captured.loops {
        ids.insert(l.statement);
        ids.extend(l.iterations.iter().flatten().copied());
    }
    ids.extend(captured.messages.iter().filter_map(|m| m.statement));
    let mut locations = std::collections::BTreeMap::new();
    for id in ids {
        let Some(p) = program.pos(id) else { continue };
        let Some(f) = program.sources.get(p.file as usize) else {
            continue;
        };
        locations.insert(
            id,
            Loc {
                app: order.get(f.app as usize).copied().unwrap_or(f.app as usize),
                file: f.path.clone(),
                line: p.line,
                column: p.col,
                end_line: p.end_line,
            },
        );
    }
    CapturedAt { captured, locations }
}
