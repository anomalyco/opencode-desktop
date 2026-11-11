//! Settings (⌘, or the palette's Open settings): Desktop's routed two-pane screen
//! (`settings/shell.tsx`, `navigation.tsx`, `settings.css`): a 328 pt rail of grouped pages
//! beside a 720 pt column with the open page, the whole screen scrolling under each page's
//! sticky header. The preferences live in `<data dir>/rune-opencode/settings.json`, next to
//! `ui.json`, in the shape of Desktop's `settings.v3` where they share a name.

use crate::pickers::PickerHost as _;
use std::collections::BTreeMap;
use std::path::PathBuf;

use rune::{El, Key, KeyEvent, TextStyle, Tx, col, el, row, text};
use rune_kit::Switch;
use serde_json::{Value, json};

use crate::App;
use crate::palette::{Pal, kit};

pub use prefs::{BackgroundActivity, Choice, Destination, FollowUp, Organize, Sounds, TimelineWidth, TitlebarPosition};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheme {
    #[default]
    System,
    Light,
    Dark,
}

impl Scheme {
    pub const ALL: [Scheme; 3] = [Scheme::System, Scheme::Light, Scheme::Dark];

    pub fn name(self) -> &'static str {
        match self { Self::System => "system", Self::Light => "light", Self::Dark => "dark" }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => opencode_i18n::t!("theme.scheme.system"),
            Self::Light => opencode_i18n::t!("theme.scheme.light"),
            Self::Dark => opencode_i18n::t!("theme.scheme.dark"),
        }
    }

    /// `"system"`, `"light"` or `"dark"`; the system's otherwise.
    pub fn parse(name: &str) -> Scheme {
        Self::ALL.into_iter().find(|s| s.name() == name).unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub open: bool,
    /// The composer's agent picker (`general.showCustomAgents`); the agent still applies when it's hidden.
    pub show_agent: bool,
    pub scheme: Scheme,
    /// Fold prompts past 12 lines behind Read more (`fold.rs`); Experimental, off unless turned on.
    pub fold_user: bool,
    /// The pane a new terminal opens in: `"side"` (default) or `"bottom"`.
    pub terminal_placement: crate::panels::DockPlacement,
    /// Which events post a system notification (`notifications.rs`), with Desktop's defaults.
    pub notify: crate::notifications::Kinds,
    /// Answer every permission request "once" as it arrives (Desktop's `permissions.autoApprove`).
    pub auto_accept: bool,
    /// Projects whose own Auto-accept permissions overrides that default, by project id.
    pub project_auto_accept: BTreeMap<String, bool>,
    /// Desktop's colour theme id (`themes.rs`); `oc-2` is OpenCode's own.
    pub theme: String,
    pub reduce_motion: crate::appearance::ReduceMotion,
    /// Appearance › Text rendering: how Rune rasterises text (`text_rendering::TextRenderer`).
    pub text_renderer: crate::text_rendering::TextRenderer,
    /// The terminal's font, an index into the Terminal font knob's choices.
    pub terminal_font: usize,
    /// Indexes into `appearance::UI_FONTS` and `CODE_FONTS`, saved by name.
    pub ui_font: usize,
    pub code_font: usize,
    /// An installed family typed into a font field that none of its choices is (Desktop's font
    /// rows take any name), which then stands before the default; empty for a choice.
    pub ui_font_name: String,
    pub code_font_name: String,
    pub terminal_font_name: String,
    /// Rune records the last minute of frames for ⌘⇧⌥R (`glitch`); on by default.
    pub record_glitches: bool,
    /// Desktop's Interface layout (`appearance.tabLayout`): top tabs unless changed.
    pub tab_layout: crate::layout_state::TabLayout,
    /// Questions take the keyboard-first dock instead of Classic (Desktop's `general.newQuestionUI`,
    /// Settings › Experimental › New question UI). Off unless turned on, as there.
    pub new_question_ui: bool,
    /// Download the browser engine while the app is idle (`browser::prefetch`); on by default.
    pub prefetch_browser: bool,
    /// The display language Settings picked (Desktop's `language` store), an `opencode_i18n`
    /// locale id; `None` follows the system's languages (`language::apply`).
    pub language: Option<String>,
    /// Where a new session starts (`workspaces.defaultDestination`).
    pub default_destination: Destination,
    /// Where each project's last session started (`workspaces.lastUsed`): `"local"` or `"workspace"`.
    pub last_used: BTreeMap<String, String>,
    /// What Enter does while the agent works (`general.followUpBehavior`).
    pub follow_up: FollowUp,
    /// Ctrl-scroll and trackpad pinches zoom the window (Desktop's `pinchZoom`); off by default.
    pub pinch_zoom: bool,
    /// Underline misspelt words while typing (Desktop's `spellcheck`); on by default.
    pub spellcheck: bool,
    /// The dictionaries a text field's Spellcheck Languages menu chose (Desktop's
    /// `spellcheckLanguages`); none follows the system languages.
    pub spellcheck_languages: Vec<String>,
    /// A narrow window's diffs wrap long lines rather than scroll (the review plugin's `mobileDiff.wrap`).
    pub mobile_diff_wrap: bool,
    /// Where a narrow window keeps its titlebar (`general.mobileTitlebarPosition`).
    pub mobile_titlebar: TitlebarPosition,
    /// The ChatGPT plan welcome has shown (Desktop's `chatgpt-plan-welcome.v1`).
    pub chatgpt_welcome_seen: bool,
    /// Experimental › New icons (`general.newIcons`).
    pub new_icons: bool,
    /// Experimental › Organize vertical sidebar (`general.sidebarOrganize`).
    pub sidebar_organize: Organize,
    /// Notifications › Sound effects (`sounds`).
    pub sounds: Sounds,
    /// Shortcuts the user changed (`keybinds`): command id to keybind, "none" for unbound.
    pub keybinds: BTreeMap<String, String>,
    /// Keep screen active as the app kept it before the Pairing plugin: read once to move it into
    /// the plugin's store (plugin_host::setup).
    pub keep_screen_active: bool,
    /// Preferences › Timeline: what of a turn's work shows (`general.timelineDetail`), the
    /// column (`timelineWidth`), where background work shows (`backgroundActivity`) and
    /// whether a finished turn lists its changed files (`showTurnDiffs`).
    pub timeline_detail: session_view::detail::Detail,
    pub timeline_width: TimelineWidth,
    pub background_activity: BackgroundActivity,
    pub turn_diffs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            open: false,
            show_agent: false,
            scheme: Scheme::default(),
            fold_user: false,
            terminal_placement: crate::panels::DockPlacement::Side,
            notify: Default::default(),
            auto_accept: false,
            project_auto_accept: BTreeMap::new(),
            theme: crate::themes::DEFAULT.into(),
            reduce_motion: Default::default(),
            text_renderer: Default::default(),
            terminal_font: 3,
            ui_font: 0,
            code_font: 0,
            ui_font_name: String::new(),
            code_font_name: String::new(),
            terminal_font_name: String::new(),
            record_glitches: true,
            tab_layout: Default::default(),
            new_question_ui: false,
            prefetch_browser: true,
            language: None,
            default_destination: Destination::default(),
            last_used: BTreeMap::new(),
            follow_up: FollowUp::default(),
            pinch_zoom: false,
            spellcheck: true,
            spellcheck_languages: Vec::new(),
            // The Review plugin's store starts wrapping (`gui-plugins/src/review/index.ts:46-48`).
            mobile_diff_wrap: true,
            mobile_titlebar: TitlebarPosition::default(),
            chatgpt_welcome_seen: false,
            new_icons: false,
            sidebar_organize: Organize::default(),
            sounds: Sounds::default(),
            keybinds: BTreeMap::new(),
            keep_screen_active: false,
            timeline_detail: Default::default(),
            timeline_width: TimelineWidth::default(),
            background_activity: BackgroundActivity::default(),
            turn_diffs: false,
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
            "permissions": { "autoApprove": self.auto_accept, "projects": self.project_auto_accept },
            "theme": self.theme,
            "reduceMotion": self.reduce_motion.name(),
            "textRenderer": self.text_renderer.name(),
            "terminalFont": self.terminal_font,
            "uiFont": if self.ui_font_name.is_empty() { crate::appearance::UI_FONTS[self.ui_font.min(crate::appearance::UI_FONTS.len() - 1)].name } else { self.ui_font_name.as_str() },
            "codeFont": if self.code_font_name.is_empty() { crate::appearance::CODE_FONTS[self.code_font.min(crate::appearance::CODE_FONTS.len() - 1)].name } else { self.code_font_name.as_str() },
            "terminalFontName": self.terminal_font_name,
            "developer": { "rewind": self.record_glitches },
            "tabLayout": self.tab_layout.name(),
            "newQuestionUI": self.new_question_ui,
            "browser": { "prefetch": self.prefetch_browser },
            "language": self.language,
            "workspaces": { "defaultDestination": self.default_destination.name(), "lastUsed": self.last_used },
            "followUpBehavior": self.follow_up.name(),
            "pinchZoom": self.pinch_zoom,
            "spellcheck": self.spellcheck,
            "spellcheckLanguages": self.spellcheck_languages,
            "mobileDiffWrap": self.mobile_diff_wrap,
            "mobileTitlebarPosition": self.mobile_titlebar.name(),
            "chatgptPlanWelcomeSeen": self.chatgpt_welcome_seen,
            "newIcons": self.new_icons,
            "sidebarOrganize": self.sidebar_organize.name(),
            "sounds": self.sounds.to_json(),
            "keybinds": self.keybinds,
            "keepScreenActive": self.keep_screen_active,
            "timelineDetail": self.timeline_detail.to_json(),
            "timelineWidth": self.timeline_width.name(),
            "backgroundActivity": self.background_activity.name(),
            "showTurnDiffs": self.turn_diffs,
        })
    }

    pub fn from_json(v: &Value) -> Settings {
        let d = Settings::default();
        Settings {
            open: false,
            show_agent: v["showAgentPicker"].as_bool().unwrap_or(false),
            scheme: Scheme::ALL.into_iter().find(|s| Some(s.name()) == v["appearance"].as_str()).unwrap_or_default(),
            fold_user: v["foldUserMessages"].as_bool().unwrap_or(d.fold_user),
            terminal_placement: crate::panels::DockPlacement::parse(v["terminalPlacement"].as_str()),
            notify: crate::notifications::Kinds::from_json(&v["notifications"]),
            auto_accept: v["permissions"]["autoApprove"].as_bool().unwrap_or(false),
            project_auto_accept: v["permissions"]["projects"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_bool()?))).collect()).unwrap_or_default(),
            theme: theme_id(v["theme"].as_str().unwrap_or_default()),
            reduce_motion: crate::appearance::ReduceMotion::parse(v["reduceMotion"].as_str()),
            text_renderer: crate::text_rendering::TextRenderer::parse(v["textRenderer"].as_str()),
            terminal_font: v["terminalFont"].as_u64().map_or(3, |i| (i as usize).min(crate::appearance::TERMINAL_FONTS.len() - 1)),
            ui_font: crate::appearance::font_index(&crate::appearance::UI_FONTS, v["uiFont"].as_str()),
            code_font: crate::appearance::font_index(&crate::appearance::CODE_FONTS, v["codeFont"].as_str()),
            ui_font_name: crate::appearance::custom_name(&crate::appearance::UI_FONTS, v["uiFont"].as_str()),
            code_font_name: crate::appearance::custom_name(&crate::appearance::CODE_FONTS, v["codeFont"].as_str()),
            terminal_font_name: v["terminalFontName"].as_str().map(str::trim).unwrap_or("").to_owned(),
            record_glitches: v["developer"]["rewind"].as_bool().unwrap_or(true),
            // Files from before the setting read as Desktop's default; Desktop nests it under
            // `appearance`, which here is the colour scheme.
            tab_layout: crate::layout_state::TabLayout::parse(v["tabLayout"].as_str().or_else(|| v["appearance"]["tabLayout"].as_str())),
            new_question_ui: v["newQuestionUI"].as_bool().unwrap_or(false),
            prefetch_browser: v["browser"]["prefetch"].as_bool().unwrap_or(true),
            language: v["language"].as_str().filter(|l| opencode_i18n::locales().iter().any(|locale| locale.id() == *l)).map(str::to_owned),
            default_destination: Destination::parse(v["workspaces"]["defaultDestination"].as_str()),
            last_used: v["workspaces"]["lastUsed"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str().filter(|v| matches!(*v, "local" | "workspace"))?.to_owned()))).collect()).unwrap_or_default(),
            follow_up: FollowUp::parse(v["followUpBehavior"].as_str()),
            pinch_zoom: v["pinchZoom"].as_bool().unwrap_or(d.pinch_zoom),
            spellcheck: v["spellcheck"].as_bool().unwrap_or(d.spellcheck),
            mobile_diff_wrap: v["mobileDiffWrap"].as_bool().unwrap_or(d.mobile_diff_wrap),
            mobile_titlebar: TitlebarPosition::parse(v["mobileTitlebarPosition"].as_str()),
            spellcheck_languages: v["spellcheckLanguages"].as_array().map(|a| a.iter().filter_map(|l| Some(l.as_str()?.to_owned())).collect()).unwrap_or_default(),
            chatgpt_welcome_seen: v["chatgptPlanWelcomeSeen"].as_bool().unwrap_or(false),
            new_icons: v["newIcons"].as_bool().unwrap_or(false),
            sidebar_organize: Organize::parse(v["sidebarOrganize"].as_str()),
            sounds: if v["sounds"].is_object() { Sounds::from_json(&v["sounds"]) } else { Sounds::default() },
            keybinds: v["keybinds"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect()).unwrap_or_default(),
            keep_screen_active: v["keepScreenActive"].as_bool().unwrap_or(false),
            timeline_detail: session_view::detail::Detail::from_json(&v["timelineDetail"]),
            timeline_width: TimelineWidth::parse(v["timelineWidth"].as_str()),
            background_activity: BackgroundActivity::parse(v["backgroundActivity"].as_str()),
            turn_diffs: v["showTurnDiffs"].as_bool().unwrap_or(false),
        }
    }
}

