//! Settings search (`settings/search.tsx`, `search-index.ts`, `search-results.ts`,
//! `search-machine.ts`): the rail's field finds pages, every row of every page, the servers and
//! their projects, and what plugins' manifests and pages declare. Its results take the page
//! list's place in the rail, grouped under sticky headers; opening one shows its page, scrolls
//! its row to the middle and lights it for 1.8 s, while the results stay beside it. ⌘F
//! (`settings.search.focus`) focuses the field; Escape clears it, then leaves Settings. The same
//! index answers the command palette's Settings results ([`palette`], [`reveal`]).

use rune::{El, Icon, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::pages::{ExtensionsTab, Page};
use crate::App;
use crate::palette::Pal;
use opencode_i18n::{key, t};

/// The results' labels and rows: 13 on 16 at 440, -0.04 tracking.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The field's text below 800 pt: 16 on `line-height-compact`'s 16.
const NARROW_INPUT: TextStyle = TextStyle::new(16.0, 440.0).lh(16.0).tracking(-0.04);
/// How many results show before "Narrow your search" (`shown`'s 60).
const SHOWN: usize = 60;
/// How long a revealed row stays lit (`settings-search-reveal`).
const REVEAL_S: f64 = 1.8;

/// One thing settings search finds.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: String,
    /// What it belongs to: a server's name, a plugin's, or nothing for the app's own.
    pub owner: String,
    /// Where it is, as the result names it: "Preferences / General", "Appearance".
    pub where_: String,
    pub page: Page,
    /// The row it reveals (`data-action`), none for a page.
    pub target: Option<String>,
    pub subtab: Option<ExtensionsTab>,
    /// A page of its own ("Pages").
    pub top: bool,
    /// A server or a project rather than a setting.
    pub entity: bool,
    pub project: Option<String>,
    pub icon: &'static Icon,
    /// The server whose page it's on, when Settings lists more than one: opening it shows that
    /// server's pages.
    pub server: Option<opencode_kit::servers::ServerKey>,
}

/// The rail's search: its field, the lit and opened results, and the row being revealed.
#[derive(Default)]
pub struct Find {
    pub query: TextEdit,
    /// The result the arrows light, by id.
    pub highlighted: Option<String>,
    /// The result opened last (`aria-current`), by id.
    pub selected: Option<String>,
    /// The row a result or the palette opened, lit since `started` (the app's clock).
    reveal: Option<Reveal>,
}

#[derive(Clone, Debug, PartialEq)]
struct Reveal {
    target: String,
    started: f64,
    /// The first frame draws it lit; after that it fades.
    fading: bool,
}

/// Whether the rail shows results instead of its pages.
pub fn active(s: &App) -> bool {
    !s.settings_pages.find.query.text.trim().is_empty()
}

/// Whether row `target` is being revealed: `Some(true)` while lit, `Some(false)` while it fades.
pub fn reveal_of(s: &App, target: &str) -> Option<bool> {
    s.settings_pages.find.reveal.as_ref().filter(|r| r.target == target).map(|r| !r.fading)
}

/// Clears the search, as Escape or closing Settings does.
pub fn clear(s: &mut App) {
    let find = &mut s.settings_pages.find;
    let focused = find.query.focused;
    *find = Find::default();
    find.query.focused = focused;
}

/// A page opened from the rail ends the reveal, as another view does.
pub fn leave(s: &mut App) {
    s.settings_pages.find.reveal = None;
}

/// Starts the lit row's fade after its first frame, and drops it once faded.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = *s.clock;
    if s.settings_pages.find.reveal.is_none() {
        return false;
    }
    let Some(r) = s.settings_pages.find.reveal.as_mut() else { return false };
    if !s.settings.open {
        s.settings_pages.find.reveal = None;
        return true;
    }
    if !r.fading && now > r.started + 0.05 {
        r.fading = true;
        tx.wake_after(std::time::Duration::from_secs_f64(REVEAL_S + 0.1));
        return true;
    }
    if r.fading && now > r.started + REVEAL_S + 0.1 {
        s.settings_pages.find.reveal = None;
        return true;
    }
    if !r.fading {
        tx.wake_after(std::time::Duration::from_millis(50));
    }
    false
}

