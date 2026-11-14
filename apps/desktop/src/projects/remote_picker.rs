//! Choosing a folder on a server (`workspaces/selection/picker.tsx`, `dialog.tsx`, `domain.ts`):
//! this computer's folder dialog for the app's own server (and one on its loopback port), and
//! for any other server, whose folders aren't this computer's (WSL, SSH, another machine),
//! Desktop's `DirectoryPickerDialog`: a path field with suggestions, ~ / Root / Parent, the
//! server's recent projects, and its folders as a tree read through `/api/fs/list`, then Select
//! folder. The dialog closes, cancelling the pick, if its server goes.
//!
//! The dialog is a state machine ([`Tree`], [`Suggest`]): every listing carries its attempt, and
//! an answer for another changes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use opencode_kit::model::Project;
use opencode_kit::servers::ServerKey;
use opencode_kit::servers::fs::{absolute_path, canonical};
use rune::machine::{Attempt, Seq};
use rune::{FileDialogOptions, TextEdit, Tx};
use serde_json::Value;

use crate::App;

/// What runs with the folder picked.
pub type OnPick = Rc<dyn Fn(&mut App, String, &mut Tx)>;

/// The dialog (`DirectoryPickerDialog`), while one is open.
#[derive(Default)]
pub enum Picker {
    #[default]
    Closed,
    Open(Box<Open>),
}

pub struct Open {
    pub server: ServerKey,
    conn: opencode_kit::client::Server,
    pub title: String,
    /// The server's name, under the title (`directory-picker-subtitle`).
    pub subtitle: String,
    /// The server's home folder, for `~` (`sync.data.path.home`).
    pub home: String,
    pub tree: Tree,
    pub input: TextEdit,
    pub suggest: Suggest,
    pub recent: Vec<Project>,
    /// Git projects the server knows, which their folders' rows mark.
    git: BTreeSet<String>,
    seq: Seq,
    on_pick: OnPick,
}

/// One folder's children (`PickerNode`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Loading { attempt: Attempt },
    Loaded(Vec<Dir>),
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dir {
    pub name: String,
    pub absolute: String,
}

/// The folder tree (`tree-machine.ts`): where the server is, then a root and its folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tree {
    /// Asking the server for its home and location.
    Locating { attempt: Attempt },
    Browsing { root: String, nodes: BTreeMap<String, Node>, expanded: BTreeSet<String>, selected: Option<String> },
    Failed,
}

/// The path field's suggestions (`combobox-machine.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Suggest {
    #[default]
    Closed,
    Searching { attempt: Attempt, query: String },
    Shown { items: Vec<String>, active: usize },
}

impl Tree {
    /// The folder Select folder picks: the one selected, else the root.
    pub fn result(&self) -> Option<&str> {
        match self {
            Tree::Browsing { selected: Some(s), .. } => Some(s),
            Tree::Browsing { root, .. } => Some(root),
            _ => None,
        }
    }
}

