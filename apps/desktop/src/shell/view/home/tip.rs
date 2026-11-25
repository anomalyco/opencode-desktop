//! Provider connect hint at Home's foot (`providers/connect/tip.tsx`, `home/route.tsx`, #64885).
//!
//! Shown while no provider is connected and not dismissed within the last 30 days. State lives in
//! `opencode_kernel::StateSlots` (never a process-wide thread-local) and writes to storage only
//! when `s.persists()` is true.

use std::cell::Cell;

use opencode_kernel::Slot;

use super::*;

/// `dismissalDuration`: 30 days in ms.
const DISMISSAL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// `Persist.global("new-session.provider-tip")`.
const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "new-session.provider-tip";

pub static PROVIDER_TIP: Slot<ProviderTipState> = Slot::new();

#[derive(Default)]
pub struct ProviderTipState {
    dismissed_at: Cell<Option<i64>>,
}

impl ProviderTipState {
    fn read(&self) -> i64 {
        if let Some(at) = self.dismissed_at.get() {
            return at;
        }
        let at = crate::storage::json(GLOBAL, KEY)
            .and_then(|v| v["dismissedAt"].as_f64())
            .map_or(0, |ms| ms as i64);
        self.dismissed_at.set(Some(at));
        at
    }
}

pub fn dismiss(s: &mut App, tx: &mut Tx) {
    let now = *s.now_ms;
    s.kernel.slots.get_mut(&PROVIDER_TIP).dismissed_at.set(Some(now));
    if s.persists() {
        crate::storage::set_json(GLOBAL, KEY, &serde_json::json!({ "dismissedAt": now }));
    }
    tx.animated();
}

/// `anyConnection`: returns `None` until the default location's integrations have loaded (unless a
/// non-keyless model already confirms a connection), preventing a premature flash for Console/Go
/// users while integrations are still loading.
fn any_connection(s: &App) -> Option<bool> {
    let models = &s.composer_ext.pickers.models;
    let beyond_free = models.iter().any(|m| m.provider != "opencode");
    if beyond_free {
        return Some(true);
    }
    let list = s.integrations(None)?;
    Some(list.iter().any(|i| !i.connections.is_empty()))
}

/// Bottom fade and centered hint over Home's scroller when nothing is connected.
pub(super) fn tip(s: &App, p: &Pal) -> Vec<El> {
    if !visible(s) {
        return vec![];
    }
    let fade = el()
        .key("home-tip-fade")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(64.0)
        .a11y_hidden()
        .linear_gradient(0.0, [(0.0, p.bg_base), (0.4, p.bg_base), (1.0, p.bg_base.alpha(0.0))]);
    let open = row()
        .key("home-tip-open")
        .role("button")
        .label_static(t!("home.providerTip"))
        .h(24.0)
        .shrink()
        .min_w(0.0)
        .pl(6.0)
        .center_y()
        .radius(4.0)
        .focus_inset(p)
        .hover(p.hover)
        .hover_group()
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_connect(None, true, tx))
        .child(text(t!("home.providerTip")).font(UI).color(p.faint).group_color(p.muted).truncate())
        .child(
            row()
                .key("chevron")
                .size(24.0, 24.0)
                .center()
                .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.faint).group_color(p.muted).rotate(-std::f32::consts::FRAC_PI_2).mirror_in_rtl()),
        );
    let close = row()
        .key("home-tip-dismiss")
        .role("button")
        .label_static(t!("common.dismiss"))
        .help(t!("common.dismiss"))
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .focus_inset(p)
        .hover(p.hover)
        .on_press_tx(dismiss)
        .child(icon(&ic::CLOSE, 16.0).color(p.icon_muted));
    let hint = row()
        .key("home-tip")
        .h(24.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .gap(4.0)
        .hover_group()
        .child(open)
        .child(session_view::style::hover_reveal(close));
    let place = row()
        .key("home-tip-place")
        .pin(Some(0.0), None, Some(0.0), Some(16.0))
        .px(40.0)
        .center()
        .child(hint);
    vec![fade, place]
}

/// The same hint at the new-session page's foot, without the fade (`new-session/view.tsx`'s
/// `NewSessionTips`): Desktop shows it there as on Home.
pub(crate) fn new_session(s: &App, p: &Pal) -> Option<El> {
    tip(s, p).pop()
}

/// Nothing connected, and not dismissed in the last 30 days.
fn visible(s: &App) -> bool {
    let dismissed = s.kernel.slots.get(&PROVIDER_TIP).read();
    any_connection(s) == Some(false) && *s.now_ms - dismissed >= DISMISSAL_MS
}