/// Every entry: the pages, then each page's rows, the plugins', the server and its projects.
pub fn index(s: &App) -> Vec<Entry> {
    let mut out = Vec::new();
    for page in super::shell::groups(s).into_iter().flatten() {
        let label = super::shell::label(s, &page);
        let keywords = match page {
            Page::Shortcuts => "keybind keyboard hotkey",
            Page::About => "version license credits",
            Page::Experimental => "experiments preview beta",
            Page::Worktrees => "workspaces disk usage cleanup delete",
            Page::Providers => "connect api key credentials",
            Page::Models => "model picker visibility",
            _ => "",
        };
        out.push(entry(&page, &label, &label, None, keywords, "", true));
    }
    let mut add = |page: Page, target: &str, title: opencode_i18n::Key, section: Option<opencode_i18n::Key>, description: Option<opencode_i18n::Key>, keywords: &str| {
        let text = |key: opencode_i18n::Key| opencode_i18n::I18N.text(key);
        let where_ = match (&page, section) {
            (Page::General, s) => format!("{} / {}", Page::General.label(), s.map_or(t!("settings.general.section.general"), text)),
            (_, Some(s)) => text(s).to_owned(),
            _ => page.label().to_owned(),
        };
        let mut e = entry(&page, text(title), &where_, Some(target), keywords, description.map_or("", text), false);
        e.id = format!("{}:{target}", page.id());
        out.push(e);
    };
    // A server's own rows are indexed only for the server its pages show, while they can load
    // (`canIndexServerSettings`).
    let servers = super::pages::servers::inventory::list(s);
    let selected = servers.iter().find(|x| x.key == s.settings_pages.server.key).unwrap_or(&servers[0]).clone();
    for (page, target, title, section, description, keywords) in ROWS {
        if *page == Page::Servers && !selected.availability.can_index() {
            continue;
        }
        add(page.clone(), target, *title, *section, *description, keywords);
    }
    if cfg!(target_os = "macos") {
        add(Page::General, "terminal-option-meta", key!("rune.settings.general.row.terminalOptionMeta.title"), None, None, "terminal option alt meta esc keyboard");
    }
    if cfg!(windows) {
        add(Page::General, "terminal-marks-pwsh", key!("rune.settings.general.row.terminalMarks.search"), None, None, "terminal shell integration prompt marks osc 133 powershell pwsh cmd git bash wsl");
    }
    add(Page::General, "terminal-placement", key!("rune.settings.general.row.terminalPlacement.title"), None, Some(key!("rune.settings.general.row.terminalPlacement.description")), "terminal dock side bottom");
    add(Page::Experimental, "record-glitches", key!("rune.settings.developer.rewind.title"), Some(key!("rune.settings.developer.title")), None, "glitch record frames developer");
    for (tab, id, title, keywords) in [(ExtensionsTab::Mcps, "mcps", t!("settings.extensions.tab.mcps"), "model context protocol tools"), (ExtensionsTab::Plugins, "plugins", t!("settings.extensions.tab.plugins"), "extension gui built-in"), (ExtensionsTab::Skills, "skills", t!("settings.extensions.tab.skills"), "")] {
        let mut e = entry(&Page::Extensions, title, Page::Extensions.label(), None, keywords, "", false);
        e.id = format!("extensions:{id}");
        e.subtab = Some(tab);
        out.push(e);
    }
    for b in crate::keymap::all().iter().filter(|b| b.listed) {
        let mut e = entry(&Page::Shortcuts, &b.title(), Page::Shortcuts.label(), Some(&format!("shortcut-{}", b.id)), "shortcut keybind", "", false);
        e.id = format!("shortcut:{}", b.id);
        out.push(e);
    }
    plugin_entries(s, &mut out);
    let multiple = servers.len() > 1;
    for e in out.iter_mut().filter(|e| e.page.per_server() || e.target.as_deref() == Some("settings-shell")) {
        if multiple {
            e.owner = selected.name.clone();
            e.server = Some(selected.key.clone());
        }
    }
    for server in &servers {
        out.push(Entry {
            id: format!("server:{}", server.key),
            title: server.name.clone(),
            where_: t!("settings.server.section.connection").into(),
            owner: t!("settings.tab.servers").into(),
            entity: true,
            server: multiple.then(|| server.key.clone()),
            ..entry(&Page::Servers, "", "", None, "", "", false)
        });
    }
    // The projects of the server the pages show, once the window has read them.
    if selected.availability.can_index() && *crate::servers::runtime::active(s) == selected.key {
        for project in s.projects.iter() {
            let name = super::pages::projects::name(project);
            out.push(Entry {
                id: format!("project:{}:{}", selected.key, project.directory),
                title: name,
                where_: t!("settings.tab.projects").into(),
                owner: selected.name.clone(),
                entity: true,
                project: Some(project.directory.clone()),
                target: Some(format!("project-{}", project.directory)),
                server: multiple.then(|| selected.key.clone()),
                ..entry(&Page::Projects, "", "", None, "", "", false)
            });
        }
    }
    out
}