/// Picks a folder on server `server`, then runs `on_pick` with it.
pub fn pick(s: &mut App, server: &ServerKey, title: String, tx: &mut Tx, on_pick: impl Fn(&mut App, String, &mut Tx) + 'static) {
    let entry = crate::servers::registry::list(s).into_iter().find(|e| e.key() == *server);
    let own = crate::servers::runtime::own_server().map(|o| o.base);
    let local = entry.as_ref().is_none_or(|e| e.conn.local(own.as_deref())) || !s.live();
    tx.instant();
    if local {
        let options = FileDialogOptions { title: title.into(), ..Default::default() };
        tx.open_folder(options, move |s: &mut App, result, tx| match result {
            Ok(Some(path)) => on_pick(s, crate::projects::clean_dir(&path.to_string_lossy()), tx),
            Ok(None) => {}
            Err(e) => crate::status_toast::fail(s, opencode_i18n::t!("rune.status.folderPickerFailed"), e.to_string()),
        });
        return;
    }
    // Without a connection to it the picker would open on nothing: the pick ends there.
    let Some(conn) = entry.as_ref().and_then(|e| crate::servers::registry::server_for(&e.conn)) else { return };
    let (recent, git) = crate::servers::runtime::with_server(s, server, |s| {
        let mut seen = BTreeSet::new();
        // `[...projects.list(), ...sync.data.project]`: the server's projects in its own order, most
        // recently active first, then by id (`desc(time_active), asc(id)`, `sync.tsx:136-138`).
        let mut all: Vec<&Project> = s.projects.iter().collect();
        all.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.id.cmp(&b.id)));
        let recent: Vec<Project> = all.into_iter().filter(|p| p.id != "global" && !p.directory.is_empty() && seen.insert(crate::projects::clean_dir(&p.directory))).take(5).cloned().collect();
        let git = s.projects.iter().filter(|p| opencode_kit::model::project_git(&p.id)).map(|p| canonical(&p.directory)).collect();
        (recent, git)
    })
    .unwrap_or_default();
    let mut seq = Seq::default();
    let attempt = seq.next();
    s.remote_picker = Picker::Open(Box::new(Open {
        server: server.clone(),
        subtitle: crate::servers::registry::label_of(s, server),
        conn: conn.clone(),
        title,
        home: String::new(),
        tree: Tree::Locating { attempt },
        input: TextEdit::new(""),
        suggest: Suggest::Closed,
        recent,
        git,
        seq,
        on_pick: Rc::new(on_pick),
    })).into();
    if let Picker::Open(o) = &mut *s.remote_picker {
        o.input.focused = true;
    }
    s.project_state.home_menu_open = false;
    let deliver = s.deliverer();
    crate::plugin_host::tasks::run(move || {
        // `bootstrapGlobal`: the home is inferred from the server's location or its project's.
        let v = conn.get("/api/location").ok();
        let location = v.as_ref().and_then(|v| text(v, &["directory"]));
        let home = v.as_ref().and_then(|v| [location.clone(), text(&v["project"], &["directory"])].into_iter().flatten().map(|d| infer_home(&d)).find(|h| !h.is_empty()));
        deliver(Box::new(move |s: &mut App, tx: &mut Tx| located(s, attempt, home, location, tx)));
    });
}

fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v[*k].as_str().filter(|t| !t.is_empty()).map(str::to_owned))
}

/// The server said where it is: the tree opens on its home (`start`).
fn located(s: &mut App, attempt: Attempt, home: Option<String>, location: Option<String>, tx: &mut Tx) {
    let Picker::Open(o) = &mut *s.remote_picker else { return };
    if o.tree != (Tree::Locating { attempt }) {
        return;
    }
    let home = home.map(|h| canonical(&h)).unwrap_or_default();
    let start = if home.is_empty() { location.map(|l| canonical(&l)).unwrap_or_default() } else { home.clone() };
    o.home = home;
    if start.is_empty() {
        o.tree = Tree::Failed;
        return tx.instant();
    }
    navigate(s, start, tx);
}

/// `inferHomeDirectory`: `/Users/<you>`, `/home/<you>`, `/root` or `C:/Users/<you>` that `dir`
/// starts with.
fn infer_home(dir: &str) -> String {
    canonical(&opencode_kit::path::infer_home(dir))
}

/// The tree opens on `root`, which is selected (`navigate`).
pub fn navigate(s: &mut App, root: String, tx: &mut Tx) {
    let Picker::Open(o) = &mut *s.remote_picker else { return };
    let attempt = o.seq.next();
    let root = canonical(&root);
    o.tree = Tree::Browsing { root: root.clone(), nodes: BTreeMap::from([(root.clone(), Node::Loading { attempt })]), expanded: BTreeSet::new(), selected: None };
    o.suggest = Suggest::Closed;
    tx.instant();
    list(s, root, attempt);
}

/// A folder's row: it's selected, and opens or shuts; opening reads it once.
pub fn toggle(s: &mut App, dir: &str, tx: &mut Tx) {
    let Picker::Open(o) = &mut *s.remote_picker else { return };
    let Tree::Browsing { nodes, expanded, selected, .. } = &mut o.tree else { return };
    *selected = Some(dir.to_owned());
    tx.instant();
    if !expanded.remove(dir) {
        expanded.insert(dir.to_owned());
        if matches!(nodes.get(dir), None | Some(Node::Failed)) {
            let attempt = o.seq.next();
            nodes.insert(dir.to_owned(), Node::Loading { attempt });
            list(s, dir.to_owned(), attempt);
        }
    }
}

