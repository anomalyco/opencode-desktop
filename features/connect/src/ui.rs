//! Shared non-generic dialog chrome, provider marks, and locale search folding used by the
//! Connect provider dialog and ChatGPT plan welcome dialog.

use rune::{Color, Cx, El, Icon, TextStyle, Transition, col, el, icon, row, text};
use rune_kit::{Badge, Button, ButtonVariant, Loader};
use rune_theme_opencode::provider_icons;
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Elevate as _, Pal, kit};

use crate::ConnectHost;

/// Desktop's body copy in its views: 13 on 20 at 440, tracking -0.04.
pub(crate) const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// 13 on 20 at 530.
pub(crate) const BODY_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// 13 on 16 at 530 (`line-height-compact`).
pub(crate) const LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

/// `opencode-logo.tsx`: OpenCode's mark, which Desktop draws for the `opencode` provider.
pub(crate) static OPENCODE_LOGO: Icon = Icon {
    name: "opencode-logo",
    view: 16.0,
    body: r#"<g transform="translate(1.2 1.2) scale(0.85)"><path opacity="0.2" d="M11.1999 12.8H4.79993V6.40002H11.1999V12.8Z" fill="currentColor"/><path d="M11.2 3.2H4.79998V12.8H11.2V3.2ZM14.4 16H1.59998V0H14.4V16Z" fill="currentColor"/></g>"#,
};

/// The dialog's own close cross (`overlays/dialog/dialog.tsx`): a 1 pt stroke corner to corner.
pub(crate) static DIALOG_CLOSE: Icon = Icon {
    name: "dialog-close",
    view: 16.0,
    body: r#"<path d="M12.4446 3.55469L3.55566 12.4436M3.55566 3.55469L12.4446 12.4436" stroke="currentColor" stroke-linejoin="round"/>"#,
};

/// `button-v2` at size normal: 28 tall, 11 in from each side, 6 pt corners, tabular numerals.
#[allow(deprecated)]
pub(crate) fn button(label: impl Into<String>, variant: ButtonVariant) -> Button {
    let label = label.into();
    Button::new(label.clone())
        .variant(variant)
        .size(rune_kit::ButtonSize::Normal)
        .label_with(move |font, ink| text(label).key("label").font(font.features("\"tnum\"")).color(ink))
}

pub(crate) fn badge(p: &Pal, label: impl Into<String>) -> El {
    Badge::new(label).build(kit(p))
}

/// Desktop's `Loader` beside muted words: a sign-in or a check under way.
pub(crate) fn status_row(p: &Pal, key: &str, label: &str) -> El {
    row()
        .key(key.to_owned())
        .role("status")
        .label(label.to_owned())
        .gap(8.0)
        .center_y()
        .child(Loader::new().size(16.0).build(kit(p)))
        .child(text(label.to_owned()).font(BODY).color(p.muted))
}

/// A provider's 16 pt mark (`ProviderModelIcon`): OpenCode's logo, the provider's own icon by
/// id (less a `console-` prefix), else Desktop's custom provider mark.
pub(crate) fn provider_mark(id: &str, color: Color) -> El {
    if id == "opencode" {
        return icon(&OPENCODE_LOGO, 16.0).color(color);
    }
    let bare = id.strip_prefix("console-").unwrap_or(id);
    let mark = [id, bare].into_iter().filter(|i| *i != "synthetic").find_map(provider_icons::named);
    match mark {
        Some(m) => icon(m, 16.0).color(color),
        None => icon(&opencode_ui::provider_mark::CUSTOM, 16.0),
    }
}

/// `--v2-elevation-overlay` on a dialog surface.
pub(crate) fn overlay(p: &Pal, e: El) -> El {
    e.elevate(p.overlay)
}

/// The window a dialog clamps its size to, in zoomed points.
pub(crate) fn dialog_window<H: ConnectHost + 'static>(s: &H, cx: &Cx<H>) -> [f32; 2] {
    let zoom = s.zoom().max(0.5);
    cx.layout_size("zoom").unwrap_or([1280.0 / zoom, 820.0 / zoom])
}

/// How a dialog's height follows its content (`<Dialog fit>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// As tall as its content, up to the size's most (`fit`).
    Content,
}

/// `card` at preset `size` in a window `win`: its width, and the most it grows to.
pub(crate) fn sized(card: El, size: DialogSize, fit: Fit, win: [f32; 2]) -> El {
    let [win_w, win_h] = win;
    let card = card.w(size.width(win_w));
    match fit {
        Fit::Content => card.max_h(size.max_height(win_h, false)),
    }
}

/// `card` at preset `size` in a window `win`, over its scrim (`modal`): centred, or with its top
/// edge kept where it fits its content and is large.
pub(crate) fn sized_modal(key: &str, p: &Pal, size: DialogSize, fit: Fit, win: [f32; 2], card: El) -> Vec<El> {
    let kept = fit == Fit::Content && size != DialogSize::Normal;
    modal(key, p, sized(card, size, fit, win), kept.then(|| size.top(win[1])))
}

/// A dialog's scrim and its centring layer.
pub(crate) fn modal(key: &str, p: &Pal, dialog: El, top: Option<f32>) -> Vec<El> {
    let scrim = el()
        .key(format!("{key}-scrim"))
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.scrim)
        .transition(Transition::none());
    let mut layer = col()
        .key(format!("{key}-modal"))
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .items_center()
        .transition(Transition::none());
    layer = match top {
        Some(t) => layer.pt(t),
        None => layer.center(),
    };
    vec![scrim, layer.child(dialog).raise()]
}

/// How searches fold in the interface's language: Turkish and Azerbaijani fold `I` to `ı`.
pub(crate) fn search_fold() -> rune_kit::search::Fold {
    rune_kit::search::Fold::for_language(opencode_i18n::locale().tag())
}