fn entry(page: &Page, title: &str, where_: &str, target: Option<&str>, keywords: &str, description: &str, top: bool) -> Entry {
    Entry {
        id: format!("{}:{}:{title}", page.id(), target.unwrap_or_default()),
        title: title.to_owned(),
        description: description.to_owned(),
        keywords: keywords.to_owned(),
        owner: String::new(),
        where_: where_.to_owned(),
        page: page.clone(),
        target: target.map(str::to_owned),
        subtab: None,
        top,
        entity: false,
        project: None,
        icon: page.icon(),
        server: None,
    }
}

/// What plugins declare: their manifests' `[[settings_search]]` rows (known before they load),
/// their settings rows (Extensions › Plugins) and their pages' entries.
fn plugin_entries(s: &App, out: &mut Vec<Entry>) {
    for e in crate::plugin_host::settings_search(s) {
        let (page, where_) = match e.page.as_str() {
            "general" => (Page::General, format!("{} / {}", Page::General.label(), e.owner)),
            "plugins" => (Page::Extensions, Page::Extensions.label().to_owned()),
            "servers" => (Page::Servers, Page::Servers.label().to_owned()),
            id => (Page::Plugin(crate::plugin_host::settings_page_key(s, id)), crate::plugin_host::settings_page_title(s, id).unwrap_or_else(|| e.owner.clone())),
        };
        let top = matches!(page, Page::Plugin(_)) && e.id == e.page;
        let mut x = entry(&page, &e.title, &where_, (!top).then_some(e.id.as_str()), &e.keywords, &e.description, top);
        x.id = format!("plugin:{}:{}", e.owner, e.id);
        out.push(x);
    }
}

/// Desktop's rows, by page (`search-catalog.ts`): `(page, data-action, title, section,
/// description, keywords)`, the keywords in English as there.
type Row = (Page, &'static str, opencode_i18n::Key, Option<opencode_i18n::Key>, Option<opencode_i18n::Key>, &'static str);

const TIMELINE: Option<opencode_i18n::Key> = Some(key!("settings.timeline.title"));
const NOTIFICATIONS: Option<opencode_i18n::Key> = Some(key!("settings.general.section.notifications"));
const SOUNDS: Option<opencode_i18n::Key> = Some(key!("settings.general.section.sounds"));