/// `GET /api/fs/list` for `dir`'s folders, on a worker.
fn list(s: &App, dir: String, attempt: Attempt) {
    let Picker::Open(o) = &*s.remote_picker else { return };
    let conn = o.conn.clone();
    let deliver = s.deliverer();
    crate::plugin_host::tasks::run(move || {
        let dirs = read_dirs(&conn, &dir);
        deliver(Box::new(move |s: &mut App, tx: &mut Tx| listed(s, dir, attempt, dirs, tx)));
    });
}

fn read_dirs(conn: &opencode_kit::client::Server, dir: &str) -> Option<Vec<Dir>> {
    let enc = crate::switcher::encode;
    let v = conn.get(&format!("/api/fs/list?location%5Bdirectory%5D={}&path={}", enc(dir), enc(dir))).ok()?;
    let location = v["location"]["directory"].as_str().unwrap_or(dir).to_owned();
    let mut dirs: Vec<Dir> = v["data"]
        .as_array()?
        .iter()
        .filter(|e| e["type"].as_str() == Some("directory"))
        .filter_map(|e| {
            let absolute = absolute_path(e["path"].as_str()?, &location);
            Some(Dir { name: opencode_kit::path::basename(&absolute).to_owned(), absolute })
        })
        .collect();
    dirs.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    Some(dirs)
}

fn listed(s: &mut App, dir: String, attempt: Attempt, dirs: Option<Vec<Dir>>, tx: &mut Tx) {
    let Picker::Open(o) = &mut *s.remote_picker else { return };
    match &mut o.suggest {
        Suggest::Searching { attempt: a, query } if *a == attempt => {
            let prefix = query.rsplit('/').next().unwrap_or_default().to_lowercase();
            let items: Vec<String> = dirs.unwrap_or_default().into_iter().filter(|d| d.name.to_lowercase().starts_with(&prefix)).take(5).map(|d| d.absolute).collect();
            o.suggest = if items.is_empty() { Suggest::Closed } else { Suggest::Shown { items, active: 0 } };
            return tx.instant();
        }
        _ => {}
    }
    let Tree::Browsing { nodes, .. } = &mut o.tree else { return };
    if nodes.get(&dir) != Some(&Node::Loading { attempt }) {
        return;
    }
    nodes.insert(dir, dirs.map_or(Node::Failed, Node::Loaded));
    tx.instant();
}

/// The path field changed: its folder's matching folders are suggested.
pub fn typed(s: &mut App, tx: &mut Tx) {
    let Picker::Open(o) = &mut *s.remote_picker else { return };
    let value = o.input.text.trim().to_owned();
    tx.instant();
    if value.is_empty() {
        o.suggest = Suggest::Closed;
        return;
    }
    let base = o.tree.result().map(str::to_owned).unwrap_or_else(|| o.home.clone());
    let absolute = resolve(&value, &o.home, &base);
    let parent = if value.ends_with('/') || value.ends_with('\\') { absolute.clone() } else { opencode_kit::path::parent(&absolute).to_owned() };
    let attempt = o.seq.next();
    o.suggest = Suggest::Searching { attempt, query: absolute };
    list(s, canonical(&parent), attempt);
}

/// `pickerAbsoluteInput`: `~` and `~/…` from home, a rooted path as it is, else from `base`.
fn resolve(value: &str, home: &str, base: &str) -> String {
    let value = value.replace('\\', "/");
    if value == "~" {
        return home.to_owned();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return absolute_path(rest, home);
    }
    absolute_path(&value, base)
}