/// A saved theme id: one of Desktop's, or a plugin's (`<plugin>:<id>`), kept as written since
/// plugins register their themes after the settings load.
fn theme_id(id: &str) -> String {
    if id.contains(':') {
        return id.to_owned();
    }
    crate::themes::all()[crate::themes::index(id)].id.clone()
}

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("settings.json"))
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

impl App {
    pub fn dark(&self, cx: &rune::Cx<App>) -> bool {
        match self.settings.scheme {
            Scheme::System => cx.system_dark(),
            Scheme::Light => false,
            Scheme::Dark => true,
        }
    }

    /// Opens Settings on its first page, Preferences.
    pub fn open_settings(&mut self) {
        self.open_settings_at(pages::Page::General);
    }

    /// Opens Settings on `page`, as Desktop's `layout.settings(tab)` does.
    pub fn open_settings_at(&mut self, page: pages::Page) {
        self.close_picker(false);
        self.close_switcher(false);
        self.composer.focused = false;
        // Settings' per-server pages start on the server of the page it opens over
        // (`SettingsSurface.server`'s `sourceServer`).
        if !self.settings.open {
            self.settings_pages.server.key = crate::servers::runtime::desired(self);
        }
        self.settings.open = true;
        self.show_settings_page(page);
    }

    pub fn close_settings(&mut self) {
        self.settings.open = false;
        search::clear(self);
        self.settings_pages.shortcuts.capture = None;
        self.settings_pages.blur_fields();
    }

