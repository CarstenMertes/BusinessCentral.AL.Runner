//! Semantic model: a workspace of apps, their objects, and resolution.

pub mod check;
pub mod lower;
pub mod members;
pub mod types;
pub mod workspace;

pub use workspace::{AppSource, Diagnostic, ObjKind, Resolution, Workspace};
