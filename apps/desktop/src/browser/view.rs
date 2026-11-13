//! What `ui.browserSurface` draws: a page's frames, and until the engine runs, the engine's
//! state in their place (its first-use download, the download's progress, why it failed, or
//! that this build has none).

use browser_pane::{ENGINE_DOWNLOAD_MB, EngineStatus};
use rune::{Anim, Cursor, El, Spring, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::odometer::odometer_end;
use crate::palette::Pal;
use crate::view::{UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;

/// Page `tab_id`'s surface, `scale` device pixels a point, or the engine's state while it can't
/// show pages; `None` when the engine is ready and the page isn't open.
pub fn surface(s: &App, p: &Pal, tab_id: u64, scale: f32) -> Option<El> {
    Some(match s.browser.host.status() {
        EngineStatus::Unsupported => unsupported(p),
        // The background download, when the pane opens on it, shows here and goes on.
        EngineStatus::Downloading { downloaded_bytes, total_bytes } => {
            let frac = total_bytes.filter(|&t| t > 0).map_or(0.25, |t| (downloaded_bytes as f32 / t as f32).clamp(0.03, 1.0));
            progress(p, opencode_i18n::t!("rune.browser.engine.downloading"), Some((downloaded_bytes, total_bytes)), frac)
        }
        EngineStatus::Verifying => progress(p, opencode_i18n::t!("rune.browser.engine.verifying"), None, 1.0),
        EngineStatus::Paused(reason) => progress(p, reason.describe(), None, 0.25),
        EngineStatus::NotInstalled => col()
            .key("browser-download-prompt")
            .grow(1.0)
            .w_full()
            .center()
            .padding(28.0)
            .gap(14.0)
            .child(icon(&ic::GLOBE, 22.0).color(p.icon_muted))
            .child(text(opencode_i18n::t!("rune.browser.engine.needsDownload", size = ENGINE_DOWNLOAD_MB)).font(UI_MED).color(p.text).truncate_fade())
            .child(
                el().key("browser-download-btn")
                    .role("button")
                    .label_static(opencode_i18n::t!("rune.browser.engine.download"))
                    .h(30.0)
                    .px(14.0)
                    .radius(6.0)
                    .center()
                    .cursor(Cursor::Pointer)
                    .fill(p.q.submit)
                    .box_shadow(Pal::ring_out(p.q.submit_ring))
                    .hover_motion(INSTANT, INSTANT)
                    .on_press(|s: &mut App| s.start_browser_install())
                    .child(text(opencode_i18n::t!("plugin.file.view.download")).font(UI_MED).color(p.q.submit_text)),
            ),
        EngineStatus::Failed(msg) => col()
            .key("browser-download-failed")
            .grow(1.0)
            .w_full()
            .center()
            .padding(28.0)
            .gap(14.0)
            .child(icon(&ic::WARNING, 20.0).color(p.icon_muted))
            .child(text(msg.message).font(UI_SMALL).color(p.muted).wrap().align(rune::Align::Center).max_w(320.0))
            .child(
                el().key("browser-retry-btn")
                    .role("button")
                    .label_static(opencode_i18n::t!("rune.browser.engine.retryDownload"))
                    .h(30.0)
                    .px(14.0)
                    .radius(6.0)
                    .center()
                    .cursor(Cursor::Pointer)
                    .fill(p.q.neutral)
                    .box_shadow(Pal::ring_out(p.q.neutral_ring))
                    .hover(p.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press(|s: &mut App| s.start_browser_install())
                    .child(text(opencode_i18n::t!("rune.home.noServer.retryShort")).font(UI_MED).color(p.text)),
            ),
        EngineStatus::Unchecked | EngineStatus::Installed | EngineStatus::Connecting | EngineStatus::Ready => {
            let tab = s.browser.host.tab(tab_id)?;
            // `embeds.View`: the page on the pane's base background, 10 pt corners where it meets
            // the pane's edge.
            col()
                .key("browser-viewport")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .fill(p.bg_base)
                // The page lays out in window points at the window's scale, whatever the app's zoom:
                // `scale` is the view's, the window's times the zoom.
                .child(s.browser.host.surface_el("browser-surface", tab.id, scale / *s.zoom, *s.zoom, |app: &mut App| &mut app.browser.host, App::focus_browser_page))
        }
    })
}

/// The engine download's progress: a label, the megabytes so far (rolling) when known, and a bar
/// filled to `frac`.
fn progress(p: &Pal, label: &str, bytes: Option<(u64, Option<u64>)>, frac: f32) -> El {
    let bar_w = 240.0;
    let odometer = bytes.map(|(downloaded_bytes, total_bytes)| {
        let done_mb = (downloaded_bytes as f64 / 1_000_000.0).floor();
        let tot_mb = total_bytes.filter(|&t| t > 0).map(|t| (t as f64 / 1_000_000.0).round().max(1.0));
        let odo_label = match tot_mb {
            Some(tot) => format!("{done_mb:.0} / {tot:.0} MB"),
            None => format!("{done_mb:.0} MB"),
        };
        odometer_end("browser-dl-mb", odo_label, done_mb, UI_MED, p.text)
    });
    col()
        .key("browser-installing")
        .grow(1.0)
        .w_full()
        .center()
        .padding(28.0)
        .gap(10.0)
        .child(row().w(bar_w).center_y().child(text(label.to_owned()).font(UI_SMALL).color(p.muted).grow(1.0).shrink().min_w(0.0).truncate_fade()).children(odometer))
        .child(
            el().key("browser-dl-track")
                .w(bar_w)
                .h(5.0)
                .radius(2.5)
                .clip_rounded(2.5)
                .fill(p.layer3)
                .child(el().key("browser-dl-fill").w(bar_w * frac).h(5.0).radius(2.5).fill(p.icon).motion(Spring::new(0.25, 0.0))),
        )
}

/// This build has no browser engine for its platform: no address bar, nothing to download.
fn unsupported(p: &Pal) -> El {
    col()
        .key("browser-unsupported")
        .grow(1.0)
        .w_full()
        .center()
        .padding(28.0)
        .gap(8.0)
        .child(icon(&ic::GLOBE, 22.0).color(p.icon_muted))
        .child(text(opencode_i18n::t!("rune.browser.unsupported.title")).font(UI_MED).color(p.text).truncate_fade())
        .child(text(opencode_i18n::t!("rune.browser.unsupported.description")).font(UI_SMALL).color(p.muted).wrap().align(rune::Align::Center).max_w(320.0))
}
