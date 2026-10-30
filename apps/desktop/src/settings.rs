//! A basic Settings page (⌘, or ⌘K "Settings"): Desktop's settings rows (`settings.css`: a
//! 720 px column, a raised list of rows with a 13/530 title and a muted description, and the
//! 24 × 16 switch), with only the preferences this app has. They live in
//! `<data dir>/rune-opencode/settings.json`, next to `ui.json`.

use std::path::PathBuf;

use rune::{Color, Cx, El, Key, KeyEvent, TextStyle, Tx, col, el, row, spacer, text};
use serde_json::{Value, json};

use crate::palette::Pal;
use crate::view::{UI, UI_MED};
use crate::App;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    System,
    Light,
    Dark,
}

impl Scheme {
    const ALL: [Scheme; 3] = [Scheme::System, Scheme::Light, Scheme::Dark];

    fn name(self) -> &'static str {
        match self { Self::System => "system", Self::Light => "light", Self::Dark => "dark" }
    }

    fn label(self) -> &'static str {
        match self { Self::System => "System", Self::Light => "Light", Self::Dark => "Dark" }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub open: bool,
    /// The composer's agent picker; the agent still applies when it's hidden.
    pub show_agent: bool,
    pub scheme: Scheme,
    /// Fold prompts past 12 lines behind Read more (`fold.rs`). On unless turned off.
    pub fold_user: bool,
    /// Where `PanelRegion::Dock` (Terminal) renders: `"side"` (default) or `"bottom"`.
    pub terminal_placement: crate::panels::DockPlacement,
    /// Which events post a system notification (`notifications.rs`), with Desktop's defaults.
    pub notify: crate::notifications::Kinds,
    /// Answer every permission request "once" as it arrives (Desktop's `permissions.autoApprove`).
    pub auto_accept: bool,
    /// Desktop's colour theme id (`themes.rs`); `oc-2` is OpenCode's own.
    pub theme: String,
    pub reduce_motion: crate::appearance::ReduceMotion,
    /// The terminal's font, an index into the Terminal font knob's choices.
    pub terminal_font: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            open: false,
            show_agent: false,
            scheme: Scheme::default(),
            fold_user: true,
            terminal_placement: crate::panels::DockPlacement::Side,
            notify: Default::default(),
            auto_accept: false,
            theme: crate::themes::DEFAULT.into(),
            reduce_motion: Default::default(),
            terminal_font: 0,
        }
    }
}

impl Settings {
    pub fn to_json(&self) -> Value {
        json!({
            "showAgentPicker": self.show_agent,
            "appearance": self.scheme.name(),
            "foldUserMessages": self.fold_user,
            "terminalPlacement": self.terminal_placement.name(),
            "notifications": self.notify.to_json(),
            "permissions": { "autoApprove": self.auto_accept },
            "theme": self.theme,
            "reduceMotion": self.reduce_motion.name(),
            "terminalFont": self.terminal_font,
        })
    }

    pub fn from_json(v: &Value) -> Settings {
        Settings {
            open: false,
            show_agent: v["showAgentPicker"].as_bool().unwrap_or(false),
            scheme: Scheme::ALL.into_iter().find(|s| Some(s.name()) == v["appearance"].as_str()).unwrap_or_default(),
            fold_user: v["foldUserMessages"].as_bool().unwrap_or(true),
            terminal_placement: crate::panels::DockPlacement::parse(v["terminalPlacement"].as_str()),
            notify: crate::notifications::Kinds::from_json(&v["notifications"]),
            auto_accept: v["permissions"]["autoApprove"].as_bool().unwrap_or(false),
            theme: crate::themes::all()[crate::themes::index(v["theme"].as_str().unwrap_or_default())].id.clone(),
            reduce_motion: crate::appearance::ReduceMotion::parse(v["reduceMotion"].as_str()),
            terminal_font: v["terminalFont"].as_u64().map_or(0, |i| (i as usize).min(crate::appearance::TERMINAL_FONTS.len() - 1)),
        }
    }
}

fn file() -> Option<PathBuf> {
    crate::restore::data_dir().map(|d| d.join("settings.json"))
}

