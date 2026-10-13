//! al-host — capability traits the core crates see instead of the OS (ADR-006).
//!
//! The only capability so far is `DotNet` (ADR-014): [`dotnet::DotNetHost`] opens one
//! [`dotnet::DotNetSession`] per VM session. The default, [`dotnet::NoDotNet`], refuses
//! everything, so core tests and a future WASM build never need .NET.

pub mod dotnet;
