//! Wire message definitions matching `crates/browser-pane/src/protocol.rs` without pulling `rune`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[path = "../../../crates/browser-pane/src/device.rs"]
pub mod device;

pub const MOD_SHIFT: u8 = 1 << 0;
pub const MOD_CTRL: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_CMD: u8 = 1 << 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerBtn {
    #[default]
    Primary,
    Secondary,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    #[default]
    Default,
    Pointer,
    Text,
    Crosshair,
    ResizeColumn,
    ResizeRow,
    Grab,
    Grabbing,
    NotAllowed,
    Move,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub tab_id: u64,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub zoom: f64,
    pub cursor: CursorKind,
    /// Why the page shows no document: Chromium's net error for a main-frame load that failed, or
    /// `page_crashed` when its renderer went away. Cleared by the next document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    /// Set on isolated `oc-preview://` tabs so browser chrome marks them as previews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewKindTag>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKindTag {
    Local,
    Remote,
    Artifact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewSpec {
    Local { root: String, document: String },
    Remote { root: String, session_prefix: String, document: String },
    Artifact { artifact_id: String, path: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickedElement {
    pub tab_id: u64,
    pub eref: String,
    pub backend_node_id: i64,
    pub selector: String,
    pub label: String,
    pub role: Option<String>,
    pub name: Option<String>,
    pub text: Option<String>,
    pub rect: [f32; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMsg {
    /// `prompts`: the app shows and answers page dialogs, permission prompts, and HTTP auth;
    /// older apps leave it false so the engine suppresses dialogs and denies permissions/auth.
    Hello {
        mach_service: Option<String>,
        pid: u32,
        #[serde(default)]
        prompts: bool,
    },
    OpenTab {
        tab_id: u64,
        url: String,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
        #[serde(default)]
        file_roots: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview: Option<PreviewSpec>,
    },
    SetTabPolicy { tab_id: u64, file_roots: Vec<String> },
    CloseTab { tab_id: u64 },
    FocusTab { tab_id: u64 },
    Navigate { tab_id: u64, url: String },
    Back { tab_id: u64 },
    Forward { tab_id: u64 },
    Reload { tab_id: u64 },
    Stop { tab_id: u64 },
    SetZoom { tab_id: u64, factor: f64 },
    SetColorScheme { dark: bool },
    Resize { tab_id: u64, width_pt: f32, height_pt: f32, scale: f32 },
    Emulate { tab_id: u64, device: Option<device::Device>, scale: f32 },
    MouseMove { tab_id: u64, x: f32, y: f32, mods: u8, leave: bool },
    MouseButton { tab_id: u64, x: f32, y: f32, button: PointerBtn, up: bool, clicks: u32, mods: u8 },
    Wheel { tab_id: u64, x: f32, y: f32, dx: f32, dy: f32, mods: u8 },
    Key { tab_id: u64, down: bool, text: Option<String>, named: Option<String>, mods: u8 },
    EditCmd { tab_id: u64, cmd: String },
    Ime { tab_id: u64, commit: Option<String>, preedit: Option<String> },
    SetInspect { tab_id: u64, enabled: bool },
    HighlightNode { tab_id: u64, backend_node_id: Option<i64> },
    CdpCall { tab_id: u64, request_id: u64, method: String, params: Value, session_id: Option<String> },
    SiteCookies { tab_id: u64, request_id: u64 },
    ClearSite { tab_id: u64, request_id: u64 },
    /// Linux: the app's GPU has finished reading `frame_id` (and every frame before it) of
    /// buffer set `pool`; a read of a set the tab no longer has frees nothing.
    FrameRead { tab_id: u64, pool: u64, frame_id: u64 },
    /// Linux: the app can't import the engine's dmabufs; frames come as BGRA paints from now on.
    DmabufFailed { reason: String },
    /// The app's answer to a held JavaScript dialog or `beforeunload` (`EngineMsg::Dialog`).
    AnswerDialog { tab_id: u64, id: u64, accept: bool, text: String },
    /// The app's answer to a held permission request (`EngineMsg::PermissionRequest`).
    AnswerPermission { tab_id: u64, id: u64, allow: bool },
    /// The app's answer to an HTTP 401 auth challenge (`EngineMsg::AuthRequest`); `None` cancels.
    AnswerAuth { tab_id: u64, id: u64, username: Option<String>, password: Option<String> },
    /// Marks a tab as agent-driven (cancels native file choosers, skips `beforeunload`, rejects HTTP auth).
    SetAgentDriven { tab_id: u64, agent_driven: bool },
    /// App-proxied response headers for a remote workspace or artifact `EngineMsg::PreviewFetch`.
    PreviewResponseHead {
        req_id: u64,
        ok: bool,
        status: u16,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_length: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_range: Option<String>,
    },
    /// App-proxied base64 body chunk for a pending `EngineMsg::PreviewFetch`.
    PreviewResponseChunk { req_id: u64, data_b64: String },
    /// Marks EOF (`ok: true`) or failure (`ok: false`) for a pending `EngineMsg::PreviewFetch`.
    PreviewResponseEnd { req_id: u64, ok: bool },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineMsg {
    /// `downloads`: keeps page downloads; `policy`: enforces URL/CDP policy; `prompts`: holds
    /// dialogs/permissions/auth for the app; `popups`: routes pop-ups to managed tabs; `preview`:
    /// serves isolated `oc-preview://` tabs.
    Ready {
        chromium_major: u32,
        pid: u32,
        downloads: bool,
        policy: bool,
        #[serde(default)]
        prompts: bool,
        #[serde(default)]
        popups: bool,
        #[serde(default)]
        preview: bool,
    },
    /// Requests bytes for a remote workspace or artifact preview from the app host (server
    /// credentials never enter the engine process).
    PreviewFetch {
        req_id: u64,
        tab_id: u64,
        kind: PreviewKindTag,
        path: String,
        method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<String>,
    },
    /// A preview page attempted to load a file outside its grant or a secret/hidden path.
    PreviewDenied { tab_id: u64 },
    Refused { tab_id: u64, reason: String },
    Dialog { tab_id: u64, dialog: PageDialog },
    DialogsReset { tab_id: u64 },
    PermissionRequest { tab_id: u64, request: PermissionPrompt },
    PermissionDismissed { tab_id: u64, id: u64 },
    AuthRequest { tab_id: u64, challenge: AuthChallenge },
    AuthReset { tab_id: u64 },
    AuthAgentRequired { tab_id: u64, origin: String },
    PopupOpen { tab_id: u64, url: String, background: bool },
    PopupBlocked { tab_id: u64, url: String },
    FileChooserBlocked { tab_id: u64 },
    TabList { tabs: Vec<TabSnapshot> },
    TabUpdate { tab: TabSnapshot },
    TabClosed { tab_id: u64 },
    InspectState { tab_id: u64, active: bool, picked: Option<PickedElement> },
    CdpResponse { tab_id: u64, request_id: u64, ok: bool, result: Value },
    CdpEvent { tab_id: u64, method: String, params: Value, session_id: Option<String> },
    CpuFrame { tab_id: u64, frame_id: u64, width: u32, height: u32, bgra_b64: String },
    /// Windows: a finished BGRA frame in the named D3D11 shared texture. The app signals the
    /// named shared fence to `frame_id` once it has read it, and the engine reuses the texture
    /// only after that.
    SharedFrame { tab_id: u64, frame_id: u64, width: u32, height: u32, texture: String, release_fence: String },
    /// A download started, moved on or ended. An app from before it skips the message.
    Download { tab_id: u64, file: DownloadFile },
    /// Linux: a tab's new linear ARGB8888 dmabufs, one file descriptor each, sent with this line
    /// (`SCM_RIGHTS`). Frames name them by `pool` and index.
    DmabufPool { tab_id: u64, pool: u64, buffers: u32, width: u32, height: u32, stride: u32, offset: u32, modifier: u64 },
    /// Linux: a finished frame in buffer `slot` of `pool`. The app says when it has read it
    /// (`HostMsg::FrameRead`), and the engine writes that buffer again only after.
    DmabufFrame { tab_id: u64, frame_id: u64, pool: u64, slot: u32 },
}

/// A file a page downloaded, kept in the engine's private downloads folder for the agent's tools
/// (Desktop's browser keeps them the same way: `gui-plugins/src/browser/files.ts`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DownloadFile {
    /// `file_<n>`, unique in the engine's run.
    pub id: String,
    /// The name the page gave it.
    pub name: String,
    pub mime: String,
    /// Bytes received so far.
    pub bytes: u64,
    pub state: DownloadState,
    /// Where it is on this computer.
    pub path: String,
    /// Why it failed: `too_large` (over 5 MiB, Desktop's limit), `canceled`, or Chromium's
    /// interrupt reason (`network_failed`, `file_no_space`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Pending,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageDialog {
    pub id: u64,
    pub kind: DialogKind,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub default_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionKind {
    Camera,
    Microphone,
    Location,
    Notifications,
    ClipboardRead,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPrompt {
    pub id: u64,
    pub origin: String,
    pub top_origin: String,
    pub kinds: Vec<PermissionKind>,
    #[serde(default)]
    pub is_media: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub id: u64,
    pub origin: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub realm: String,
    pub scheme: String,
}
