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

/// The results' labels and rows: 13 on 16 at 440, -0.04 tracking.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
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
    let now = s.clock;
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
            Page::Pairing => "pair device qr phone remote",
            Page::WhatsNew => "changelog release notes beta",
            _ => "",
        };
        out.push(entry(&page, &label, &label, None, keywords, "", true));
    }
    let mut add = |page: Page, target: &str, title: &str, section: Option<&str>, description: &str, keywords: &str| {
        let label = page.label();
        let where_ = match (&page, section) {
            (Page::General, s) => format!("Preferences / {}", s.unwrap_or("General")),
            (_, Some(s)) => s.to_owned(),
            _ => label.to_owned(),
        };
        out.push(entry(&page, title, &where_, Some(target), keywords, description, false));
    };
    for (page, target, title, section, description, keywords) in ROWS {
        add(page.clone(), target, title, *section, description, keywords);
    }
    if !s.browser_unsupported() {
        add(Page::General, "prefetch-browser", "Download browser in the background", None, "", "browser engine download cef");
    }
    add(Page::General, "terminal-placement", "Terminal placement", None, "Choose where the terminal opens in sessions.", "terminal dock side bottom");
    add(Page::Experimental, "record-glitches", "Rewind", Some("Developer"), "", "glitch record frames developer");
    for (tab, title, keywords) in [(ExtensionsTab::Mcps, "MCPs", "model context protocol tools"), (ExtensionsTab::Plugins, "Plugins", "extension gui built-in"), (ExtensionsTab::Skills, "Skills", "")] {
        let mut e = entry(&Page::Extensions, title, "Extensions", None, keywords, "", false);
        e.id = format!("extensions:{title}");
        e.subtab = Some(tab);
        out.push(e);
    }
    for b in crate::keymap::all().iter().filter(|b| !b.hidden) {
        let mut e = entry(&Page::Shortcuts, b.title, "Shortcuts", Some(&format!("shortcut-{}", b.id)), "shortcut keybind", "", false);
        e.id = format!("shortcut:{}", b.id);
        out.push(e);
    }
    plugin_entries(s, &mut out);
    let server = super::pages::servers::name(s);
    out.push(Entry { id: format!("server:{server}"), title: server.clone(), where_: "Connection".into(), owner: "Servers".into(), entity: true, ..entry(&Page::Servers, "", "", None, "", "", false) });
    for project in s.projects.iter() {
        let name = super::pages::projects::name(project);
        out.push(Entry {
            id: format!("project:{}", project.directory),
            title: name,
            where_: "Projects".into(),
            owner: server.clone(),
            entity: true,
            project: Some(project.directory.clone()),
            target: Some(format!("project-{}", project.directory)),
            ..entry(&Page::Projects, "", "", None, "", "", false)
        });
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
    }
}

/// What plugins declare: their manifests' `[[settings_search]]` rows (known before they load),
/// their settings rows (Extensions › Plugins) and their pages' entries.
fn plugin_entries(s: &App, out: &mut Vec<Entry>) {
    for e in crate::plugin_host::settings_search(s) {
        let (page, where_) = match e.page.as_str() {
            "general" => (Page::General, format!("Preferences / {}", e.owner)),
            "plugins" => (Page::Extensions, "Extensions".to_owned()),
            "servers" => (Page::Servers, "Server".to_owned()),
            id => (Page::Plugin(crate::plugin_host::settings_page_key(s, id)), crate::plugin_host::settings_page_title(s, id).unwrap_or_else(|| e.owner.clone())),
        };
        let top = matches!(page, Page::Plugin(_)) && e.id == e.page;
        let mut x = entry(&page, &e.title, &where_, (!top).then_some(e.id.as_str()), &e.keywords, &e.description, top);
        x.id = format!("plugin:{}:{}", e.owner, e.id);
        out.push(x);
    }
}