/// The path field's keys: ↑ ↓ move through suggestions, ↩ opens the highlighted one or the
/// typed path, Escape closes the suggestions, then the dialog.
pub fn key(s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    let Picker::Open(o) = &mut *s.remote_picker else { return false };
    if o.input.is_composing() {
        return false;
    }
    match (&mut o.suggest, &ev.key) {
        (Suggest::Shown { items, active }, rune::Key::Down | rune::Key::Up) => {
            let n = items.len();
            *active = if ev.key == rune::Key::Down { (*active + 1) % n } else { (*active + n - 1) % n };
            tx.instant();
            true
        }
        (Suggest::Shown { items, active }, rune::Key::Enter) => {
            let path = items[*active].clone();
            choose_input(s, path, tx);
            true
        }
        (Suggest::Shown { .. } | Suggest::Searching { .. }, rune::Key::Escape) => {
            o.suggest = Suggest::Closed;
            tx.instant();
            true
        }
        (_, rune::Key::Enter) if !o.input.text.trim().is_empty() => {
            let base = o.tree.result().map(str::to_owned).unwrap_or_else(|| o.home.clone());
            let path = resolve(o.input.text.trim(), &o.home, &base);
            choose_input(s, path, tx);
            true
        }
        (_, rune::Key::Escape) => {
            close(s, tx);
            true
        }
        _ => false,
    }
}

/// A suggestion or a typed path: the tree opens on it (`directory.chosen`, `fromInput`).
pub fn choose_input(s: &mut App, path: String, tx: &mut Tx) {
    if let Picker::Open(o) = &mut *s.remote_picker {
        o.input = TextEdit::new("");
        o.input.focused = true;
    }
    navigate(s, path, tx);
}

/// Select folder: the pick ends with the folder.
pub fn resolve_pick(s: &mut App, tx: &mut Tx) {
    let Picker::Open(o) = std::mem::take(&mut *s.remote_picker) else { return };
    let Some(path) = o.tree.result().map(str::to_owned) else {
        *s.remote_picker = Picker::Open(o);
        return;
    };
    tx.instant();
    // The server's own separators: a Windows server's folder in backslashes, as it lists them.
    let path = if path.as_bytes().get(1) == Some(&b':') { path.replace('/', "\\") } else { path };
    (o.on_pick)(s, path, tx);
}

/// Cancel, Escape, or a press outside: the pick ends with nothing.
pub fn close(s: &mut App, tx: &mut Tx) {
    if matches!(*s.remote_picker, Picker::Open(_)) {
        *s.remote_picker = Picker::Closed;
        tx.instant();
    }
}

/// Each poll: a picker whose server left the list closes (`picker.tsx:61-63`).
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Picker::Open(o) = &*s.remote_picker else { return false };
    let key = o.server.clone();
    if crate::servers::runtime::listed().iter().any(|l| l.key == key) {
        return false;
    }
    close(s, tx);
    true
}

impl Open {
    /// The folder's row shows "git repo" (`renderRowDecoration`).
    pub fn git(&self, dir: &str) -> bool {
        self.git.contains(&canonical(dir))
    }
}

/// `displayPickerPath`: a Windows path with backslashes, else `~` for home.
pub fn display(path: &str, home: &str) -> String {
    let windows = |p: &str| p.as_bytes().get(1) == Some(&b':');
    if windows(path) || windows(home) {
        return path.replace('/', "\\");
    }
    if !home.is_empty() && path == home {
        return "~".into();
    }
    match path.strip_prefix(&format!("{home}/")).filter(|_| !home.is_empty()) {
        Some(rest) => format!("~/{rest}"),
        None => path.to_owned(),
    }
}

/// `shortenPickerRecentPath`: the last two folders after `~/…/`, `/<first>/…/` or the drive.
pub fn shorten(path: &str, home: &str) -> String {
    let shown = display(path, home);
    let keep = |prefix: &str, rest: &str, sep: &str| {
        let parts: Vec<&str> = rest.split(sep).filter(|p| !p.is_empty()).collect();
        if parts.len() <= 2 { None } else { Some(format!("{prefix}…{sep}{}", parts[parts.len() - 2..].join(sep))) }
    };
    if let Some(rest) = shown.strip_prefix("~/") {
        return keep("~/", rest, "/").unwrap_or(shown);
    }
    if shown.starts_with('/') && !shown.starts_with("//") {
        let rest = &shown[1..];
        let first = rest.split('/').next().unwrap_or_default();
        return keep(&format!("/{first}/"), rest.trim_start_matches(first), "/").unwrap_or(shown);
    }
    if shown.as_bytes().get(1) == Some(&b':') && shown.split('\\').filter(|p| !p.is_empty()).count() > 3 {
        let drive = &shown[..3];
        return keep(drive, &shown[3..], "\\").unwrap_or(shown);
    }
    shown
}