    pub(crate) fn change_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings);
        if self.persists() {
            save(&self.settings);
        }
    }

    pub fn set_show_agent(&mut self, on: bool) {
        self.change_settings(|s| s.show_agent = on);
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

    /// Settings' keys: the shortcut being recorded takes every key; Escape steps back through
    /// search and then leaves; mod+f focuses the search (`settings.search.focus`).
    pub fn settings_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.settings.open {
            return false;
        }
        if pages::shortcuts::key(self, ev, tx) || search::key(self, ev, tx) {
            return true;
        }
        if ev.key == Key::Escape {
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

/// `.settings-section-title`: 13 on 16 at 530.
const SECTION: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
/// `settings-row-description`: 13 on 20 at 440.
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
/// `settings-row-title`: 13 on 16 at 530, -0.04 tracking.
const ROW_TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

/// The Settings screen in a panel `size` points large (0 when not measured yet).
pub fn page(s: &App, p: &Pal, size: (f32, f32)) -> El {
    shell::view(s, p, size)
}

/// `.settings-section`: its title, 16 above its list.
pub(crate) fn section(p: &Pal, title: &str, list: El) -> El {
    col().w_full().gap(16.0).child(text(title.to_owned()).font(SECTION).color(p.text)).child(list)
}

pub(crate) fn settings_card<'a, K: AsRef<str>, D: AsRef<str>>(p: &Pal, key: &str, items: impl IntoIterator<Item = (K, &'a str, D, El)>) -> El {
    list(p, key, items.into_iter().map(|(k, title, description, control)| setting(p, k.as_ref(), title, description.as_ref(), control)).collect())
}