const ROWS: &[Row] = &[
    (Page::General, "settings-language", key!("settings.general.row.language.title"), None, None, ""),
    (Page::General, "settings-tab-layout", key!("settings.general.row.interfaceMode.title"), None, None, "top side tabs inbox sidebar mode layout vertical horizontal focus pro"),
    (Page::General, "settings-workspace-destination", key!("settings.workspaces.default.title"), None, Some(key!("settings.workspaces.default.description")), "worktree workspace default destination"),
    (Page::General, "settings-auto-accept-permissions", key!("command.permissions.autoaccept.enable"), None, Some(key!("settings.permissions.autoAccept.default.description")), "permissions approve allow auto accept default"),
    (Page::General, "settings-timeline-detail", key!("settings.timeline.detail"), TIMELINE, None, "thinking reasoning tools timeline summary detailed"),
    (Page::General, "settings-background-activity", key!("settings.timeline.row.backgroundActivity.title"), TIMELINE, Some(key!("settings.timeline.row.backgroundActivity.description")), "capsule panel subagent shell background activity position alignment hidden left center right"),
    (Page::General, "settings-timeline-width", key!("settings.timeline.row.width.title"), TIMELINE, Some(key!("settings.timeline.row.width.description")), "column line length measure narrow focused comfortable wide readability"),
    (Page::General, "settings-show-turn-diffs", key!("settings.timeline.row.turnDiffs.title"), TIMELINE, Some(key!("settings.timeline.row.turnDiffs.description")), ""),
    (Page::General, "settings-show-custom-agents", key!("settings.general.row.showCustomAgents.title"), Some(key!("settings.general.section.general")), None, ""),
    (Page::General, "settings-follow-up-behavior", key!("settings.general.row.followUpBehavior.title"), None, None, "queue steer follow up"),
    (Page::General, "settings-pinch-zoom", key!("settings.general.row.pinchZoom.title"), None, None, ""),
    (Page::General, "settings-spellcheck", key!("settings.general.row.spellcheck.title"), None, None, "spelling spell check dictionary"),
    (Page::General, "settings-mobile-diff-wrap", key!("plugin.review.settings.wrapLines.title"), None, None, "diff wrap lines"),
    (Page::Experimental, "settings-new-icons", key!("settings.experimental.row.newIcons.title"), None, Some(key!("settings.experimental.row.newIcons.description")), "icons iconography glyphs redesign"),
    (Page::Experimental, "settings-fold-user-messages", key!("settings.experimental.row.foldUserMessages.title"), None, Some(key!("settings.experimental.row.foldUserMessages.description")), "fold collapse long messages prompts read more"),
    (Page::Experimental, "settings-new-question-ui", key!("settings.experimental.row.newQuestionUI.title"), None, Some(key!("settings.experimental.row.newQuestionUI.description")), "question answers keyboard dock redesign"),
    (Page::Experimental, "settings-sidebar-organize", key!("settings.experimental.row.sidebarOrganize.title"), None, Some(key!("settings.experimental.row.sidebarOrganize.description")), "vertical side tabs projects recents organize sidebar"),
    (Page::Appearance, "settings-color-scheme", key!("settings.general.row.colorScheme.title"), None, None, "dark mode light mode system"),
    (Page::Appearance, "settings-theme", key!("settings.general.row.theme.title"), None, None, ""),
    (Page::Appearance, "settings-ui-font", key!("settings.general.row.uiFont.title"), None, None, "interface typeface"),
    (Page::Appearance, "settings-code-font", key!("settings.general.row.font.title"), None, None, "code typeface"),
    (Page::Appearance, "settings-terminal-font", key!("settings.general.row.terminalFont.title"), None, None, "terminal typeface"),
    (Page::Appearance, "settings-zoom", key!("settings.appearance.row.zoom.title"), None, None, "scale size text font larger bigger accessibility"),
    (Page::Appearance, "settings-reduce-motion", key!("settings.appearance.row.reduceMotion.title"), None, None, "animation accessibility"),
    (Page::Appearance, "settings-text-renderer", key!("rune.textRendering.title"), None, Some(key!("rune.textRendering.description")), "font antialiasing smoothing windfoil directwrite cleartype retina"),
    (Page::Notifications, "settings-notifications-agent", key!("settings.general.notifications.agent.title"), NOTIFICATIONS, Some(key!("settings.general.notifications.agent.description")), "desktop notifications agent"),
    (Page::Notifications, "settings-notifications-permissions", key!("settings.general.notifications.permissions.title"), NOTIFICATIONS, Some(key!("settings.general.notifications.permissions.description")), "desktop notifications permissions"),
    (Page::Notifications, "settings-notifications-questions", key!("settings.general.notifications.questions.title"), NOTIFICATIONS, Some(key!("settings.general.notifications.questions.description")), "desktop notifications questions mcp elicitation input"),
    (Page::Notifications, "settings-notifications-errors", key!("settings.general.notifications.errors.title"), NOTIFICATIONS, Some(key!("settings.general.notifications.errors.description")), "desktop notifications errors"),
    (Page::Notifications, "settings-sounds-agent", key!("settings.general.sounds.agent.title"), SOUNDS, Some(key!("settings.general.sounds.agent.description")), "sound audio agent"),
    (Page::Notifications, "settings-sounds-permissions", key!("settings.general.sounds.permissions.title"), SOUNDS, Some(key!("settings.general.sounds.permissions.description")), "sound audio permissions"),
    (Page::Notifications, "settings-sounds-questions", key!("settings.general.sounds.questions.title"), SOUNDS, Some(key!("settings.general.sounds.questions.description")), "sound audio questions mcp elicitation input"),
    (Page::Notifications, "settings-sounds-errors", key!("settings.general.sounds.errors.title"), SOUNDS, Some(key!("settings.general.sounds.errors.description")), "sound audio errors"),
    (Page::Notifications, "settings-sounds-interface", key!("settings.general.sounds.interface.title"), SOUNDS, Some(key!("settings.general.sounds.interface.description")), "sound audio interface effects wordmark logo"),
    (Page::Servers, "settings-shell", key!("settings.general.row.shell.title"), None, Some(key!("settings.general.row.shell.description")), "bash zsh powershell"),
];

