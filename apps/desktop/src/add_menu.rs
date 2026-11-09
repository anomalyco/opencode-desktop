//! The composer's + menu and what it opens, as in Desktop (`composer/editor/editor.tsx`
//! `ComposerEditorAddMenu`, `suggestions/machine.ts`, `composer/submit.ts`): ⌘U's file picker,
//! the `/` command popover, `@` context and `!` shell mode.
//!
//! A `/name` the server knows sends its arguments to `POST …/command`; shell mode sends the
//! composer's text to `POST …/shell` and can't wait in the queue.

use std::collections::HashMap;

use rune::{Color, El, Key, KeyEvent, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;
use session_view::composer::mode::{self, Effect, Event, Mode};

use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::Delivery;

use crate::palette::{Elevate as _, Pal};
use crate::view::UI;
use crate::slash::Slash;
use crate::{App, Route, Source};
use opencode_i18n::t;

const SHORTCUT: rune::TextStyle = rune::TextStyle::new(11.0, 530.0).lh(11.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Attach,
    Commands,
    Context,
    Shell,
}

impl Item {
    pub const ALL: [Item; 4] = [Item::Attach, Item::Commands, Item::Context, Item::Shell];

    fn key(self) -> &'static str {
        match self {
            Item::Attach => "attach",
            Item::Commands => "commands",
            Item::Context => "context",
            Item::Shell => "shell",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Item::Attach => t!("ui.promptInput.attachments"),
            Item::Commands => t!("ui.promptInput.commands"),
            Item::Context => t!("ui.promptInput.context"),
            Item::Shell => t!("ui.promptInput.shell"),
        }
    }

    fn shortcut(self) -> String {
        match self {
            Item::Attach => crate::keymap::Bind::Attach.label(),
            Item::Commands => "/".into(),
            Item::Context => "@".into(),
            Item::Shell => "!".into(),
        }
    }
}

/// The + menu: shut, or open with the item hovered or moved to, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Menu {
    #[default]
    Closed,
    Open { active: Option<Item> },
}

#[derive(Default)]
pub struct AddMenu {
    pub menu: Menu,
    /// The open draft's mode: prompts or shell commands (see [`mode`]).
    pub mode: Mode,
    /// The other drafts' shell modes, by draft key: Desktop keeps the mode in each draft.
    parked: HashMap<String, Mode>,
    pub slash: Slash,
}

impl AddMenu {
    pub fn open(&self) -> bool {
        matches!(self.menu, Menu::Open { .. })
    }

    fn active(&self) -> Option<Item> {
        match self.menu {
            Menu::Open { active } => active,
            Menu::Closed => None,
        }
    }

    /// The composer sends shell commands instead of prompts.
    pub fn shell(&self) -> bool {
        self.mode == Mode::Shell
    }

    /// The composer moves from draft `from` to draft `to`: each keeps its own mode.
    pub fn swap_draft(&mut self, from: Option<&str>, to: Option<&str>) {
        if let Some(from) = from {
            if self.shell() { self.parked.insert(from.to_owned(), Mode::Shell); } else { self.parked.remove(from); }
        }
        let next = to.and_then(|k| self.parked.remove(k)).unwrap_or_default();
        mode::step(&mut self.mode, Event::Synced(next));
    }
}

impl App {
    /// The open page has a composer: a draft, or a session that isn't a subagent's.
    pub fn has_composer(&self) -> bool {
        match &self.route {
            Route::Draft(_) => true,
            Route::Session(id) => self.session(id).is_none_or(|i| i.parent.is_none()),
            Route::Home => false,
        }
    }

    pub fn open_add_menu(&mut self) {
        if self.add.shell() {
            return;
        }
        self.close_mentions();
        self.add.menu = Menu::Open { active: None };
    }

    pub fn close_add_menu(&mut self) {
        self.add.menu = Menu::Closed;
    }

    pub fn choose_add(&mut self, item: Item, tx: &mut Tx) {
        tx.instant();
        self.close_add_menu();
        match item {
            Item::Attach => self.pick_attachment(tx),
            Item::Commands => {
                self.composer.focused = true;
                if self.composer.text.trim().is_empty() {
                    self.composer.clear();
                }
                self.add.slash.closed = None;
                self.insert_trigger("/");
            }
            Item::Context => {
                self.composer.focused = true;
                self.insert_trigger("@");
                self.sync_mentions();
            }
            Item::Shell => {
                tx.animated();
                self.enter_shell();
            }
        }
    }

