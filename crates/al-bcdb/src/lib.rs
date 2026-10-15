//! Reads Business Central data straight out of a SQL Server native backup (`.bak`) or
//! a cloud export (`.bacpac`). A port of bcdb (BusinessCentral.BakReader), the same
//! owner's C# tool. The core does no OS access (ADR-006): it works on a [`io::ByteSource`].

pub mod bacpac;
pub mod bak;
pub mod bcp;
pub mod catalog;
pub mod error;
pub mod io;
pub mod lob;
pub(crate) mod model;
pub mod mtf;
pub mod pagefile;
pub mod query;
pub mod records;
pub mod scsu;
pub mod serve;
pub mod source;
pub mod symbols;
pub mod types;
pub mod value;
pub mod zip;

pub use error::{Error, Result};
pub use source::{BcSource, SourceTable, open, open_indexed};