/// `rankSettings`: entries any of whose words the query names, the pages first, then the
/// best match (the whole title, every word in it, in its keywords, anywhere), then by title.
pub fn rank(query: &str, items: Vec<Entry>) -> Vec<Entry> {
    let value = normalize(query);
    if value.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(Entry, u8)> = items
        .into_iter()
        .filter_map(|item| {
            let tokens: Vec<&str> = value.split(' ').collect();
            let title = normalize(&item.title);
            let primary = normalize(&format!("{} {}", item.title, item.keywords));
            let description = normalize(&item.description);
            let context = normalize(&format!("{} {}", item.owner, if item.entity { "" } else { &item.where_ }));
            let fuzzy = fuzzy(&value, &title);
            let named = tokens.iter().any(|t| format!("{primary} {description}").contains(t));
            if !named && !fuzzy {
                return None;
            }
            let all = |hay: &str| tokens.iter().all(|t| hay.contains(t));
            let score = if title == value {
                5
            } else if all(&title) {
                4
            } else if all(&primary) {
                3
            } else if all(&format!("{primary} {description} {context}")) {
                2
            } else if fuzzy {
                1
            } else {
                0
            };
            (score > 0).then_some((item, score))
        })
        .collect();
    scored.sort_by(|(a, sa), (b, sb)| b.top.cmp(&a.top).then(sb.cmp(sa)).then_with(|| rune::text::compare_names(&a.title, &b.title)));
    scored.into_iter().map(|(e, _)| e).collect()
}

/// A close fuzzy match on the title: its letters in order, mostly in runs (fuzzysort's 0.6).
fn fuzzy(query: &str, title: &str) -> bool {
    let len = query.chars().filter(|c| !c.is_whitespace()).count() as i64;
    session_view::pickers::search::score(query, &[title]).is_some_and(|s| s >= 18 * len)
}

