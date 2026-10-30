//! Browser panel content rendered inside `SideRegion`: navigation/URL bar (`⌘L`), `@eN` picker
//! toggle (`⇧⌘C`), zoom reset pill, first-use engine install progress, and `NativeSurface`.

use browser_pane::EngineStatus;
use rune::{
    Anim, Color, El, Icon, TextStyle, Tx, col, el, icon, row, spacer, text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::panels::PanelViewCtx;
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

pub fn render_panel(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    let tab_id = ctx
        .tab
        .id
        .parse::<u64>()
        .ok()
        .or_else(|| s.browser.host.focused_tab_id());

    col()
        .key("browser-pane-content")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(address_bar(s, p, tab_id))
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(viewport(s, p, tab_id, ctx.env_scale))
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
        .child(icon(&ic::GLOBE, 13.0).color(p.faint))
        .child(
            text_input(&s.browser.address, MONO_URL)
                .key("browser-address")
                .placeholder("Enter URL, :port, or search… (⌘L)", p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(|s: &mut App| &mut s.browser.address)
                .on_submit(|s: &mut App, _tx: &mut Tx| s.submit_browser_address()),
        );

    let mut back_btn = icon_btn(p, "browser-back", "Back", &BACK_ICON, 15.0, false, can_back);
    if can_back {
        back_btn = back_btn.on_press(move |s: &mut App| {
            if let Some(id) = tab_id {
                s.browser.host.back(id);
            }
        });
    }

    let mut fwd_btn = icon_btn(p, "browser-forward", "Forward", &ic::CHEVRON_RIGHT, 15.0, false, can_fwd);
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
        if loading { "Stop loading" } else { "Reload page (⌘R)" },
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
                .label("Reset page zoom")
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
        "Pick page element for prompt (@e1, ⇧⌘C)",
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

fn viewport(s: &App, p: &Pal, tab_id: Option<u64>, env_scale: f32) -> El {
    let status = s.browser.host.status();
    match status {
        EngineStatus::Downloading { downloaded_bytes, total_bytes } => {
            let mb = downloaded_bytes as f64 / (1024.0 * 1024.0);
            let label = match total_bytes {
                Some(tot) if tot > 0 => format!(
                    "Installing Browser Engine · {mb:.1} / {:.1} MB",
                    tot as f64 / (1024.0 * 1024.0)
                ),
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
                .child(text(label).font(UI_MED).color(p.text).truncate_fade())
                .child(
                    el().w(220.0)
                        .h(5.0)
                        .radius(2.5)
                        .fill(p.layer3)
                        .child(el().w(220.0 * frac).h(5.0).radius(2.5).fill(p.icon)),
                )
                .child(
                    text("Downloaded once into Application Support and shared across updates.")
                        .font(UI_SMALL)
                        .color(p.faint)
                        .truncate_fade(),
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
                .child(text(detail).font(UI_SMALL).color(p.muted).truncate_fade())
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
                        .child(text("Enter a URL or :port above").font(UI_MED).color(p.text))
                        .child(
                            text("Press ⌘L to jump to the address bar, or ⇧⌘C to pick an element.")
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
                        env_scale * s.zoom,
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
