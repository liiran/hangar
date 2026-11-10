//! hangar: run many coding-agent sessions on one machine.
//!
//! * [`pool`]: one running copy of each MCP server, shared by every session
//!   through `hangar-shim`.
//! * [`bench`]: measures what that saves.

pub mod agent;
pub mod bench;
pub mod client;
pub mod config;
pub mod daemon;
pub mod jsonrpc;
pub mod launch;
pub mod policy;
pub mod pool;
pub mod procs;

pub use hangar_proto as proto;