fn normalize(value: &str) -> String {
    value.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A result's group (`category`): Pages, Settings, Projects or Servers.
fn category(e: &Entry) -> &'static str {
    if e.top {
        "Pages"
    } else if !e.entity {
        "Settings"
    } else if e.project.is_some() {
        "Projects"
    } else {
        "Servers"
    }
}

/// A group's header (`settings.search.group.*`).
fn category_label(group: &str) -> &'static str {
    match group {
        "Pages" => t!("settings.search.group.pages"),
        "Settings" => t!("settings.search.group.settings"),
        "Projects" => t!("settings.search.group.projects"),
        _ => t!("settings.search.group.servers"),
    }
}

/// The results as shown: the first 60, each group's together in the order they first rank.
fn shown(s: &App) -> (Vec<Entry>, bool) {
    let all = rank(&s.settings_pages.find.query.text, index(s));
    let more = all.len() > SHOWN;
    let top: Vec<Entry> = all.into_iter().take(SHOWN).collect();
    let mut order: Vec<&'static str> = Vec::new();
    for e in &top {
        if !order.contains(&category(e)) {
            order.push(category(e));
        }
    }
    (order.into_iter().flat_map(|c| top.iter().filter(move |e| category(e) == c).cloned()).collect(), more)
}

/// Opens result `e`: its page, its row scrolled to the middle and lit.
pub fn open(s: &mut App, e: &Entry, tx: &mut Tx) {
    s.settings_pages.find.selected = Some(e.id.clone());
    s.settings_pages.find.highlighted = Some(e.id.clone());
    reveal_entry(s, e, tx);
}

/// Opens `e` from the command palette: Settings on its page with its row lit, without a search.
pub fn reveal(s: &mut App, e: &Entry, tx: &mut Tx) {
    if !s.settings.open {
        s.open_settings_at(e.page.clone());
    }
    clear(s);
    reveal_entry(s, e, tx);
}

fn reveal_entry(s: &mut App, e: &Entry, tx: &mut Tx) {
    tx.instant();
    let moved = e.server.as_ref().is_some_and(|server| *server != s.settings_pages.server.key);
    if let Some(server) = &e.server {
        s.settings_pages.server.key = server.clone();
    }
    if s.settings_pages.page != e.page || moved {
        s.show_settings_page(e.page.clone());
    }
    if let Some(tab) = e.subtab {
        s.settings_pages.tab = tab;
    }
    match &e.target {
        Some(target) => {
            s.settings_pages.find.reveal = Some(Reveal { target: target.clone(), started: *s.clock, fading: false });
            tx.scroll_center(format!("setting-{target}"));
            tx.wake_after(std::time::Duration::from_millis(50));
        }
        None => tx.scroll_to_instant("settings", 0.0, 0.0),
    }
}

/// The command palette's Settings results for `query` (VP-09): the same index and ranking as
/// the rail, at most `max`.
pub fn palette(s: &App, query: &str, max: usize) -> Vec<Entry> {
    rank(query, index(s)).into_iter().take(max).collect()
}

