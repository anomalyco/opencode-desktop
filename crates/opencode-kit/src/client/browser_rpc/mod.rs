//! Rust client, explicit state machines, binary multiplexed WebSocket stream client, and remote
//! tunnel proxy for the OpenCode server's `experimental.browser` v4 RPC protocol
//! (`packages/plugin-browser/src/{rpc,machine,connection,tunnel,proxy,stream-wire,stream-mux}.ts`,
//! `packages/gui-plugins/src/browser/{pane,pane-machine,machine,network,tunnel-client}.ts`).

#[cfg(not(target_arch = "wasm32"))]
mod framing;
pub mod http;
pub mod machine;
pub mod proxy_machine;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream_mux;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream_wire;
#[cfg(not(target_arch = "wasm32"))]
pub mod tunnel_client;
#[cfg(not(target_arch = "wasm32"))]
mod tunnel_pool;
#[cfg(not(target_arch = "wasm32"))]
pub mod tunnel_proxy;
#[cfg(not(target_arch = "wasm32"))]
mod tunnel_pump;
#[cfg(not(target_arch = "wasm32"))]
mod tunnel_stream;
pub mod types;

pub use http::{AbortHandle, classify_attach_error, classify_error, rpc_path};
pub use machine::{AttachEffect, AttachEvent, AttachMachine, AttachState, BlockReason};
pub use proxy_machine::{ProxyEffect, ProxyEvent, ProxyState, ReadEvent, ReadState};
#[cfg(not(target_arch = "wasm32"))]
pub use stream_mux::{ClientMux, MuxConfig, MuxStream, ReadOutcome};
#[cfg(not(target_arch = "wasm32"))]
pub use stream_wire::{Frame, FrameType, ResetCode, WireProtocolError};
#[cfg(not(target_arch = "wasm32"))]
pub use tunnel_client::{open_authenticated_websocket, resolve_browser_tunnel_url};
#[cfg(not(target_arch = "wasm32"))]
pub use tunnel_proxy::{PROXY_REALM, ProxyCredentials, TunnelProxy};
pub use types::{
    AttachOutcome, BROWSER_TUNNEL_PATH, BrowserState, CLOSE_ATTACHMENT_CLOSED,
    CLOSE_ATTACHMENT_REPLACED, CONTROL_EVENT, Command, ControlFrame, LocationRef, MAX_FILE_BYTES,
    MAX_TEXT, MAX_TUNNELS, MAX_WAIT_MS, MuxCloseInfo, Outcome, PROMPT_GATE_TIMEOUT_MS,
    PROTOCOL_VERSION, REGISTER_TIMEOUT_MS, RETRY_BASE_MS, RETRY_MAX_MS, RPC_ID, ResultPayload,
    RpcCallError, STREAM_CAPABILITY, STREAM_PROTOCOL_VERSION, TUNNEL_CHUNK_BYTES, Tab, TargetMeta,
    TunnelRead, TunnelTarget, TunnelTicket, TunnelUpgradeError, TunnelUpgradeFailureKind,
    WireFile, decode_control, failure_code, is_valid_file_id, is_valid_tab_id, new_connection_id,
    new_file_id, new_tab_id, normalize_ref, random_hex, uuid_v4,
};
