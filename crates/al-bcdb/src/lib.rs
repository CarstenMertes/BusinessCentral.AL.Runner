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
pub mod mtf;
pub mod pagefile;
pub mod records;
pub mod scsu;
pub mod source;
pub mod types;
pub mod zip;
pub mod value;

pub use error::{Error, Result};
pub use source::{open, BcSource, SourceTable};