/// The field's and results' keys: ↑↓ light a result, Enter opens it, Escape clears the query;
/// ⌘F focuses the field from anywhere in Settings.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if crate::keymap::Bind::SettingsSearch.matches_tx(ev, tx) {
        tx.instant();
        let pages = &mut *s.settings_pages;
        let q = &mut pages.find.query;
        pages.search.focused = false;
        pages.shortcuts.filter.focused = false;
        pages.projects.filter.focused = false;
        q.focused = true;
        let end = q.text.len();
        q.set_selection(0, end);
        return true;
    }
    let focused = s.settings_pages.find.query.focused;
    if ev.key == Key::Escape && active(s) {
        tx.instant();
        clear(s);
        s.settings_pages.find.query.focused = focused;
        return true;
    }
    if !focused || !active(s) {
        return false;
    }
    let (list, _) = shown(s);
    let at = active_index(&list, s.settings_pages.find.highlighted.as_deref());
    // `searchListMove` and `searchListTarget` (`search-list/navigation.ts`): the arrows wrap,
    // Home and End go to the ends.
    let next = match ev.key {
        _ if list.is_empty() || ev.mods.shift || ev.mods.ctrl || ev.mods.cmd || ev.mods.alt => None,
        Key::Down => Some((at + 1) % list.len()),
        Key::Up => Some(if at == 0 { list.len() - 1 } else { at - 1 }),
        Key::Home => Some(0),
        Key::End => Some(list.len() - 1),
        _ => None,
    };
    if let Some(next) = next {
        tx.instant();
        s.settings_pages.find.highlighted = Some(list[next].id.clone());
        // The first row shows from the top, so its header is never hidden (`search-list.tsx:133-140`).
        if next == 0 {
            tx.scroll_to_instant("settings-results-scroll", 0.0, 0.0);
        } else {
            tx.scroll_into_view(format!("settings-result-{next}"));
        }
        return true;
    }
    if ev.key == Key::Enter
        && let Some(e) = list.get(at).cloned()
    {
        open(s, &e, tx);
        return true;
    }
    false
}

/// The active row (`createTypeahead`'s `activeItem`): the one picked while it's still listed,
/// else the first (`typeahead/controller.ts:17-22`).
fn active_index(list: &[Entry], highlighted: Option<&str>) -> usize {
    highlighted.and_then(|id| list.iter().position(|e| e.id == id)).unwrap_or(0)
}

/// The rail's field (`.settings-search`): 28 pt, transparent until hovered, focused or typed in,
/// the glass 8 pt in and a clear button once typed. Below 800 pt the screen's container query sets
/// its text in 16 (`settings.css:1022-1024`).
pub fn field(s: &App, p: &Pal, narrow: bool) -> El {
    let q = &s.settings_pages.find.query;
    let lit = q.focused || !q.text.is_empty();
    let clear_button = (!q.text.is_empty()).then(|| {
        row().key("settings-search-clear").role("button").label_static(t!("common.clear")).size(20.0, 20.0).center().radius(4.0)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                clear(s);
                s.settings_pages.find.query.focused = true;
            })
            .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_faint))
    });
    // `text-input-v2`'s 8 at its end, the clear button its own child (`text-input.css:11`,
    // `text-input.tsx:82-108`).
    let field = row().key("settings-search").w_full().h(28.0).pr(8.0).gap(6.0).center_y().radius(6.0)
        .fill(if lit { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover);
    // Focused, `text-input-v2` draws its 1 pt `border-focus` outline inside its edge
    // (`text-input.css:17-18, 62-65`).
    let field = if q.focused { field.border(1.0, p.v2.border_focus) } else { field };
    field
        .child(el().pl(8.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(session_view::style::field(q, if narrow { NARROW_INPUT } else { ROW }).key("settings-search-input").single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).placeholder(t!("settings.search.placeholder"), p.faint)
            // A touch screen's `input::placeholder` is 13 under the 16 the field types (`base.css`).
            .placeholder_font(if narrow && !session_view::style::touch_controls() { NARROW_INPUT } else { ROW }).caret(p.text, p.selection).selection_style(p.field_selection).label_static(t!("settings.search.placeholder"))
            .on_edit(|s: &mut App| &mut s.settings_pages.find.query)
            // Enter opens the lit result, else the first (`search-machine.ts`).
            .on_submit(|s: &mut App, tx: &mut Tx| {
                let (list, _) = shown(s);
                if let Some(e) = list.get(active_index(&list, s.settings_pages.find.highlighted.as_deref())).cloned() {
                    open(s, &e, tx);
                }
            })
            .on_input_change(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.settings_pages.find.highlighted = None;
                tx.scroll_to_instant("settings-results", 0.0, 0.0);
            }))
        .children(clear_button)
}