    /// Puts `trigger` at the caret, after a space unless it starts the text or a word.
    fn insert_trigger(&mut self, trigger: &str) {
        let caret = self.composer.range().1;
        let before = self.composer.text[..caret].chars().next_back();
        let space = before.is_some_and(|c| !c.is_whitespace());
        self.composer.set_caret(caret);
        self.composer.insert(&format!("{}{trigger}", if space { " " } else { "" }));
    }

    /// Steps the composer's mode and runs what it asks; true when it took the key.
    pub fn composer_mode(&mut self, event: Event) -> bool {
        let step = mode::step(&mut self.add.mode, event);
        for effect in step.effects {
            match effect {
                Effect::ClosePopovers => {
                    self.close_mentions();
                    self.close_add_menu();
                }
                Effect::ClearDraft => self.composer.clear(),
            }
        }
        step.handled
    }

    pub fn enter_shell(&mut self) {
        self.composer_mode(Event::Shell);
        self.composer.focused = true;
    }

    pub fn exit_shell(&mut self) {
        self.composer_mode(Event::Normal);
    }

    /// The draft became exactly `!`: prompt mode switches to shell (`inputChanged`).
    pub fn composer_typed(&mut self) -> bool {
        let bang = self.composer.text == "!" && self.composer.preedit().is_none();
        self.composer_mode(Event::Input { bang })
    }

    /// Keys for the + menu, ⌘U, shell mode and the command popover. The composer's handler
    /// and the window's both call it.
    pub fn add_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if let Menu::Open { active } = self.add.menu {
            let step = |i: Option<Item>, d: i64| {
                let n = Item::ALL.len() as i64;
                let at = i.and_then(|i| Item::ALL.iter().position(|x| *x == i)).map_or(if d > 0 { -1 } else { 0 }, |p| p as i64);
                Item::ALL[(at + d).rem_euclid(n) as usize]
            };
            match ev.key {
                Key::Escape => {
                    tx.instant();
                    self.close_add_menu();
                    self.composer.focused = true;
                }
                Key::Down => self.add.menu = Menu::Open { active: Some(step(active, 1)) },
                Key::Up => self.add.menu = Menu::Open { active: Some(step(active, -1)) },
                Key::Enter => {
                    if let Some(item) = active {
                        self.choose_add(item, tx);
                    }
                }
                _ => {}
            }
            return true;
        }
        if crate::keymap::Bind::Attach.matches_tx(ev, tx) && self.has_composer() {
            if !self.add.shell() {
                self.close_mentions();
                self.pick_attachment(tx);
            }
            return true;
        }
        if !self.composer.focused {
            return false;
        }
        if self.slash_key(ev, tx) {
            return true;
        }
        // With no list open, Escape and Backspace reach the machine, which leaves shell mode.
        let event = match ev.key {
            Key::Escape => Event::Escape,
            Key::Backspace => Event::Backspace { empty: self.composer.text.is_empty() },
            _ => return false,
        };
        if self.composer.preedit().is_some() || self.mentions.open || !self.composer_mode(event) {
            return false;
        }
        // The + button and the pickers fade back in (`editor.tsx:178-182`).
        tx.animated();
        true
    }

    /// Runs a `/command` prompt: in the open session (steered or queued while it works, by
    /// the server), or in a new one from a draft.
    pub fn send_command(&mut self, mut prompt: Prompt, alternate: bool, tx: &mut Tx) {
        match self.route.clone() {
            Route::Session(_) if matches!(self.source, Source::Fixture) => {
                crate::status_toast::say(self, t!("rune.composer.commandsNeedServer"), t!("rune.composer.needServer.description"));
                return;
            }
            Route::Session(id) => {
                if self.busy(&id) {
                    let queue = alternate != (self.settings.follow_up == crate::settings::FollowUp::Queue);
                    prompt.delivery = Some(if queue { Delivery::Queue } else { Delivery::Steer });
                }
                let staged = self.session(&id).and_then(|i| i.revert.as_ref()).is_some();
                let w = if staged {
                    Write::SendFromRevert { session: id.clone(), prompt }
                } else {
                    Write::Send { session: id.clone(), prompt }
                };
                if !self.write(w) {
                    return;
                }
                if staged {
                    self.commit_staged_revert(&id);
                }
            }
            Route::Draft(_) if self.live() => {
                if !self.write(Write::Start { directory: self.draft_dir().to_owned(), prompt }) {
                    return;
                }
                self.status = t!("rune.composer.starting").into();
            }
            _ => {
                crate::status_toast::say(self, t!("rune.composer.openSession"), "");
                return;
            }
        }
        tx.instant();
        self.composer.clear();
        self.attach.files.clear();
    }

    /// Sends shell mode's command: now, in the open session, never queued.
    pub fn submit_shell(&mut self, alternate: bool, tx: &mut Tx) {
        let command = self.composer.text.trim().to_owned();
        if command.is_empty() {
            return;
        }
        let Route::Session(id) = self.route.clone() else {
            crate::status_toast::say(self, t!("rune.composer.shell.needsSession"), t!("rune.composer.shell.needsSession.description"));
            return;
        };
        if alternate && self.busy(&id) {
            crate::status_toast::say(self, t!("rune.composer.shell.noQueue"), t!("rune.composer.shell.noQueue.description"));
            return;
        }
        if matches!(self.source, Source::Fixture) {
            crate::status_toast::say(self, t!("rune.composer.shell.needsServer"), t!("rune.composer.needServer.description"));
            return;
        }
        let w = Write::Shell { session: id, id: Some(opencode_kit::client::message_id()), command };
        if self.write(w) {
            tx.instant();
            self.composer.clear();
            self.exit_shell();
        }
    }
}