/// Desktop's rows, by page: `(page, data-action, title, section, description, keywords)`.
type Row = (Page, &'static str, &'static str, Option<&'static str>, &'static str, &'static str);

const ROWS: &[Row] = &[
    (Page::General, "settings-language", "Language", None, "", ""),
    (Page::General, "settings-tab-layout", "Interface layout", None, "", "top side tabs inbox sidebar mode layout vertical horizontal focus pro"),
    (Page::General, "settings-workspace-destination", "Default environment", None, "Choose where new sessions start", "worktree workspace default destination"),
    (Page::General, "settings-auto-accept-permissions", "Auto-accept permissions", None, "Approve permission requests without asking. Projects can override this in their settings.", "permissions approve allow auto accept default"),
    (Page::General, "settings-show-custom-agents", "Show agent", Some("General"), "", ""),
    (Page::General, "settings-follow-up-behavior", "Follow-up behavior", None, "", "queue steer follow up"),
    (Page::General, "settings-pinch-zoom", "Pinch to zoom", None, "", ""),
    (Page::General, "settings-spellcheck", "Spellcheck", None, "", "spelling spell check dictionary"),
    (Page::Experimental, "settings-new-icons", "New icons", None, "Preview the updated icon set", "icons iconography glyphs redesign"),
    (Page::Experimental, "settings-fold-user-messages", "Fold long messages", None, "Fold your messages past 12 lines behind a Read more button", "fold collapse long messages prompts read more"),
    (Page::Experimental, "settings-new-question-ui", "New question UI", None, "Preview the keyboard-first question dock", "question answers keyboard dock redesign"),
    (Page::Experimental, "settings-sidebar-organize", "Organize vertical sidebar", None, "Group side tabs by project or show one list. Only applies to side tabs.", "vertical side tabs projects recents organize sidebar"),
    (Page::Appearance, "settings-color-scheme", "Color scheme", None, "", "dark mode light mode system"),
    (Page::Appearance, "settings-theme", "Theme", None, "", ""),
    (Page::Appearance, "settings-ui-font", "UI Font", None, "", "interface typeface"),
    (Page::Appearance, "settings-code-font", "Code Font", None, "", "code typeface"),
    (Page::Appearance, "settings-terminal-font", "Terminal Font", None, "", "terminal typeface"),
    (Page::Appearance, "settings-zoom", "Zoom", None, "", "scale size text font larger bigger accessibility"),
    (Page::Appearance, "settings-reduce-motion", "Reduce motion", None, "", "animation accessibility"),
    (Page::Notifications, "settings-notifications-agent", "Agent", Some("Desktop notifications"), "Show system notification when the agent is complete or needs attention", "desktop notifications agent"),
    (Page::Notifications, "settings-notifications-permissions", "Permissions", Some("Desktop notifications"), "Show system notification when a permission is required", "desktop notifications permissions"),
    (Page::Notifications, "settings-notifications-questions", "Questions", Some("Desktop notifications"), "Show system notification when the agent asks a question", "desktop notifications questions mcp elicitation input"),
    (Page::Notifications, "settings-notifications-errors", "Errors", Some("Desktop notifications"), "Show system notification when an error occurs", "desktop notifications errors"),
    (Page::Notifications, "settings-sounds-agent", "Agent", Some("Sound effects"), "Play sound when the agent is complete or needs attention", "sound audio agent"),
    (Page::Notifications, "settings-sounds-permissions", "Permissions", Some("Sound effects"), "Play sound when a permission is required", "sound audio permissions"),
    (Page::Notifications, "settings-sounds-questions", "Questions", Some("Sound effects"), "Play sound when the agent asks a question", "sound audio questions mcp elicitation input"),
    (Page::Notifications, "settings-sounds-errors", "Errors", Some("Sound effects"), "Play sound when an error occurs", "sound audio errors"),
    (Page::Notifications, "settings-sounds-interface", "Interface", Some("Sound effects"), "Play subtle sounds when the wordmark appears and when you play with it", "sound audio interface effects wordmark logo"),
    (Page::Servers, "settings-shell", "Terminal shell", None, "Shell used by the terminal and agent tools", "bash zsh powershell"),
    (Page::Pairing, "settings-remote-access", "Remote access", None, "Paired devices reach this computer's OpenCode server over the internet. Turning it off disconnects them until you pair them again.", "internet tunnel opentunnel"),
    (Page::Pairing, "settings-keep-screen-active", "Keep screen active", None, "Prevent this computer's display from sleeping while OpenCode is running.", "display sleep awake local"),
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
    scored.sort_by(|(a, sa), (b, sb)| b.top.cmp(&a.top).then(sb.cmp(sa)).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
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
    if s.settings_pages.page != e.page {
        s.show_settings_page(e.page.clone());
    }
    if let Some(tab) = e.subtab {
        s.settings_pages.tab = tab;
    }
    match &e.target {
        Some(target) => {
            s.settings_pages.find.reveal = Some(Reveal { target: target.clone(), started: s.clock, fading: false });
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
    if crate::keymap::Bind::Find.matches_tx(ev, tx) {
        tx.instant();
        let q = &mut s.settings_pages.find.query;
        s.settings_pages.search.focused = false;
        s.settings_pages.shortcuts.filter.focused = false;
        s.settings_pages.projects.filter.focused = false;
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
    let at = s.settings_pages.find.highlighted.as_ref().and_then(|id| list.iter().position(|e| &e.id == id));
    match ev.key {
        Key::Down | Key::Up if !list.is_empty() => {
            tx.instant();
            let next = match (at, ev.key == Key::Down) {
                (None, true) => 0,
                (None, false) => list.len() - 1,
                (Some(i), true) => (i + 1).min(list.len() - 1),
                (Some(i), false) => i.saturating_sub(1),
            };
            s.settings_pages.find.highlighted = Some(list[next].id.clone());
            tx.scroll_into_view(format!("settings-result-{next}"));
            true
        }
        Key::Enter => {
            let Some(e) = at.or((!list.is_empty()).then_some(0)).map(|i| list[i].clone()) else { return false };
            open(s, &e, tx);
            true
        }
        _ => false,
    }
}

/// The rail's field (`.settings-search`): 28 pt, transparent until hovered, focused or typed in,
/// the glass 8 pt in and a clear button once typed.
pub fn field(s: &App, p: &Pal) -> El {
    let q = &s.settings_pages.find.query;
    let lit = q.focused || !q.text.is_empty();
    let clear_button = (!q.text.is_empty()).then(|| {
        row().key("settings-search-clear").role("button").label("Clear").size(20.0, 20.0).center().radius(4.0)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                clear(s);
                s.settings_pages.find.query.focused = true;
            })
            .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_faint))
    });
    row().key("settings-search").w_full().h(28.0).pr(4.0).gap(6.0).center_y().radius(6.0)
        .fill(if lit { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover)
        .child(el().pl(8.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(rune::text_input(q, ROW).key("settings-search-input").single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).placeholder("Search", p.faint).caret(p.text, p.selection).label("Search")
            .on_edit(|s: &mut App| &mut s.settings_pages.find.query)
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
    let find = &s.settings_pages.find;
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
                .child(text(group).font(ROW).color(p.muted)));
        for e in entries {
            let lit = find.highlighted.as_deref() == Some(&e.id);
            let current = find.selected.as_deref() == Some(&e.id);
            let entry = e.clone();
            let mut r = row().key(format!("settings-result-{at}")).role("option").label(e.title.clone()).w_full().h(36.0).px(12.0).gap(8.0).center_y().radius(6.0)
                .hover(p.hover)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| open(s, &entry, tx));
            if lit || current {
                r = r.fill(p.hover);
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
        .child(text("Narrow your search to see more specific results.").font(ROW).color(p.muted).wrap()));
    col().key("settings-results").w_full().min_h(0.0)
        .child(col().key("settings-results-scroll").w_full().shrink().min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).clip_fade(0.0, 0.0, 0.0, 24.0).child(body))
        .children(footer)
}

/// A title with the query's first match in the accent (`SearchHighlight`).
fn highlight(p: &Pal, title: &str, query: &str) -> El {
    let lower = title.to_lowercase();
    let q = query.to_lowercase();
    let Some(at) = (!q.is_empty()).then(|| lower.find(&q)).flatten().filter(|at| title.is_char_boundary(*at) && title.is_char_boundary(at + q.len())) else {
        return text(title.to_owned()).font(ROW).color(p.text).truncate();
    };
    let (a, rest) = title.split_at(at);
    let (m, b) = rest.split_at(q.len());
    row()
        .children((!a.is_empty()).then(|| text(a.to_owned()).font(ROW).color(p.text)))
        .child(text(m.to_owned()).font(ROW).color(p.accent))
        .children((!b.is_empty()).then(|| text(b.to_owned()).font(ROW).color(p.text)))
}