/// `pickerRoot`: `/`, `C:/`, `//server/share`.
pub fn root_of(path: &str) -> String {
    if path.starts_with("//") {
        return path.splitn(5, '/').take(4).collect::<Vec<_>>().join("/");
    }
    if path.as_bytes().get(1) == Some(&b':') {
        return format!("{}/", &path[..2]);
    }
    "/".into()
}

/// `pickerParent`.
pub fn parent_of(path: &str) -> String {
    let root = root_of(path);
    if path == root || path.len() <= root.len() {
        return root;
    }
    match path.rfind('/') {
        Some(i) if i >= root.len() => path[..i].to_owned(),
        _ => root,
    }
}

// ---------------------------------------------------------------------------------------------
// The dialog (`dialog.tsx:574-736`, `dialog.css`): 640 wide (`size="large"`), its title and the
// server's name, a hairline, then the path row, the recent projects and the folder tree, and
// the selection beside Cancel and Select folder.

pub mod view {
    use rune::{Color, El, Tx, col, el, icon, row, text};
    use rune_kit::{ButtonVariant, TextInput};
    use rune_theme_opencode::icons as ic;

    use super::{Node, Open, Picker, Suggest, Tree};
    use crate::App;
    use crate::palette::{Elevate as _, Pal, kit};
    use crate::view::{UI, UI_MED};
    use opencode_i18n::t;

    /// One tree row (`--trees-item-height`) and the indent each level adds.
    const ROW_H: f32 = 24.0;
    const INDENT: f32 = 16.0;

