//! Right-hand Browser split panel view: tab strip, URL bar, `@eN` picker toggle, zoom,
//! first-use install progress card, and zero-copy `NativeSurface`.

use browser_pane::EngineStatus;
use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Drag, Easing, El, Icon, Phase, TextStyle, Tx, col, el,
    icon, row, spacer, text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::browser::{BROWSER_MAX_W, BROWSER_MIN_W};
use crate::palette::Pal;
use crate::view::{UI, UI_MED, UI_SMALL};

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

pub fn aside(s: &App, p: &Pal) -> Option<El> {
    if !s.browser.open {
        return None;
    }
    Some(
        panel_box(s, p, s.browser.width)
            .key("browser-panel")
            .mt(8.0)
            .mr(8.0)
            .mb(8.0),
    )
}

pub fn resize_handle(s: &App, p: &Pal, window_w: f32) -> Option<El> {
    if !s.browser.open || window_w <= 0.0 {
        return None;
    }
    let x = (window_w - s.browser.width - 12.0).max(0.0);
    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let dragging = s.browser.drag_origin.is_some();

    let pill = el()
        .key("browser-resize-pill")
        .w(3.0)
        .h(36.0)
        .radius(1.5)
        .fill(p.ring)
        .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0);

    Some(
        el().key("browser-resize")
            .absolute(x, 0.0)
            .w(8.0)
            .h_full()
            .center()
            .cursor(Cursor::ResizeColumn)
            .hover_group()
            .hover_motion(enter, exit)
            .on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                match d.phase {
                    Phase::Down => s.browser.drag_origin = Some(s.browser.width),
                    Phase::Move => {
                        if let Some(orig) = s.browser.drag_origin {
                            s.browser.width =
                                (orig - d.dx / s.zoom).clamp(BROWSER_MIN_W, BROWSER_MAX_W);
                        }
                    }
                    Phase::Up => {
                        if d.cancelled()
                            && let Some(orig) = s.browser.drag_origin
                        {
                            s.browser.width = orig;
                        }
                        s.browser.drag_origin = None;
                    }
                }
            })
            .child(pill),
    )
}

pub fn panel_card(s: &App, p: &Pal, width: f32, card_h: f32) -> El {
    panel_box(s, p, width).h(card_h)
}

fn panel_box(s: &App, p: &Pal, width: f32) -> El {
    col()
        .w(width)
        .radius(10.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(2.0, 4.0, p.shadow)
        .shadow(1.0, 2.0, p.shadow)
        .clip_soft(0.0)
        .child(tab_strip(s, p))
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(address_bar(s, p))
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(viewport(s, p))
}

fn tab_strip(s: &App, p: &Pal) -> El {
    let focused_id = s.browser.host.focused_tab_id();
    let inspecting = s.browser.host.focused_tab().is_some_and(|t| t.inspecting);

    let mut tabs_row = row().key("browser-tabs").gap(4.0).center_y().grow(1.0).min_w(0.0);
    for tab in s.browser.host.tabs() {
        let id = tab.id;
        let close_id = tab.id;
        let active = focused_id == Some(id);
        let label = if tab.title.is_empty() || tab.title == "New tab" {
            if tab.url == "about:blank" || tab.url.is_empty() {
                "New tab".into()
            } else {
                tab.url.trim_start_matches("https://").trim_start_matches("http://").to_owned()
            }
        } else {
            tab.title.clone()
        };

        let pill = row()
            .key(format!("btab-{id}"))
            .role("button")
            .label(label.clone())
            .h(26.0)
            .max_w(168.0)
            .pl(8.0)
            .pr(4.0)
            .gap(6.0)
            .center_y()
            .radius(6.0)
            .fill(if active { p.pressed } else { Color::TRANSPARENT })
            .hover(p.hover)
            .hover_motion(INSTANT, INSTANT)
            .on_press(move |s: &mut App| {
                s.browser.host.focus_tab(id);
                s.sync_browser_address(true);
            })
            .child(
                text(label)
                    .font(if active { UI_MED } else { UI_SMALL })
                    .color(if active { p.text } else { p.muted })
                    .truncate()
                    .shrink(),
            )
            .child(
                el().key(format!("btab-close-{id}"))
                    .role("button")
                    .label("Close browser tab")
                    .size(18.0, 18.0)
                    .radius(4.0)
                    .center()
                    .hover(p.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press(move |s: &mut App| {
                        s.browser.host.close_tab(close_id);
                        if s.browser.host.tabs().is_empty() {
                            s.browser.open = false;
                        } else {
                            s.sync_browser_address(true);
                        }
                    })
                    .child(icon(&ic::XMARK_SMALL, 12.0).color(p.faint)),
            );
        tabs_row = tabs_row.child(pill);
    }

    tabs_row = tabs_row.child(
        icon_btn(p, "browser-new-tab", "New browser tab", &ic::PLUS, 14.0, false).on_press(
            |s: &mut App| {
                s.open_new_browser_tab("about:blank");
                s.browser.address.focused = true;
            },
        ),
    );

    row()
        .key("browser-tabbar")
        .w_full()
        .h(38.0)
        .pl(8.0)
        .pr(8.0)
        .gap(4.0)
        .center_y()
        .child(tabs_row)
        .child(
            icon_btn(
                p,
                "browser-inspect",
                "Pick page element for prompt (@e1)",
                &PICKER_ICON,
                15.0,
                inspecting,
            )
            .on_press(|s: &mut App| {
                if let Some(id) = s.browser.host.focused_tab_id() {
                    s.browser.host.toggle_inspect(id);
                }
            }),
        )
        .child(
            icon_btn(p, "browser-close", "Hide browser pane (⌥⌘B)", &ic::CLOSE, 15.0, false)
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.toggle_browser(tx);
                }),
        )
}

