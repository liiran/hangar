//! hangar: run many coding-agent sessions on one machine.
//!
//! * [`pool`]: one running copy of each MCP server, shared by every session
//!   through `hangar-shim`.
//! * [`sessions`]: many headless Claude Code sessions, admitted by the
//!   memory [`budget`], shown in the web UI served by [`api`].
//! * [`bench`]: measures what that saves; [`verify`] checks it costs no quality.

pub mod agent;
pub mod api;
pub mod bench;
pub mod budget;
pub mod claude_config;
pub mod client;
pub mod config;
pub mod daemon;
pub mod folders;
pub mod jsonrpc;
pub mod launch;
pub mod policy;
pub mod pool;
pub mod procs;
pub mod sessions;
pub mod util;
pub mod verify;

pub use hangar_proto as proto;