pub fn load() -> Settings {
    file().map_or_else(Settings::default, |p| read(&p))
}

fn read(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).map_or_else(Settings::default, |v| Settings::from_json(&v))
}

fn save(settings: &Settings) {
    if let Some(path) = file() {
        let json = settings.to_json().to_string();
        crate::disk::later("settings", move || {
            crate::disk::replace(&path, json.as_bytes());
        });
    }
}

#[cfg(test)]
fn write(path: &std::path::Path, settings: &Settings) {
    crate::disk::replace(path, settings.to_json().to_string().as_bytes());
}

impl App {
    pub fn dark(&self, cx: &Cx<App>) -> bool {
        match self.settings.scheme {
            Scheme::System => cx.system_dark(),
            Scheme::Light => false,
            Scheme::Dark => true,
        }
    }

    pub fn open_settings(&mut self) {
        self.close_picker(false);
        self.close_switcher(false);
        self.composer.focused = false;
        self.settings.open = true;
    }

    pub fn close_settings(&mut self) {
        self.settings.open = false;
    }

    pub(crate) fn change_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings);
        if self.persists() {
            save(&self.settings);
        }
    }

    pub fn set_show_agent(&mut self, on: bool) {
        self.change_settings(|s| s.show_agent = on);
        self.pickers.toolbar_focus = self.pickers.toolbar_focus.filter(|k| on || *k != crate::pickers::Kind::Agent);
    }

    pub fn set_fold_user(&mut self, on: bool) {
        self.change_settings(|s| s.fold_user = on);
    }

    pub fn set_notify(&mut self, kind: crate::notifications::Kind, on: bool) {
        self.change_settings(|s| s.notify.set(kind, on));
    }

    pub fn set_scheme(&mut self, scheme: Scheme) {
        self.change_settings(|s| s.scheme = scheme);
    }

    pub fn set_terminal_placement(&mut self, placement: crate::panels::DockPlacement, tx: &mut Tx) {
        tx.instant();
        self.change_settings(|s| s.terminal_placement = placement);
    }

    /// Esc closes the page.
    pub fn settings_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.settings.open && ev.key == Key::Escape {
            tx.instant();
            self.close_settings();
            return true;
        }
        false
    }

    pub fn toggle_settings(&mut self, tx: &mut Tx) {
        tx.instant();
        if self.settings.open { self.close_settings() } else { self.open_settings() }
    }

    pub fn zoom_by(&mut self, dir: i32, tx: &mut Tx) {
        tx.instant();
        self.set_zoom(dir);
    }
}

const TITLE: TextStyle = TextStyle::new(15.0, 640.0).lh(24.0);
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const SECTION: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);

pub fn page(s: &App, p: &Pal) -> El {
    let general = settings_card(p, "settings-list", [
        ("agent-picker", "Show agent picker", "Choose the agent from the composer. Hidden, the last agent still applies.",
            switch(p, "show-agent", s.settings.show_agent).on_press(|s: &mut App| s.set_show_agent(!s.settings.show_agent))),
        ("fold-user", "Fold long messages", "Fold your messages past 12 lines behind a Read more button",
            switch(p, "fold-user-switch", s.settings.fold_user).on_press(|s: &mut App| s.set_fold_user(!s.settings.fold_user))),
        ("auto-accept", "Auto-accept permissions", "Permission requests will be automatically approved",
            switch(p, "auto-accept-switch", s.settings.auto_accept).on_press(|s: &mut App| s.set_auto_accept(!s.settings.auto_accept))),
        ("terminal-placement", "Terminal placement", "Choose where the terminal opens in sessions.", placement_control(s, p)),
    ]);
    let section = |title: &'static str, list: El| col().w_full().gap(16.0).child(text(title).font(SECTION).color(p.text)).child(list);
    col()
        .key("settings")
        .role("region")
        .label("Settings")
        .w_full()
        .grow(1.0)
        .shrink()
        .items_center()
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(
            col()
                .w_full()
                .max_w(720.0)
                .px(40.0)
                .pt(48.0)
                .pb(48.0)
                .gap(32.0)
                .child(text("Settings").font(TITLE).color(p.text))
                .child(section("General", general))
                .child(section("Appearance", settings_card(p, "appearance-list", crate::appearance::rows(s, p))))
                .child(section("Notifications", notification_list(s, p))),
        )
}