    pub fn view(s: &App, p: &Pal) -> Vec<El> {
        let Picker::Open(o) = &*s.remote_picker else { return Vec::new() };
        let k = kit(p);
        // `dialog-header`: the title and the server under it, and the 20 pt close button.
        let titles = col()
            .key("directory-picker-titles")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .gap(8.0)
            .child(text(o.title.clone()).font(rune::TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04)).color(p.text))
            .child(text(o.subtitle.clone()).font(rune::TextStyle::new(11.0, 440.0).lh(12.0).tracking(0.05)).color(p.muted).truncate().dir(rune::Direction::Ltr));
        let close = row()
            .key("directory-picker-close")
            .role("button")
            .label_static(t!("common.close"))
            .size(20.0, 20.0)
            .mt(-2.0)
            .mr(-2.0)
            .radius(4.0)
            .center()
            .hover(p.hover)
            .on_press_tx(|s: &mut App, tx: &mut Tx| super::close(s, tx))
            .child(icon(&ic::CLOSE, 20.0).color(p.icon_muted));
        let header = row().key("directory-picker-header").w_full().padding(16.0).items_start().child(titles).child(close);
        let input = TextInput::new(&o.input)
            .placeholder(t!("dialog.directory.search.placeholder"))
            .label(t!("dialog.directory.search.placeholder"))
            .width(None)
            .spellcheck(false);
        // `showClearButton`: only while something is typed.
        let input = if o.input.text.is_empty() { input } else { input.clear_button() };
        let input = input
            .on_edit(|s: &mut App| match &mut *s.remote_picker {
                Picker::Open(o) => &mut o.input,
                Picker::Closed => unreachable!("the picker's field outlives its dialog"),
            })
            .on_change(|s: &mut App, tx: &mut Tx| super::typed(s, tx))
            .on_key(|s: &mut App, ev, tx| super::key(s, ev, tx))
            .build(k)
            .key("directory-picker-input")
            .grow(1.0)
            .shrink()
            .min_w(0.0);
        let base = o.tree.result().map(str::to_owned).unwrap_or_default();
        let home = if o.home.is_empty() { base.clone() } else { o.home.clone() };
        let (root, parent) = (super::root_of(&base), super::parent_of(&base));
        let ghost = |key: &str, label: String, to: String| {
            crate::widgets::button(label, ButtonVariant::Ghost).on_press_tx(move |s: &mut App, tx: &mut Tx| super::navigate(s, to.clone(), tx)).build(k).key(key.to_owned())
        };
        let mut path = row()
            .key("directory-picker-path")
            .w_full()
            .gap(8.0)
            .center_y()
            .child(input)
            .child(row().key("directory-picker-actions").gap(2.0).child(ghost("picker-home", "~".into(), home)).child(ghost("picker-root", t!("dialog.directory.root").into(), root)).child(ghost("picker-parent", t!("dialog.directory.parent").into(), parent)));
        if let Suggest::Shown { items, active } = &o.suggest {
            let mut list = col().key("directory-picker-suggestions").role("listbox").pin(Some(0.0), Some(34.0), Some(0.0), None).padding(4.0).radius(6.0).fill(p.layer2).box_shadow(crate::palette::Pal::ring_in(p.border)).elevate(p.overlay);
            for (i, item) in items.iter().enumerate() {
                let to = item.clone();
                let lit = i == *active;
                list = list.child(
                    row()
                        .key(format!("suggestion-{i}"))
                        .role("option")
                        .w_full()
                        .px(8.0)
                        .py(6.0)
                        .radius(4.0)
                        .fill(if lit { p.hover } else { Color::TRANSPARENT })
                        .hover(p.hover)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| super::choose_input(s, to.clone(), tx))
                        .child(text(format!("{}/", super::display(item, &o.home))).font(UI).color(if lit { p.text } else { p.muted }).truncate().dir(rune::Direction::Ltr)),
                );
            }
            path = path.child(list.raise());
        }
        let mut body = col().key("directory-picker-body").w_full().grow(1.0).min_h(0.0).gap(12.0).pt(14.0).px(16.0).child(path);
        if !o.recent.is_empty() {
            let mut chips = row().key("directory-picker-recent-list").w_full().gap(8.0).wrap();
            for proj in &o.recent {
                let to = proj.directory.clone();
                let name = proj.name.clone();
                let full = super::display(&proj.directory, &o.home);
                let avatar = crate::icons::badge(s, p, Some(proj), &proj.name);
                let chip = crate::widgets::button(name.clone(), ButtonVariant::Neutral)
                    .label_with(move |font, color| row().gap(6.0).center_y().max_w(214.0).child(avatar).child(text(name).font(font).color(color).truncate().shrink().min_w(0.0)))
                    .help(full)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| super::navigate(s, to.clone(), tx))
                    .build(k)
                    .key(format!("recent-{}", proj.id));
                chips = chips.child(chip);
            }
            body = body.child(col().key("directory-picker-recent").w_full().gap(8.0).child(text(t!("dialog.directory.recent")).font(UI_MED).color(p.text)).child(chips));
        }
        body = body.child(browser(o, p));
        let selection = o.tree.result().map(|r| super::shorten(r, &o.home)).unwrap_or_default();
        let full = o.tree.result().map(|r| super::display(r, &o.home)).unwrap_or_default();
        let picked = o.tree.result().is_some();
        let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| super::close(s, tx)).build(k).key("directory-picker-cancel");
        let select = crate::widgets::button(t!("dialog.directory.action.selectFolder"), ButtonVariant::Contrast).disabled(!picked).on_press_tx(|s: &mut App, tx: &mut Tx| super::resolve_pick(s, tx)).build(k).key("directory-picker-select");
        let mut chosen = text(selection.clone()).key("directory-picker-selection").font(UI).color(p.faint).truncate().dir(rune::Direction::Ltr).grow(1.0).shrink().min_w(0.0);
        if full != selection {
            chosen = chosen.help(full);
        }
        let footer = row().key("directory-picker-footer").w_full().padding(16.0).gap(8.0).center_y().child(chosen).child(cancel).child(select);
        // `size="large"`: 640 by 480.
        let card = col()
            .w(640.0)
            .h(480.0)
            .radius(6.0)
            .fill(p.layer1)
            .elevate(p.overlay)
            // The clip on a child: on the card it would clip the card's own shadow too.
            .child(col().w_full().h_full().clip_rounded(6.0)
            .child(header)
            // `divider-v2`: a 1 point rule drawn at half its height (`scaleY(0.5)`), centred in it.
            .child(col().w_full().h(1.0).py(0.25).child(el().w_full().h(0.5).fill(p.q.border_strong)))
            .child(body)
            .child(footer));
        let card = col().key("directory-picker").role("dialog").label(o.title.clone()).modal(true, |s: &mut App| super::close(s, &mut Tx::default())).child(card);
        crate::widgets::modal("directory-picker-modal", p, card, None)
    }

    /// `directory-picker-browser`: 280 tall, 6 in, a 0.5 pt border (a whole device pixel that
    /// takes its room, as a CSS border does), 6 pt corners on bg-base; the root's folders as a
    /// tree, or Loading, or why it can't be read.
    fn browser(o: &Open, p: &Pal) -> El {
        let frame = col().key("directory-picker-browser").w_full().grow(1.0).min_h(0.0).padding(6.0).radius(6.0).fill(p.bg_base).border(0.5, p.border).border_layout();
        let state = |words: &str| frame.clone().center().child(text(words.to_owned()).font(UI).color(p.muted));
        let Tree::Browsing { root, nodes, expanded, selected } = &o.tree else {
            return if o.tree == Tree::Failed { state(t!("dialog.directory.readError")) } else { state(t!("common.loading")) };
        };
        match nodes.get(root) {
            Some(Node::Loading { .. }) | None => return state(t!("common.loading")),
            Some(Node::Failed) => return state(t!("dialog.directory.readError")),
            Some(Node::Loaded(_)) => {}
        }
        let mut rows = Vec::new();
        let mut out: Vec<(super::Dir, usize)> = Vec::new();
        flatten(root, 0, nodes, expanded, &mut out);
        for (d, depth) in out {
            let open = expanded.contains(&d.absolute);
            let lit = selected.as_deref() == Some(d.absolute.as_str());
            let to = d.absolute.clone();
            let failed = matches!(nodes.get(&d.absolute), Some(Node::Failed));
            let mut r = row()
                .key(format!("dir-{}", d.absolute))
                .role("treeitem")
                .label(d.name.clone())
                .expanded(open)
                .w_full()
                .h(ROW_H)
                .pl(8.0 + depth as f32 * INDENT)
                .pr(8.0)
                .gap(6.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| super::toggle(s, &to, tx))
                .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl())
                .child(icon(&ic::FOLDER, 14.0).color(p.icon_muted))
                .child(text(d.name.clone()).font(if lit { UI_MED } else { UI }).color(if lit { p.text } else if failed { p.faint } else { p.muted }).truncate().shrink().min_w(0.0));
            if o.git(&d.absolute) {
                r = r.child(row().key("git").h(16.0).px(4.0).radius(2.0).center_y().fill(p.layer3).child(text(t!("dialog.directory.gitRepo")).font(rune::TextStyle::new(11.0, 530.0).lh(12.0).tracking(0.05)).color(p.muted)));
            }
            if lit {
                r = r.fill(p.pressed).box_shadow(Pal::ring_in(p.border));
            }
            rows.push(r);
        }
        // Desktop's Pierre tree scrolls itself with its own bar (`workspaces/selection/dialog.tsx:533`),
        // its `--trees-fg` left at the tree's default, text-base on bg-base.
        let bar = session_view::scroller::Bar::Tree { fg: p.text, bg: p.bg_base };
        frame.child(session_view::scroller::scroller("directory-picker-tree").bar(bar).build::<App>(p, col().role("tree").w_full().grow(1.0).min_h(0.0), col().w_full().children(rows)))
    }

    /// The open folders' rows, each folder's children right under it.
    fn flatten(dir: &str, depth: usize, nodes: &std::collections::BTreeMap<String, Node>, expanded: &std::collections::BTreeSet<String>, out: &mut Vec<(super::Dir, usize)>) {
        let Some(Node::Loaded(children)) = nodes.get(dir) else { return };
        for c in children {
            out.push((c.clone(), depth));
            if expanded.contains(&c.absolute) {
                flatten(&c.absolute, depth + 1, nodes, expanded, out);
            }
        }
    }
}