fn address_bar(s: &App, p: &Pal) -> El {
    let tab = s.browser.host.focused_tab();
    let can_back = tab.is_some_and(|t| t.can_go_back);
    let can_fwd = tab.is_some_and(|t| t.can_go_forward);
    let loading = tab.is_some_and(|t| t.loading);
    let zoom = tab.map_or(1.0, |t| t.zoom);

    let url_field = row()
        .key("browser-url-box")
        .grow(1.0)
        .min_w(0.0)
        .h(26.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_deep)
        .border(0.5, if s.browser.address.focused { p.border } else { p.border_muted })
        .child(icon(&ic::GLOBE, 13.0).color(p.faint))
        .child(
            text_input(&s.browser.address, MONO_URL)
                .key("browser-address")
                .placeholder("Enter URL, :port, or search… (⌘L)", p.faint)
                .color(p.text)
                .grow(1.0)
                .min_w(0.0)
                .on_edit(|s: &mut App| &mut s.browser.address)
                .on_submit(|s: &mut App, _tx: &mut Tx| s.submit_browser_address()),
        );

    let mut bar = row()
        .key("browser-navbar")
        .w_full()
        .h(38.0)
        .px(8.0)
        .gap(4.0)
        .center_y()
        .child(
            icon_btn(p, "browser-back", "Back", &BACK_ICON, 15.0, false)
                .opacity(if can_back { 1.0 } else { 0.35 })
                .on_press(|s: &mut App| {
                    if let Some(id) = s.browser.host.focused_tab_id() {
                        s.browser.host.back(id);
                    }
                }),
        )
        .child(
            icon_btn(p, "browser-forward", "Forward", &ic::CHEVRON_RIGHT, 15.0, false)
                .opacity(if can_fwd { 1.0 } else { 0.35 })
                .on_press(|s: &mut App| {
                    if let Some(id) = s.browser.host.focused_tab_id() {
                        s.browser.host.forward(id);
                    }
                }),
        )
        .child(
            icon_btn(
                p,
                "browser-reload",
                if loading { "Stop loading" } else { "Reload page" },
                if loading { &ic::XMARK_SMALL } else { &RELOAD_ICON },
                14.0,
                false,
            )
            .on_press(|s: &mut App| {
                if let Some(t) = s.browser.host.focused_tab() {
                    let id = t.id;
                    if t.loading {
                        s.browser.host.stop(id);
                    } else {
                        s.browser.host.reload(id);
                    }
                }
            }),
        )
        .child(url_field);

    if (zoom - 1.0).abs() > 0.01 {
        bar = bar.child(
            el().key("browser-zoom-pill")
                .role("button")
                .label("Reset page zoom")
                .h(24.0)
                .px(6.0)
                .radius(5.0)
                .center()
                .fill(p.hover)
                .on_press(|s: &mut App| {
                    if let Some(id) = s.browser.host.focused_tab_id() {
                        s.browser.host.step_zoom(id, 0);
                    }
                })
                .child(text(format!("{}%", (zoom * 100.0).round() as i32)).font(UI_SMALL).color(p.muted)),
        );
    }

    bar
}

