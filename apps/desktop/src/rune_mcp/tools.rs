//! The `rune` client MCP tools (`tools.rune.*` in Code Mode): the plugins' own. The browser
//! plugin brings `browser_*`, the file plugin `open_file`, the terminal plugin `open_terminal`
//! and the review plugin `open_diff` (`plugin_host::mcp_tools`).

use opencode_kit::client_mcp::{McpCall, McpResult, McpTool};
use rune::Tx;

use crate::App;

pub fn catalog() -> Vec<McpTool> {
    crate::plugin_host::mcp_tools()
}

/// Routes `call` to the plugin whose tool it names, in this window.
pub(super) fn execute(s: &mut App, sid: &str, call: McpCall, tx: &mut Tx) {
    let name = call.name.clone();
    if let Some(call) = crate::plugin_host::mcp_call(s, sid, call, tx) {
        call.respond(McpResult::error(format!("Unknown tool: {name}")));
    }
}
