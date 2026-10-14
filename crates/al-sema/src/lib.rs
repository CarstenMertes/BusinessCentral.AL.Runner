//! Semantic model: a workspace of apps, their objects, and resolution.

pub mod base_access;
pub mod calcformula;
pub mod check;
pub mod lower;
pub mod members;
pub mod pages;
pub mod queries;
pub mod relation;
pub mod reports;
pub mod types;
pub mod workspace;
pub mod xmlports;

pub use workspace::{AppKind, AppSource, Diagnostic, ObjKind, Resolution, Workspace};