/// The + button at the start of the toolbar: `icon-button-v2` ghost-muted at size large
/// (`editor.tsx:977-987`, `icon-button.css:36-39,115-142`), pressed with its icon in
/// `icon-icon-base` while the menu is open. Shell mode fades it out with the pickers
/// ([`shell_hidden`]). The pickers start 4 after it (`ms-1`): the toolbar's 2 pt gap and its
/// own 2.
pub fn trigger(s: &App, p: &Pal) -> El {
    let open = s.add.open();
    let mut b = el()
        .key("add")
        .role("button")
        .label(t!("ui.promptInput.add"))
        .help(t!("ui.promptInput.add")).help_keys(crate::keymap::Bind::Attach.keycaps())
        .size(28.0, 28.0)
        .mr(2.0)
        .radius(6.0)
        .center()
        .child(icon(&ic::PLUS, 16.0).color(if open { p.icon } else { p.icon_muted }))
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            if s.add.open() { s.close_add_menu() } else { s.open_add_menu() }
            crate::popover::motion::toggled(&s.knobs, tx, s.add.open());
        });
    if open {
        b = b.fill(p.pressed);
    }
    shell_hidden(s, b)
}

/// The + button and the pickers in shell mode: still in their places, faded out over 200 ms
/// and out of reach of the pointer, Tab and assistive technology (`editor.tsx:178-182`,
/// `aria-hidden` and `inert` at 426-460).
pub fn shell_hidden(s: &App, el: El) -> El {
    let el = el.opacity_motion(rune::Anim::new(rune::Curve::tween(0.2, rune::Easing::EASE)));
    if s.add.shell() { el.opacity(0.0).inert().a11y_hidden() } else { el.opacity(1.0) }
}

