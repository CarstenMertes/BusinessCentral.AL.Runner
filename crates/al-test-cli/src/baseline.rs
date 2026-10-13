//! The install baseline (ADR-013 decision 5), mirroring AL Runner (`InstallTriggerRunner`,
//! `CompanyInitializer`, `TestExecutor`): once per run, before the first test,
//!
//! 1. every dependency app's install codeunits, dependency apps first; within a codeunit
//!    `OnInstallAppPerCompany`, then `OnInstallAppPerDatabase`;
//! 2. Base Application's `Company-Initialize` (codeunit 2), once, after the dependency installs;
//! 3. a capture of the data ([`Baseline`], ADR-004), put back at the start of every test codeunit.
//!
//! Only install triggers run, never upgrade triggers. A trigger that cannot run stops where it
//! stops: `Company-Initialize` keeps the rows it wrote before (as AL Runner's partial company
//! initialization does), an install trigger is rolled back. What did not run is recorded in the
//! engine's `unseeded` set, so that a `Get` that finds nothing in dependency code is refused for
//! exactly those tables instead of answered.

use crate::runtime::{subscribers_reached, tables_touched};
use al_builtins::native::Signal;
use al_sema::lower::Lowered;
use al_vm::{Baseline, Engine, Session};
use std::collections::BTreeSet;
use std::sync::Arc;

/// What the baseline run leaves.
pub struct BaselineRun {
    pub baseline: Baseline,
    /// Tables the roots that did not run (or stopped early) would have written to, and that hold
    /// no row in the baseline: object indexes.
    pub unseeded: BTreeSet<u32>,
    /// How many tables hold a row in the baseline.
    pub filled: usize,
    /// One line per root that could not run to its end.
    pub notes: Vec<String>,
    /// Roots that ran to their end, and all roots.
    pub ran: usize,
    pub total: usize,
}

fn is_install(o: &al_ir::Object) -> bool {
    o.kind == "codeunit"
        && o.properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Install"))
}

fn trigger(o: &al_ir::Object, name: &str) -> Option<usize> {
    o.routines
        .iter()
        .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case(name))
}

/// Runs the baseline on a fresh session of `engine`. `None`: no dependency app has anything to run.
pub fn run(engine: &Arc<Engine>, lowered: &Lowered) -> Option<BaselineRun> {
    let objs = &lowered.program.objects;
    // Install codeunits, dependencies first (the apps of the program are in dependency order).
    let mut installs: Vec<usize> = (0..objs.len())
        .filter(|&oi| lowered.dependency[oi] && is_install(&objs[oi]))
        .collect();
    installs.sort_by_key(|&oi| (objs[oi].app, oi));
    // (object, trigger, keep what it wrote when it stops early, no company chosen)
    //
    // Per app: every codeunit's OnInstallAppPerCompany, then every OnInstallAppPerDatabase, which
    // runs last and with CompanyName() empty. Measured on real BC 27.5 (a fresh install with two
    // companies, a scratch app; docs/probes-needed.md). AL Runner and ADR-013 as first written ran
    // per codeunit, per-company then per-database, with the company set.
    let mut roots: Vec<(usize, usize, bool, bool)> = Vec::new();
    let mut apps: Vec<usize> = installs.iter().map(|&oi| objs[oi].app as usize).collect();
    apps.dedup();
    for app in apps {
        for (name, no_company) in [("OnInstallAppPerCompany", false), ("OnInstallAppPerDatabase", true)] {
            for &oi in installs.iter().filter(|&&oi| objs[oi].app as usize == app) {
                if let Some(ri) = trigger(&objs[oi], name) {
                    roots.push((oi, ri, false, no_company));
                }
            }
        }
    }
    let company_initialize = (0..objs.len()).find(|&oi| {
        lowered.dependency[oi]
            && objs[oi].kind == "codeunit"
            && objs[oi].id == Some(2)
            && objs[oi].name.eq_ignore_ascii_case("Company-Initialize")
    });
    if let Some(oi) = company_initialize {
        if let Some(ri) = trigger(&objs[oi], "OnRun") {
            roots.push((oi, ri, true, false));
        }
    }
    if roots.is_empty() {
        return None;
    }

    let mut session = Session::with_host(engine.clone(), crate::host::system_host());
    session.set_install_scope(true);
    let mut notes = Vec::new();
    let mut touched = BTreeSet::new();
    let mut ran = 0;
    let company = session.set_company("");
    session.set_company(&company);
    for &(oi, ri, keep_prefix, no_company) in &roots {
        let label = format!("{}.{}", objs[oi].name, objs[oi].routines[ri].name);
        let inst = session.instance(oi);
        if no_company {
            session.set_company("");
        }
        let why = match session.call(oi, ri, inst, Vec::new()) {
            Ok(_) => None,
            Err(Signal::Unsupported(r)) => Some(r),
            Err(Signal::Error(e)) => Some(format!("failed with an error: {}", e.message)),
            Err(_) => None,
        };
        session.set_company(&company);
        match why {
            None => {
                ran += 1;
                session.commit_transaction();
            }
            Some(why) => {
                if keep_prefix {
                    session.commit_transaction();
                } else {
                    session.rollback_transaction();
                }
                notes.push(format!(
                    "{label} did not run to its end{}: {why}",
                    if keep_prefix {
                        " (the rows before it are kept)"
                    } else {
                        ""
                    }
                ));
                touched.extend(tables_touched(lowered, oi, ri));
                for (so, sr) in subscribers_reached(engine, lowered, oi, ri) {
                    touched.extend(tables_touched(lowered, so, sr));
                }
            }
        }
    }
    session.set_install_scope(false);
    let baseline = session.capture_baseline();
    let filled = baseline.nonempty_tables();
    let unseeded = touched.difference(&filled).copied().collect();
    Some(BaselineRun {
        baseline,
        unseeded,
        filled: filled.len(),
        notes,
        ran,
        total: roots.len(),
    })
}