/// `[data-component="settings-list"]`: a `layer-01` card with 8 pt corners, 20 pt in from each
/// side and a 0.5 pt `border-muted` ring inside; its rows split by 0.5 pt `border-base` rules.
pub(crate) fn list(p: &Pal, key: &str, rows: Vec<El>) -> El {
    let n = rows.len();
    rows.into_iter().enumerate().fold(col().key(key.to_owned()).w_full().px(20.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)), |l, (i, r)| {
        l.child(r).children((i + 1 < n).then(|| el().w_full().h(0.5).fill(p.border)))
    })
}

/// `SettingsRow`: its title and description beside its control, the control centred on them.
pub(crate) fn setting(p: &Pal, key: &str, title: &str, description: &str, control: El) -> El {
    setting_at(p, key, title, description, control, true)
}

/// A row; its control centred on the text, or at its top (`SettingsRow align="start"`). A row
/// settings search can reveal carries its `data-action` as `setting-<key>`.
pub(crate) fn setting_at(p: &Pal, key: &str, title: &str, description: &str, control: El, centred: bool) -> El {
    let about = text(description.to_owned()).key(format!("setting-{key}-about")).font(BODY).color(p.muted).wrap();
    setting_with(p, key, title, about, control, centred)
}

thread_local! {
    /// The page is 520 pt or narrower (`@container settings-panel (max-width: 520px)`): each
    /// row's control wraps under its words, at the row's end.
    static ROWS_WRAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The screen is under 800 pt (`@container settings-screen (max-width: 799px)`).
    static COMPACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Notes the page's width (`.settings-panel`'s, 0 at full size) for the rows this frame builds,