/// Desktop's three notification switches, with its titles and descriptions.
fn notification_list(s: &App, p: &Pal) -> El {
    use crate::notifications::Kind;
    let rows = [
        (Kind::Agent, "Agent", "Show system notification when the agent is complete or needs attention"),
        (Kind::Permission, "Permissions", "Show system notification when a permission is required"),
        (Kind::Error, "Errors", "Show system notification when an error occurs"),
    ];
    settings_card(p, "notifications-list", rows.map(|(kind, title, description)| {
        let key = format!("notify-{}", kind.name());
        let control = switch(p, &key, s.settings.notify.get(kind)).on_press(move |s: &mut App| s.set_notify(kind, !s.settings.notify.get(kind)));
        (key, title, description, control)
    }))
}

pub(crate) fn settings_card<'a, K: AsRef<str>>(p: &Pal, key: &str, items: impl IntoIterator<Item = (K, &'a str, &'a str, El)>) -> El {
    let items: Vec<_> = items.into_iter().collect();
    let n = items.len();
    let mut list = col().key(key).w_full().px(20.0).radius(8.0).fill(p.layer1).border(0.5, p.border_muted);
    for (i, (k, title, description, control)) in items.into_iter().enumerate() {
        list = list.child(setting(p, k.as_ref(), title, description, control, i + 1 < n));
    }
    list
}

fn setting(p: &Pal, key: &str, title: &str, description: &str, control: El, divider: bool) -> El {
    let r = row()
        .key(format!("setting-{key}"))
        .w_full()
        .gap(16.0)
        .py(20.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .shrink()
                .gap(8.0)
                .child(text(title.to_owned()).font(UI_MED).color(p.text))
                .child(text(description.to_owned()).font(BODY).color(p.muted)),
        )
        .child(control);
    col().key(format!("setting-{key}-block")).w_full().child(r).children(divider.then(|| el().w_full().h(0.5).fill(p.border)))
}

/// Desktop's switch: 24 × 16, the thumb 12 × 12, accent when on. It flips in the press's frame.
fn switch(p: &Pal, key: &str, on: bool) -> El {
    let thumb = el().size(12.0, 12.0).radius(2.0).fill(p.switch_thumb);
    row()
        .key(key)
        .role("switch")
        .label(if on { "On" } else { "Off" })
        .w(24.0)
        .h(16.0)
        .px(2.0)
        .center_y()
        .radius(4.0)
        .fill(if on { p.bg_accent } else { p.q.layer4 })
        .child(if on { row().w_full().child(spacer()).child(thumb) } else { row().w_full().child(thumb) })
}

