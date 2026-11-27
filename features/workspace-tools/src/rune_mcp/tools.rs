//! The `rune` client MCP tools (`tools.rune.*` in Code Mode): the plugins' own. The browser
//! plugin brings `browser_*`, the file plugin `open_file`, the terminal plugin `open_terminal`
//! and the review plugin `open_diff` (`plugin_host::mcp_tools`).

use std::sync::OnceLock;

use opencode_kit::client_mcp::{McpCall, McpResult, McpTool};
use rune::Tx;

use super::RuneMcpHost;

/// Process-wide plugin MCP tool catalog callbacks registered once at startup by the host binary.
#[derive(Clone, Copy)]
pub struct ToolsProvider {
    pub catalog: fn() -> Vec<McpTool>,
    pub generation: fn() -> u64,
}

fn empty_catalog() -> Vec<McpTool> {
    Vec::new()
}

fn zero_generation() -> u64 {
    0
}

const DEFAULT_PROVIDER: ToolsProvider = ToolsProvider {
    catalog: empty_catalog,
    generation: zero_generation,
};

static PROVIDER: OnceLock<ToolsProvider> = OnceLock::new();

/// Registers the host's process-wide MCP tool catalog and generation counter.
/// Call once at startup before any [`super::RuneMcpHub`] connections open.
pub fn configure(provider: ToolsProvider) {
    PROVIDER.set(provider).ok();
}

pub fn catalog() -> Vec<McpTool> {
    (PROVIDER.get().map_or(DEFAULT_PROVIDER, |p| *p).catalog)()
}

pub fn generation() -> u64 {
    (PROVIDER.get().map_or(DEFAULT_PROVIDER, |p| *p).generation)()
}

/// Routes `call` to the plugin whose tool it names, in this window.
pub(super) fn execute(host: &mut dyn RuneMcpHost, sid: &str, call: McpCall, tx: &mut Tx) {
    let name = call.name.clone();
    if super::is_browser_tool(&name)
        && let Some(reason) = host.browser_mcp_refusal(sid)
    {
        call.respond(McpResult::error(reason));
        return;
    }
    if let Some(call) = host.dispatch_mcp_call(sid, call, tx) {
        call.respond(McpResult::error(format!("Unknown tool: {name}")));
    }
}