/// and whether the window is at least 640 wide: below it each row's control wraps under its
/// words too (`settings-row` wraps and its control is 100% wide until `min-width: 640px`,
/// `settings.css:810-831, 864-869`).
pub(crate) fn note_panel_width(width: f32, sm: bool) {
    ROWS_WRAP.set((width > 0.0 && width <= 520.0) || !sm);
    COMPACT.set(width > 0.0);
}

/// The rows wrap their controls under their words (a page 520 wide or less, or a window under
/// 640).
pub(crate) fn rows_wrap() -> bool {
    ROWS_WRAP.get()
}

/// The screen has the narrow layout: Back and the page menu over the page.
pub(crate) fn compact() -> bool {
    COMPACT.get()
}

/// A row whose description is built apart (a link after its words).
pub(crate) fn setting_with(p: &Pal, key: &str, title: &str, about: El, control: El, centred: bool) -> El {
    let copy = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .child(text(title.to_owned()).font(ROW_TITLE).color(p.text).wrap())
        // `margin-block: -3.5px` keeps its 20 pt lines on the title's 16 pt rhythm.
        .child(about.my(-3.5));
    if ROWS_WRAP.get() {
        return col().key(format!("setting-{key}")).w_full().gap(16.0).py(20.0).child(copy.w_full()).child(row().key("control").w_full().justify_end().child(control));
    }
    let r = row().key(format!("setting-{key}")).w_full().gap(16.0).py(20.0);
    // `data-align="start"` is `align-items: flex-start` (`settings.css:819`): the control keeps
    // its own height beside a taller description rather than stretching to it.
    let r = if centred { r.center_y() } else { r.items_start() };
    r.child(copy).child(row().key("control").justify_end().child(control))
}

/// `settings-row-description` text, for a description built apart.
pub(crate) fn about(p: &Pal, words: &str) -> El {
    text(words.to_owned()).font(BODY).color(p.muted)
}

/// `a.settings-link`: accent, its hover ink under the pointer, opening `url`.
pub(crate) fn link(p: &Pal, key: &str, words: &str, url: &'static str) -> El {
    text(words.to_owned()).key(key.to_owned()).role("link").label(words.to_owned()).font(BODY).color(p.accent)
        .on_press_tx(move |_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url(url);
        })
}

/// Desktop's switch in a settings row (`switch.css`, compact, as the web draws it), with the 4 pt
/// padding a row gives it. It flips in the press's frame.
pub(crate) fn switch(p: &Pal, key: &str, on: bool) -> El {
    Switch::new(on).web().build(kit(p)).key(key.to_owned()).padding(4.0)
}

mod interface_mode;
pub mod pages;
pub mod prefs;
pub mod search;
mod shell;
mod stories;