fn placement_control(s: &App, p: &Pal) -> El {
    use crate::panels::DockPlacement;
    let mut r = row().key("terminal-placement-control").gap(2.0).padding(2.0).radius(6.0).fill(p.layer2);
    for (pl, label) in [(DockPlacement::Side, "Side"), (DockPlacement::Bottom, "Bottom")] {
        let on = s.settings.terminal_placement == pl;
        r = r.child(
            row()
                .key(format!("placement-{}", pl.name()))
                .role("button")
                .h(24.0)
                .px(10.0)
                .center()
                .radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.set_terminal_placement(pl, tx))
                .child(text(label).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}

pub(crate) fn scheme_control(s: &App, p: &Pal) -> El {
    let mut r = row().key("scheme").gap(2.0).padding(2.0).radius(6.0).fill(p.layer2);
    for scheme in Scheme::ALL {
        let on = s.settings.scheme == scheme;
        r = r.child(
            row()
                .key(format!("scheme-{}", scheme.name()))
                .role("button")
                .h(24.0)
                .px(10.0)
                .center()
                .radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press(move |s: &mut App| s.set_scheme(scheme))
                .child(text(scheme.label()).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}

pub(crate) fn zoom_control(s: &App, p: &Pal) -> El {
    let step = |key: &str, label: &str, dir: i32| {
        row()
            .key(key.to_owned())
            .role("button")
            .label(if dir < 0 { "Zoom out" } else { "Zoom in" })
            .size(24.0, 24.0)
            .center()
            .radius(4.0)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.zoom_by(dir, tx))
            .child(text(label.to_owned()).font(UI).color(p.muted))
    };
    row()
        .key("zoom-control")
        .gap(2.0)
        .padding(2.0)
        .center_y()
        .radius(6.0)
        .fill(p.layer2)
        .child(step("zoom-out", "−", -1))
        .child(row().key("zoom-value").w(48.0).center().child(text(format!("{}%", (s.zoom * 100.0).round() as i32)).font(UI).color(p.text)))
        .child(step("zoom-in", "+", 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture_runtime as runtime;
    use rune::{Mods, Runtime};

    fn has_agent(rt: &Runtime<App>) -> bool {
        rt.elements().iter().any(|e| e.label.as_deref() == Some("Choose agent"))
    }

    fn key(rt: &mut Runtime<App>, key: Key, cmd: bool) {
        rt.key(KeyEvent { key, mods: Mods { cmd, ..Mods::default() } });
        rt.frame(1.0 / 60.0);
    }

    fn tap(rt: &mut Runtime<App>, target: &str) {
        let (x, y) = rt.find(target).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn effort_shows_without_a_chosen_variant() {
        let mut rt = runtime();
        rt.state.pickers.open = Some(crate::pickers::Kind::Effort);
        rt.state.choose_picker("default");
        assert!(rt.state.selection().model.variant.is_empty());
        rt.frame(1.0 / 60.0);
        assert!(rt.state.has_effort());
        let (id, provider) = { let m = rt.state.selection().model; (m.id, m.provider) };
        assert!(rt.state.pickers.models.iter().any(|m| m.id == id && m.provider == provider && !m.variants.is_empty()));
        assert!(rt.find("effort").is_some());
    }

    #[test]
    fn live_models_read_variant_objects() {
        let mut p = crate::pickers::Pickers::default();
        let models = json!([{ "id": "m", "providerID": "z", "name": "M", "variants": [{ "id": "low" }, { "id": "high" }], "limit": { "context": 9 } }]);
        p.load_live(&models, &json!([{ "id": "build" }, { "id": "explore", "mode": "subagent" }]));
        assert_eq!(p.models[0].variants, ["low", "high"]);
        assert_eq!(p.agents, ["build"]);
    }

    #[test]
    fn agent_picker_is_hidden_until_the_setting_is_on() {
        let mut rt = runtime();
        assert!(!has_agent(&rt));
        assert_eq!(rt.state.selection().agent, "build");
        rt.state.set_show_agent(true);
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(has_agent(&rt));
    }

    #[test]
    fn command_comma_opens_settings_and_escape_closes_it() {
        let mut rt = runtime();
        key(&mut rt, Key::Command(','), true);
        assert!(rt.state.settings.open);
        assert!(rt.find("show-agent").is_some());
        tap(&mut rt, "show-agent");
        assert!(rt.state.settings.show_agent);
        tap(&mut rt, "scheme-dark");
        assert_eq!(rt.state.settings.scheme, Scheme::Dark);
        key(&mut rt, Key::Escape, false);
        assert!(!rt.state.settings.open);
        assert!(has_agent(&rt));
    }

    #[test]
    fn settings_persist() {
        let dir = std::env::temp_dir().join(format!("rune-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(read(&path), Settings::default());
        let mut on = Settings { open: false, show_agent: true, scheme: Scheme::Light, fold_user: false, terminal_placement: crate::panels::DockPlacement::Bottom, notify: Default::default(), auto_accept: true, theme: "tokyonight".into(), reduce_motion: crate::appearance::ReduceMotion::On, terminal_font: 2 };
        on.notify.set(crate::notifications::Kind::Agent, true);
        on.notify.set(crate::notifications::Kind::Permission, false);
        write(&path, &on);
        assert_eq!(read(&path), on);
        let _ = std::fs::remove_dir_all(dir);
    }
}
