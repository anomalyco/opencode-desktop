//! Line-delimited JSON wire protocol between `crates/browser-pane` and `opencode-browser-engine`.

use rune::{Cursor, Mods, PointerButton};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MOD_SHIFT: u8 = 1 << 0;
pub const MOD_CTRL: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_CMD: u8 = 1 << 3;

pub fn pack_mods(m: Mods) -> u8 {
    let mut bits = 0;
    if m.shift {
        bits |= MOD_SHIFT;
    }
    if m.ctrl {
        bits |= MOD_CTRL;
    }
    if m.alt {
        bits |= MOD_ALT;
    }
    if m.cmd || m.logo {
        bits |= MOD_CMD;
    }
    bits
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerBtn {
    #[default]
    Primary,
    Secondary,
    Middle,
}

impl From<PointerButton> for PointerBtn {
    fn from(b: PointerButton) -> Self {
        match b {
            PointerButton::Secondary => Self::Secondary,
            PointerButton::Middle => Self::Middle,
            _ => Self::Primary,
        }
    }
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

impl CursorKind {
    pub fn to_rune(self) -> Cursor {
        match self {
            Self::Default => Cursor::Default,
            Self::Pointer => Cursor::Pointer,
            Self::Text => Cursor::Text,
            Self::Crosshair => Cursor::Crosshair,
            Self::ResizeColumn => Cursor::ResizeColumn,
            Self::ResizeRow => Cursor::ResizeRow,
            Self::Grab => Cursor::Grab,
            Self::Grabbing => Cursor::Grabbing,
            Self::NotAllowed => Cursor::NotAllowed,
            Self::Move => Cursor::Move,
        }
    }
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
}

/// An element chosen with the visual element picker (`Overlay.setInspectMode`), ready to insert
/// as an `@eN` reference in the session composer.
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
    Hello {
        mach_service: Option<String>,
        pid: u32,
    },
    OpenTab {
        tab_id: u64,
        url: String,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
        focused: bool,
    },
    CloseTab {
        tab_id: u64,
    },
    FocusTab {
        tab_id: u64,
    },
    Navigate {
        tab_id: u64,
        url: String,
    },
    Back {
        tab_id: u64,
    },
    Forward {
        tab_id: u64,
    },
    Reload {
        tab_id: u64,
    },
    Stop {
        tab_id: u64,
    },
    SetZoom {
        tab_id: u64,
        factor: f64,
    },
    Resize {
        tab_id: u64,
        width_pt: f32,
        height_pt: f32,
        scale: f32,
    },
    MouseMove {
        tab_id: u64,
        x: f32,
        y: f32,
        mods: u8,
        leave: bool,
    },
    MouseButton {
        tab_id: u64,
        x: f32,
        y: f32,
        button: PointerBtn,
        up: bool,
        clicks: u32,
        mods: u8,
    },
    Wheel {
        tab_id: u64,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        mods: u8,
    },
    Key {
        tab_id: u64,
        down: bool,
        text: Option<String>,
        named: Option<String>,
        mods: u8,
    },
    EditCmd {
        tab_id: u64,
        cmd: String,
    },
    Ime {
        tab_id: u64,
        commit: Option<String>,
        preedit: Option<String>,
    },
    SetInspect {
        tab_id: u64,
        enabled: bool,
    },
    HighlightNode {
        tab_id: u64,
        backend_node_id: Option<i64>,
    },
    CdpCall {
        tab_id: u64,
        request_id: u64,
        method: String,
        params: Value,
        session_id: Option<String>,
    },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineMsg {
    Ready {
        chromium_major: u32,
        pid: u32,
    },
    TabList {
        tabs: Vec<TabSnapshot>,
    },
    TabUpdate {
        tab: TabSnapshot,
    },
    TabClosed {
        tab_id: u64,
    },
    InspectState {
        tab_id: u64,
        active: bool,
        picked: Option<PickedElement>,
    },
    CdpResponse {
        tab_id: u64,
        request_id: u64,
        ok: bool,
        result: Value,
    },
    CdpEvent {
        tab_id: u64,
        method: String,
        params: Value,
        session_id: Option<String>,
    },
    CpuFrame {
        tab_id: u64,
        frame_id: u64,
        width: u32,
        height: u32,
        bgra_b64: String,
    },
}
