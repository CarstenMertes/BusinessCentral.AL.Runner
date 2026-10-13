//! Semantic model: a workspace of apps, their objects, and resolution.

pub mod check;
pub mod members;
pub mod types;
pub mod workspace;

pub use workspace::{AppSource, Diagnostic, ObjKind, Resolution, Workspace};