fn viewport(s: &App, p: &Pal) -> El {
    let status = s.browser.host.status();
    match status {
        EngineStatus::Downloading { downloaded_bytes, total_bytes } => {
            let mb = downloaded_bytes as f64 / (1024.0 * 1024.0);
            let label = match total_bytes {
                Some(tot) if tot > 0 => {
                    format!("Installing Browser Engine · {mb:.1} / {:.1} MB", tot as f64 / (1024.0 * 1024.0))
                }
                _ => format!("Installing Browser Engine · {mb:.1} MB"),
            };
            let frac = total_bytes
                .filter(|&t| t > 0)
                .map_or(0.35, |t| (downloaded_bytes as f32 / t as f32).clamp(0.05, 1.0));
            col()
                .key("browser-installing")
                .grow(1.0)
                .w_full()
                .center()
                .padding(24.0)
                .gap(10.0)
                .child(text(label).font(UI_MED).color(p.text))
                .child(
                    el().w(240.0)
                        .h(5.0)
                        .radius(2.5)
                        .fill(p.layer3)
                        .child(el().w(240.0 * frac).h(5.0).radius(2.5).fill(p.icon)),
                )
                .child(
                    text("Downloaded once into Application Support and shared across updates.")
                        .font(UI_SMALL)
                        .color(p.faint),
                )
        }
        EngineStatus::NotInstalled | EngineStatus::Failed(_) => {
            let detail = match status {
                EngineStatus::Failed(msg) => msg,
                _ => "Chromium 156 runs as an on-demand sidecar so OpenCode (Rune) stays 8 MB.".into(),
            };
            col()
                .key("browser-empty")
                .grow(1.0)
                .w_full()
                .center()
                .padding(28.0)
                .gap(12.0)
                .child(icon(&ic::GLOBE, 22.0).color(p.muted))
                .child(text("Browser engine not installed").font(UI_MED).color(p.text))
                .child(text(detail).font(UI_SMALL).color(p.muted))
                .child(
                    el().key("browser-install-btn")
                        .role("button")
                        .label("Install or connect browser engine")
                        .h(30.0)
                        .px(14.0)
                        .radius(6.0)
                        .center()
                        .fill(p.pressed)
                        .hover(p.hover)
                        .hover_motion(INSTANT, INSTANT)
                        .on_press(|s: &mut App| s.browser.host.retry_install())
                        .child(text("Connect Engine").font(UI).color(p.text)),
                )
        }
        EngineStatus::Connecting | EngineStatus::Ready => {
            if let Some(tab) = s.browser.host.focused_tab() {
                col()
                    .key("browser-viewport")
                    .grow(1.0)
                    .min_h(0.0)
                    .w_full()
                    .fill(p.bg_deep)
                    .child(s.browser.host.surface_el(
                        "browser-surface",
                        tab.id,
                        2.0 * s.zoom,
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

fn icon_btn(
    p: &Pal,
    key: &str,
    label: &str,
    glyph: &'static Icon,
    size: f32,
    active: bool,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .size(26.0, 26.0)
        .radius(6.0)
        .center()
        .fill(if active { p.pressed } else { Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, size).color(if active { p.text } else { p.icon_muted }))
}