/// "Exit Esc" before Send in shell mode (`editor.tsx:499-516`): `button-v2` ghost-faint at size
/// small (`button.css:46-50,304-326`), 24 tall, 6 in and the word 6 from the keys, 4 pt
/// corners, 12 before Send (`me-3`: the toolbar's 2 pt gap and 10), its ring 2 inside
/// (`editor.css:70-73`). The keys show only in a window at least 640 wide (`hidden sm:block`).
/// It goes back to prompts and the caret to the composer.
pub fn exit_shell_button(p: &Pal, wide: bool) -> El {
    let word = text(t!("ui.promptInput.exitShell")).font(crate::view::UI_MED).color(p.faint);
    row()
        .key("exit-shell")
        .role("button")
        .label(t!("ui.promptInput.exitShell"))
        .h(24.0)
        .px(6.0)
        .mr(10.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .focus_ring(2.0, -2.0, p.v2.border_focus)
        // It shows and goes at once with all it holds, as Desktop's `<Show>` does.
        .transition(rune::Transition::none())
        .instant_changes()
        .on_press_tx(|s: &mut App, _: &mut Tx| {
            s.exit_shell();
            s.composer.focused = true;
        })
        .child(word)
        .children(wide.then(|| crate::keymap::chips(p, vec![t!("common.key.esc").to_owned()])))
}

/// The menu over the + button, and the scrim that closes it.
pub fn menu(s: &App, p: &Pal) -> Option<El> {
    if !s.add.open() {
        return None;
    }
    // `menu-v2-content` (`menu.css:6-21`): layer 1, a 2 pt inset, 6 pt corners and
    // `--v2-elevation-floating`.
    let mut menu = col()
        .key("add-menu")
        .role("menu")
        .label(t!("ui.common.add"))
        .above("add", 6.0)
        .min_w(180.0)
        .padding(2.0)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(crate::popover::motion::transition(&s.knobs, rune::Side::Top, rune::Align::Start))
        // What it holds comes in with it, not on fades of its own.
        .instant_changes()
        .on_press(|_: &mut App| {});
    for item in Item::ALL {
        if item == Item::Commands {
            // `menu-v2-separator`: edge to edge, through the menu's 2 pt inset.
            menu = menu.child(el().key("add-separator").h(1.0).mt(2.0).mb(2.0).ml(-2.0).mr(-2.0).fill(p.border_muted));
        }
        let active = s.add.active() == Some(item);
        menu = menu.child(
            row()
                .key(format!("add-{}", item.key()))
                .role("menuitem")
                .label(item.label())
                .h(28.0)
                .w_full()
                .pl(12.0)
                .pr(6.0)
                .gap(8.0)
                .center_y()
                .radius(4.0)
                .fill(if active { p.hover } else { Color::TRANSPARENT })
                .press(p.pressed)
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if inside && s.add.open() {
                        tx.instant();
                        s.add.menu = Menu::Open { active: Some(item) };
                    }
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.choose_add(item, tx))
                .child(text(item.label()).font(UI).color(p.text))
                .child(spacer())
                .child(row().key("shortcut").min_w(20.0).center().child(text(item.shortcut()).font(SHORTCUT).color(p.faint))),
        );
    }
    Some(
        el().key("add-dismiss")
            .transition(rune::Transition::none())
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.close_add_menu();
            })
            .child(menu),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::{commands, parse_slash};
    use crate::mentions::tests::{key, runtime, shown, tap, typed};
    use rune::Mods;

    fn scratch(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("seer-add-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn the_menu_opens_over_plus_and_context_types_an_at() {
        let mut rt = runtime();
        tap(&mut rt, "add");
        crate::popover::tests::grows_whole(&mut rt, "add-menu");
        let menu = shown(&rt, "add-menu").expect("the menu opens");
        let plus = shown(&rt, "add").unwrap();
        assert!((menu.target[1] + menu.target[3] + 6.0 - plus.target[1]).abs() < 0.1, "6 points above the +");
        assert!((menu.target[0] - plus.target[0]).abs() < 0.1, "starts at its left edge");
        assert!(menu.target[2] >= 180.0);
        tap(&mut rt, "add-context");
        assert!(shown(&rt, "add-menu").is_none());
        assert_eq!(rt.state.composer.text, "@");
        assert!(rt.state.composer.focused);
        assert!(shown(&rt, "mentions").is_some(), "the @ popover opens");
    }

    #[test]
    fn picked_files_attach_inline_or_by_path() {
        let png = scratch("shot.png", b"\x89PNG\r\n\x1a\n");
        let log = scratch("build.log", b"compiling seer\nok\n");
        let mut rt = runtime();
        tap(&mut rt, "add");
        tap(&mut rt, "add-attach");
        assert!(rt.file_dialog().is_some(), "the file picker opens");
        rt.resolve_file_dialog(Some(png.clone())).unwrap();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.attach.files.iter().map(|f| f.mime.as_str()).collect::<Vec<_>>(), ["image/png"]);

        tap(&mut rt, "prompt");
        rt.key(KeyEvent { key: Key::Command('u'), mods: Mods { cmd: true, ..Default::default() } });
        rt.frame(1.0 / 60.0);
        assert!(rt.file_dialog().is_some(), "⌘U opens it too");
        rt.resolve_file_dialog(Some(log.clone())).unwrap();
        rt.frame(1.0 / 60.0);
        let r = &rt.state.attach.paths;
        assert_eq!((r.len(), r[0].name.as_str(), r[0].mime.as_str()), (1, "build.log", "text/plain"));
        assert!(shown(&rt, &format!("path-{}", log.display())).is_some(), "a card shows it");
        std::fs::remove_dir_all(png.parent().unwrap()).ok();
        std::fs::remove_dir_all(log.parent().unwrap()).ok();
    }

    #[test]
    fn path_attachments_go_out_as_lines_and_metadata() {
        let mut app = App::empty();
        let writes = app.fake_writes();
        app.route = Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("what failed?");
        app.attach.add_path(opencode_kit::client::PathAttachment { name: "b.log".into(), mime: "text/plain".into(), path: "/w/b.log".into() });
        app.send(&mut Tx::default());
        let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no prompt") };
        assert_eq!(prompt.text, "what failed?\nAttached file: `/w/b.log`");
        assert_eq!(prompt.display_text.as_deref(), Some("what failed?"));
        assert_eq!(prompt.attachments.len(), 1);
        assert!(app.attach.paths.is_empty());
    }

    #[test]
    fn slash_lists_commands_and_picks_one() {
        let mut rt = runtime();
        tap(&mut rt, "add");
        tap(&mut rt, "add-commands");
        assert_eq!(rt.state.composer.text, "/");
        assert!(shown(&rt, "commands").is_some());
        let names: Vec<String> = rt.state.slash_items().iter().map(|c| c.name.clone()).collect();
        assert_eq!(names[..2], ["init", "review"], "the server's commands lead the app's");
        assert!(names.iter().any(|n| n == "compact"));
        typed(&mut rt, "rev");
        assert_eq!(rt.state.slash_active().as_deref(), Some("review"));
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.composer.text, "/review ");
        assert!(shown(&rt, "commands").is_none(), "a space closes it");
        typed(&mut rt, "/");
        key(&mut rt, Key::Escape);
        assert!(shown(&rt, "commands").is_none());
    }

    /// Each letter after `/` filters the drawn list in the frame it lands: no row slides,
    /// fades or lingers.
    #[test]
    fn filtering_lands_in_the_keys_frame() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        for c in "/rev".chars() {
            key(&mut rt, Key::Text(c.to_string()));
            let want: Vec<String> = rt.state.slash_items().iter().map(|c| format!("command-{}", c.id)).collect();
            let rows: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("command-"))).collect();
            assert_eq!(rows.iter().map(|e| e.key.clone().unwrap()).collect::<Vec<_>>(), want, "after {c}");
            for e in rows {
                let moved = e.rect.iter().zip(e.target).any(|(a, b)| (a - b).abs() > 0.01);
                assert!(!e.exiting && !moved && (e.opacity - 1.0).abs() < 1e-3, "{:?} after {c}: {:?} → {:?}, opacity {}", e.key, e.rect, e.target, e.opacity);
            }
        }
    }

    #[test]
    fn a_known_command_runs_as_a_command() {
        let mut app = App::empty();
        let writes = app.fake_writes();
        app.add.slash.commands = commands(&serde_json::json!({"data": [{"name": "review", "description": ""}]}));
        app.route = Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("/review HEAD~1");
        app.send(&mut Tx::default());
        let Ok(Write::Send { session, prompt }) = writes.try_recv() else { panic!("no command") };
        assert_eq!((session.as_str(), prompt.command.as_deref(), prompt.text.as_str()), ("ses_a", Some("review"), "HEAD~1"));
        assert!(app.composer.text.is_empty());
        // An unknown one is a prompt.
        app.composer = rune::TextEdit::new("/nope x");
        app.send(&mut Tx::default());
        let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no prompt") };
        assert_eq!((prompt.command, prompt.text.as_str()), (None, "/nope x"));
    }

    #[test]
    fn bang_enters_shell_mode_and_backspace_leaves() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        typed(&mut rt, "!");
        assert!(rt.state.add.shell());
        assert_eq!(rt.state.composer.text, "");
        assert!(shown(&rt, "add").is_some_and(|b| b.label.as_deref() == Some("Add images and files")));
        tap(&mut rt, "add");
        assert!(!rt.state.add.open(), "the + is off in shell mode");
        tap(&mut rt, "prompt");
        typed(&mut rt, "l@");
        assert!(!rt.state.mentions.open, "no @ popover in shell mode");
        key(&mut rt, Key::Backspace);
        key(&mut rt, Key::Backspace);
        assert!(rt.state.add.shell());
        key(&mut rt, Key::Backspace);
        assert!(!rt.state.add.shell());
        typed(&mut rt, "a!");
        assert_eq!(rt.state.composer.text, "a!", "! starts shell mode only in an empty composer");
    }

    #[test]
    fn shell_commands_post_now_and_never_queue() {
        let mut app = App::empty();
        let writes = app.fake_writes();
        app.route = Route::Session("ses_a".into());
        app.add.mode = Mode::Shell;
        app.composer = rune::TextEdit::new("ls -la ");
        app.send(&mut Tx::default());
        let Ok(Write::Shell { session, id, command }) = writes.try_recv() else { panic!("no shell") };
        assert_eq!((session.as_str(), command.as_str()), ("ses_a", "ls -la"));
        assert!(id.is_some_and(|i| i.starts_with("msg_")));
        assert!(!app.add.shell(), "back to prompts after sending");
        assert!(app.composer.text.is_empty());
    }

    #[test]
    fn parses_slash_commands() {
        assert_eq!(parse_slash("/review  HEAD~1 x "), Some(("review", "HEAD~1 x")));
        assert_eq!(parse_slash("/init"), Some(("init", "")));
        assert_eq!(parse_slash("init"), None);
    }
}
