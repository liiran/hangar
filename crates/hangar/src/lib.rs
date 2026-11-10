//! hangar: run many coding-agent sessions on one machine.
//!
//! * [`pool`]: one running copy of each MCP server, shared by every session
//!   through `hangar-shim`.
//! * [`sessions`]: many headless Claude Code sessions, admitted by the
//!   memory [`budget`], shown in the web UI served by [`api`].
//! * [`bench`]: measures what that saves.

pub mod agent;
pub mod api;
pub mod bench;
pub mod budget;
pub mod claude_config;
pub mod client;
pub mod config;
pub mod daemon;
pub mod jsonrpc;
pub mod launch;
pub mod policy;
pub mod pool;
pub mod procs;
pub mod sessions;
pub mod util;

pub use hangar_proto as proto;
