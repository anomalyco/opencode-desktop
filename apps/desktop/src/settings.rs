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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { open: false, show_agent: false, scheme: Scheme::default(), fold_user: true }
    }
}

impl Settings {
    pub fn to_json(&self) -> Value {
        json!({ "showAgentPicker": self.show_agent, "appearance": self.scheme.name(), "foldUserMessages": self.fold_user })
    }

    pub fn from_json(v: &Value) -> Settings {
        Settings {
            open: false,
            show_agent: v["showAgentPicker"].as_bool().unwrap_or(false),
            scheme: Scheme::ALL.into_iter().find(|s| Some(s.name()) == v["appearance"].as_str()).unwrap_or_default(),
            fold_user: v["foldUserMessages"].as_bool().unwrap_or(true),
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

    fn change_settings(&mut self, f: impl FnOnce(&mut Settings)) {
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

    pub fn set_scheme(&mut self, scheme: Scheme) {
        self.change_settings(|s| s.scheme = scheme);
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
    let list = col()
        .key("settings-list")
        .w_full()
        .px(20.0)
        .radius(8.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .child(setting(
            p,
            "agent-picker",
            "Show agent picker",
            "Choose the agent from the composer. Hidden, the last agent still applies.",
            switch(p, "show-agent", s.settings.show_agent)
                .on_press(|s: &mut App| s.set_show_agent(!s.settings.show_agent)),
            true,
        ))
        .child(setting(
            p,
            "fold-user",
            "Fold long messages",
            "Fold your messages past 12 lines behind a Read more button",
            switch(p, "fold-user-switch", s.settings.fold_user).on_press(|s: &mut App| s.set_fold_user(!s.settings.fold_user)),
            true,
        ))
        .child(setting(p, "appearance", "Appearance", "Follow the system, or always light or dark.", scheme_control(s, p), true))
        .child(setting(p, "zoom", "Interface zoom", "Scale the whole window. ⌘+ ⌘− ⌘0 do the same.", zoom_control(s, p), false));
    col()
        .key("settings")
        .role("region")
        .label("Settings")
        .w_full()
        .grow(1.0)
        .shrink()
        .items_center()
        .child(
            col()
                .w_full()
                .max_w(720.0)
                .px(40.0)
                .pt(48.0)
                .gap(32.0)
                .child(text("Settings").font(TITLE).color(p.text))
                .child(col().w_full().gap(16.0).child(text("General").font(SECTION).color(p.text)).child(list)),
        )
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
    let thumb = el().size(12.0, 12.0).radius(2.0).fill(if p.dark { rune::color::hex("#d9d9d9").unwrap_or(Color::WHITE) } else { Color::WHITE });
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

fn scheme_control(s: &App, p: &Pal) -> El {
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

fn zoom_control(s: &App, p: &Pal) -> El {
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
    use rune::{Env, Mods, Runtime};

    fn runtime() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.frame(1.0 / 60.0);
        rt
    }

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
        let on = Settings { open: false, show_agent: true, scheme: Scheme::Light, fold_user: false };
        write(&path, &on);
        assert_eq!(read(&path), on);
        let _ = std::fs::remove_dir_all(dir);
    }
}
