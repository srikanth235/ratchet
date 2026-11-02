//! `ratchet-mcp`: Ratchet's MCP server.
//!
//! * [`protocol`], [`server`], [`tool`]: stdio JSON-RPC per the MCP spec (`initialize` with version negotiation, `ping`,
//!   `tools/list`, `tools/call`), over any [`tool::ToolProvider`].
//! * [`sets`], [`args`]: the two session tool sets of the harness plan (chat and optimization); schemas are generated from the
//!   argument types, which use `ratchet-core` enums.
//! * [`session`], [`socket`]: a session serves one set and forwards calls to the daemon over its unix socket with the session's
//!   capability token. The daemon and the cloud runner implement the backends.
//! * [`launch`]: the `McpServer` value that goes into `SessionSpec::mcp`, so every harness adapter hands the CLI this server.
//!
//! The owner mode (`ratchet mcp`) lives in `ratchet-cli`, which owns the command table it is generated from.

pub mod args;
pub mod launch;
pub mod protocol;
pub mod server;
pub mod session;
pub mod sets;
pub mod socket;
pub mod testing;
pub mod tool;

pub use server::{McpServer, ServerInfo};
pub use session::SessionTools;
pub use sets::ToolSet;
pub use tool::{Refusal, Tool, ToolBackend, ToolError, ToolOutput, ToolProvider};
