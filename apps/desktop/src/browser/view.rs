//! Browser panel content rendered inside `SideRegion`: navigation/URL bar (`⌘L`), `@eN` picker
//! toggle (`⇧⌘C`), zoom reset pill, first-use engine install progress, and `NativeSurface`.

use browser_pane::{ENGINE_DOWNLOAD_MB, EngineStatus};
use rune::{
    Anim, Color, Cursor, El, Icon, Spring, TextStyle, Tx, col, el, icon, row, spacer, text,
    text_input,
};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::odometer::odometer_end;
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
use crate::view::{UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const MONO_URL: TextStyle = TextStyle::new(12.0, 440.0).mono().lh(16.0);

static BACK_ICON: Icon = Icon {
    name: "browser-back",
    view: 16.0,
    body: r#"<path d="M10 3.5L5.5 8L10 12.5" stroke="currentColor" stroke-linecap="square"/>"#,
};
static RELOAD_ICON: Icon = Icon {
    name: "browser-reload",
    view: 16.0,
    body: r#"<path d="M13 8A5 5 0 1 1 11.6 4.5M13 2.8V5.6H10.2" stroke="currentColor" stroke-linecap="square"/>"#,
};
static PICKER_ICON: Icon = Icon {
    name: "browser-picker",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5" stroke="currentColor"/><circle cx="8" cy="8" r="1.5" fill="currentColor"/><path d="M8 1.5V3M8 13V14.5M1.5 8H3M13 8H14.5" stroke="currentColor"/>"#,
};

pub fn render_panel(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    let tab_id = ctx
        .tab
        .id
        .parse::<u64>()
        .ok()
        .or_else(|| s.browser.host.focused_tab_id());

    let status = s.browser.host.status();
    let content = col().key("browser-pane-content").w_full().grow(1.0).min_h(0.0);
    // Without an engine there is nothing to type an address for.
    let content = if status == EngineStatus::Unsupported {
        content
    } else {
        content.child(address_bar(s, p, tab_id)).child(el().w_full().h(0.5).fill(p.border_muted))
    };
    content.child(viewport(s, p, tab_id, status, ctx.env_scale))
}

fn address_bar(s: &App, p: &Pal, tab_id: Option<u64>) -> El {
    let tab = tab_id.and_then(|id| s.browser.host.tab(id));
    let can_back = tab.is_some_and(|t| t.can_go_back);
    let can_fwd = tab.is_some_and(|t| t.can_go_forward);
    let loading = tab.is_some_and(|t| t.loading);
    let has_url = tab.is_some_and(|t| !t.url.is_empty() && t.url != "about:blank");
    let can_reload = loading || has_url;
    let inspecting = tab.is_some_and(|t| t.inspecting);
    let zoom = tab.map_or(1.0, |t| t.zoom);

    let url_field = row()
        .key("browser-url-box")
        .grow(1.0)
        .shrink()
        .h(26.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .clip_rounded(6.0)
        .fill(p.bg_deep)
        .border(0.5, if s.browser.address.focused { p.border } else { p.border_muted })
        .motion(rune::Spring::NONE)
        .child(icon(&ic::GLOBE, 13.0).color(p.faint))
        .child(
            text_input(&s.browser.address, MONO_URL)
                .key("browser-address")
                .placeholder(opencode_i18n::t!("plugin.browser.address.search"), p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(|s: &mut App| &mut s.browser.address)
                .on_submit(|s: &mut App, _tx: &mut Tx| s.submit_browser_address()),
        );

    let mut back_btn = icon_btn(p, "browser-back", opencode_i18n::t!("ui.common.back"), &BACK_ICON, 15.0, false, can_back);
    if can_back {
        back_btn = back_btn.on_press(move |s: &mut App| {
            if let Some(id) = tab_id {
                s.browser.host.back(id);
            }
        });
    }

    let mut fwd_btn = icon_btn(p, "browser-forward", opencode_i18n::t!("desktop.menu.forward"), &ic::CHEVRON_RIGHT, 15.0, false, can_fwd);
    if can_fwd {
        fwd_btn = fwd_btn.on_press(move |s: &mut App| {
            if let Some(id) = tab_id {
                s.browser.host.forward(id);
            }
        });
    }

    let mut reload_btn = icon_btn(
        p,
        "browser-reload",
        &if loading { opencode_i18n::t!("plugin.browser.action.stop").to_owned() } else { crate::keymap::Bind::BrowserReload.tip(opencode_i18n::t!("plugin.browser.action.reload")) },
        if loading { &ic::XMARK_SMALL } else { &RELOAD_ICON },
        14.0,
        false,
        can_reload,
    );
    if can_reload {
        reload_btn = reload_btn.on_press(move |s: &mut App| {
            if let Some(id) = tab_id
                && let Some(t) = s.browser.host.tab(id)
            {
                if t.loading {
                    s.browser.host.stop(id);
                } else {
                    s.browser.host.reload(id);
                }
            }
        });
    }

    let mut bar = row()
        .key("browser-navbar")
        .w_full()
        .h(38.0)
        .px(8.0)
        .gap(4.0)
        .center_y()
        .child(back_btn)
        .child(fwd_btn)
        .child(reload_btn)
        .child(url_field);

    if (zoom - 1.0).abs() > 0.01 {
        bar = bar.child(
            el().key("browser-zoom-pill")
                .role("button")
                .label(opencode_i18n::t!("plugin.browser.zoom.reset"))
                .h(24.0)
                .px(6.0)
                .radius(5.0)
                .center()
                .fill(p.hover)
                .on_press(move |s: &mut App| {
                    if let Some(id) = tab_id {
                        s.browser.host.step_zoom(id, 0);
                    }
                })
                .child(
                    text(format!("{}%", (zoom * 100.0).round() as i32))
                        .font(UI_SMALL)
                        .color(p.muted),
                ),
        );
    }

    let mut inspect_btn = icon_btn(
        p,
        "browser-inspect",
        &crate::keymap::Bind::BrowserInspect.tip(opencode_i18n::t!("plugin.browser.inspect")),
        &PICKER_ICON,
        15.0,
        inspecting,
        has_url,
    );
    if has_url {
        inspect_btn = inspect_btn.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if let Some(id) = tab_id {
                s.browser.host.toggle_inspect(id);
            }
        });
    }
    bar = bar.child(inspect_btn);

    bar
}

fn viewport(s: &App, p: &Pal, tab_id: Option<u64>, status: EngineStatus, env_scale: f32) -> El {
    match status {
        EngineStatus::Unsupported => unsupported(p),
        // The background download, when the pane opens on it, shows here and goes on.
        EngineStatus::Downloading { downloaded_bytes, total_bytes } => {
            let frac = total_bytes
                .filter(|&t| t > 0)
                .map_or(0.25, |t| (downloaded_bytes as f32 / t as f32).clamp(0.03, 1.0));
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
            .child(
                text(opencode_i18n::t!("rune.browser.engine.needsDownload", size = ENGINE_DOWNLOAD_MB))
                    .font(UI_MED)
                    .color(p.text)
                    .truncate_fade(),
            )
            .child(
                el().key("browser-download-btn")
                    .role("button")
                    .label(opencode_i18n::t!("rune.browser.engine.download"))
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
                    .label(opencode_i18n::t!("rune.browser.engine.retryDownload"))
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
        EngineStatus::Installed | EngineStatus::Connecting | EngineStatus::Ready => {
            if let Some(id) = tab_id
                && let Some(tab) = s.browser.host.tab(id)
            {
                if (tab.url.is_empty() || tab.url == "about:blank") && !tab.loading {
                    return col()
                        .key("browser-blank")
                        .grow(1.0)
                        .min_h(0.0)
                        .w_full()
                        .center()
                        .padding(24.0)
                        .gap(8.0)
                        .child(icon(&ic::GLOBE, 20.0).color(p.icon_muted))
                        .child(text(opencode_i18n::t!("rune.browser.empty.title")).font(UI_MED).color(p.text))
                        .child(
                            text(opencode_i18n::t!("rune.browser.empty.keys", address = crate::keymap::Bind::BrowserAddress.label(), tab = crate::keymap::Bind::BrowserTab.label(), inspect = crate::keymap::Bind::BrowserInspect.label()))
                                .font(UI_SMALL)
                                .color(p.muted)
                                .truncate_fade(),
                        );
                }
                col()
                    .key("browser-viewport")
                    .grow(1.0)
                    .min_h(0.0)
                    .w_full()
                    .fill(p.bg_deep)
                    .child(s.browser.host.surface_el(
                        "browser-surface",
                        tab.id,
                        env_scale,
                        s.zoom,
                        |app: &mut App| &mut app.browser.host,
                    ))
            } else {
                col()
                    .key("browser-no-tabs")
                    .grow(1.0)
                    .w_full()
                    .center()
                    .child(spacer())
            }
        }
    }
}

/// The engine download's progress: a label, the megabytes so far (rolling) when known, and a bar
/// filled to `frac`.
fn progress(p: &Pal, label: &str, bytes: Option<(u64, Option<u64>)>, frac: f32) -> El {
    let bar_w = 240.0;
    let odometer = bytes.map(|(downloaded_bytes, total_bytes)| {
        let done_mb = (downloaded_bytes as f64 / 1_000_000.0).floor();
        let tot_mb = total_bytes
            .filter(|&t| t > 0)
            .map(|t| (t as f64 / 1_000_000.0).round().max(1.0));
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
        .child(
            row()
                .w(bar_w)
                .center_y()
                .child(
                    text(label.to_owned())
                        .font(UI_SMALL)
                        .color(p.muted)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .truncate_fade(),
                )
                .children(odometer),
        )
        .child(
            el().key("browser-dl-track")
                .w(bar_w)
                .h(5.0)
                .radius(2.5)
                .clip_rounded(2.5)
                .fill(p.layer3)
                .child(
                    el().key("browser-dl-fill")
                        .w(bar_w * frac)
                        .h(5.0)
                        .radius(2.5)
                        .fill(p.icon)
                        .motion(Spring::new(0.25, 0.0)),
                ),
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
        .child(
            text(opencode_i18n::t!("rune.browser.unsupported.description"))
                .font(UI_SMALL)
                .color(p.muted)
                .wrap()
                .align(rune::Align::Center)
                .max_w(320.0),
        )
}

fn icon_btn(
    p: &Pal,
    key: &str,
    label: &str,
    glyph: &'static Icon,
    size: f32,
    active: bool,
    enabled: bool,
) -> El {
    let mut b = el()
        .key(key)
        .role("button")
        .label(label)
        .size(26.0, 26.0)
        .radius(6.0)
        .center()
        .fill(if active { p.pressed } else { Color::TRANSPARENT })
        .child(icon(glyph, size).color(if active { p.text } else { p.icon_muted }));
    if enabled {
        b = b.hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT);
    } else {
        b = b.dim(0.35);
    }
    b
}
