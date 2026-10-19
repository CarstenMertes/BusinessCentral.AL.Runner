//! Runs the corpus's `[Test]` codeunits and reports them against the oracle.

pub use al_testrun::{baseline, deps, dotnet, explain, host, isolation, runtime, target};

pub mod args;
pub mod behavior_coverage;
pub mod config;
pub mod corpus;
pub mod coverage;
pub mod drift;
pub mod insight;
pub mod language_catalog;
pub mod mnc;
pub mod oracle;
pub mod output;
pub mod pin;
pub mod platform_columns;
pub mod report;
pub mod runner;
pub mod select;