/// The results in the rail (`SearchList`): sections under sticky 28 pt headers, 36 pt rows 1 apart.
pub fn results(s: &App, p: &Pal) -> El {
    let (list, more) = shown(s);
    let query = s.settings_pages.find.query.text.trim().to_owned();
    if list.is_empty() {
        return row().key("settings-results").w_full().h(120.0).center()
            .child(super::pages::no_results(p, "settings-search-empty", &query));
    }
    let active = active_index(&list, s.settings_pages.find.highlighted.as_deref());
    // `[data-active]` (`search-list.css:155-158, 222-224`): the hover overlay, `alpha-light-10` in dark.
    let active_fill = if p.dark { rune::Color::WHITE.alpha(0.10) } else { p.hover };
    let mut body = col().key("settings-results-list").w_full().pt(6.0).px(6.0).pb(8.0);
    let mut at = 0;
    let groups: Vec<&'static str> = list.iter().map(category).fold(Vec::new(), |mut v, c| {
        if !v.contains(&c) {
            v.push(c);
        }
        v
    });
    for group in groups {
        let entries: Vec<&Entry> = list.iter().filter(|e| category(e) == group).collect();
        let icons = entries.iter().any(|e| e.top || e.project.is_some());
        let mut section = col().key(format!("settings-results-{group}")).w_full().gap(1.0).pb(16.0)
            .child(row().key("header").w_full().h(28.0).mx(-6.0).px(18.0).center_y().fill(p.bg_base).sticky_top(0.0)
                .child(text(category_label(group)).font(ROW).color(p.muted)));
        for e in entries {
            let entry = e.clone();
            let id = e.id.clone();
            // The pointer takes the active row as it moves over one (`search-list.tsx:281-286`);
            // the row has no hover of its own.
            let mut r = row().key(format!("settings-result-{at}")).role("option").label(e.title.clone()).w_full().h(36.0).px(12.0).gap(8.0).center_y().radius(6.0)
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if inside && s.settings_pages.find.highlighted.as_deref() != Some(id.as_str()) {
                        tx.instant();
                        s.settings_pages.find.highlighted = Some(id.clone());
                    }
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| open(s, &entry, tx));
            if at == active {
                r = r.fill(active_fill);
            }
            if icons {
                let mark = match &e.project {
                    Some(dir) => crate::icons::badge(s, p, s.project_at(dir), &e.title),
                    None => icon(e.icon, 16.0).color(p.icon_muted),
                };
                r = r.child(el().size(16.0, 16.0).center().child(mark));
            }
            r = r.child(highlight(p, &e.title, &query));
            if e.where_ != e.title && !(e.entity && e.project.is_some()) {
                r = r.child(text(e.where_.clone()).font(ROW).color(p.muted).shrink().min_w(0.0).truncate());
            }
            section = section.child(r);
            at += 1;
        }
        body = body.child(section);
    }
    let footer = more.then(|| row().key("settings-results-footer").w_full().pt(4.0).px(18.0).pb(12.0)
        .child(text(t!("settings.search.refine")).font(ROW).color(p.muted).wrap()));
    col().key("settings-results").w_full().min_h(0.0)
        // `SearchList`'s `MenuScroll` (`search.tsx:125`, `search-list.tsx:319`).
        .child(session_view::scroller::scroller("settings-results-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().w_full().shrink().min_h(0.0).clip_fade(0.0, 0.0, 0.0, 24.0), body))
        .children(footer)
}

/// A title with the query's first match in the accent (`SearchHighlight`), one run that shrinks
/// and truncates beside the page as the row's `min-w-0 truncate` spans do.
fn highlight(p: &Pal, title: &str, query: &str) -> El {
    let t = text(title.to_owned()).font(ROW).color(p.text).truncate().shrink().min_w(0.0);
    match crate::language::search_fold().find(title, query.trim()).first() {
        Some(r) => t.spans(vec![rune::Span::new(r.start, r.end).color(p.accent)]),
        None => t,
    }
}
