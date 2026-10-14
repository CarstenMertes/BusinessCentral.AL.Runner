//! Runs the corpus's `[Test]` codeunits and reports them against the oracle.

pub mod args;
pub mod baseline;
pub mod config;
pub mod corpus;
pub mod deps;
pub mod dotnet;
pub mod drift;
pub mod explain;
pub mod host;
pub mod insight;
pub mod isolation;
pub mod oracle;
pub mod output;
pub mod pin;
pub mod report;
pub mod runner;
pub mod select;
pub mod runtime;
pub mod target;
