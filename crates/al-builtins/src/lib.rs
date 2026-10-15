//! The AL standard library: builtin signatures (generated from the docs, see
//! ADR-008) and, later, the value model and native implementations.

pub mod signature;
pub mod stream;
pub mod textfns;
pub mod value;
pub mod version;
pub mod wsac;
pub mod xml;
pub mod xmlport;

pub mod assumptions;
pub mod catalog;
pub mod collation;
pub mod collections;
pub mod datatransfer;
pub mod datefns;
pub mod describe;
pub mod dotnet_sigs;
pub mod errorinfo;
pub mod evaluate;
pub mod file;
pub mod filter;
pub mod format;
pub mod formatstr;
pub mod http;
pub mod iso;
pub mod json;
pub mod mathfns;
pub mod media;
pub mod moduleinfo;
pub mod native;
pub mod ops;
pub mod pagebuiltins;
pub mod pagefmt;
pub mod platform;
pub mod querybuiltins;
pub mod refs;
pub mod sessionfns;
pub mod sessiontypes;
pub mod sysenum;
