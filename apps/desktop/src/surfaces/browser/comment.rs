//! Floating element-comment spotlight + editor (`Comment on part of the page`) and whole-page
//! note editor (`Add page to session`), matching official `packages/gui-plugins/src/browser/`
//! (`panel.tsx:1525-1665`, `comment.ts`, `comment.css`, `overlay-machine.ts`).

use std::cell::Cell;
use std::rc::Rc;

use browser_pane::PickedElement;
use opencode_feature_annotations::quote::comment::{self, Config, Field, FieldEffect, FieldEvent, Mention};
use opencode_feature_annotations::quote::editor::{
    self, CHECK, FollowUp, PathSearch, key_of, mention_rows, neutral_lift, prepare_path_search,
    request_path_search,
};
use opencode_feature_annotations::quote::QuoteHost as _;
use opencode_ui::palette::Elevate as _;
use rune::machine::{Attempt, Seq};
use rune::{
    Color, Cursor, El, KeyEvent, Mods, Spring, TextEdit, TextStyle, Transition, Tx, Waker, col, el,
    icon, row, text,
};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::{Pal, kit};
use crate::panels::panel_key;

use super::PLUGIN;

const GAP: f32 = 8.0;
const ELEMENT_EDITOR_W: f32 = 320.0;
const PAGE_EDITOR_W: f32 = 400.0;
const FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const MONO: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0).mono();

pub enum Overlay {
    Idle,
    Commenting {
        tab_id: u64,
        url: String,
        element: PickedElement,
        draft: TextEdit,
        field: Field,
        editor_h: Cell<f32>,
        editor_initial_h: Cell<f32>,
    },
    Noting {
        tab_id: u64,
        url: String,
        title: String,
        draft: TextEdit,
        field: Field,
    },
}

impl Default for Overlay {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Default)]
pub struct BrowserOverlay {
    pub state: Overlay,
    pub(super) describe: super::comment_describe::PendingDescribe,
    seq: Seq,
    mention_fetch: rune::Latest<(Attempt, Option<Vec<String>>)>,
    spare: TextEdit,
    pane_size: Cell<[f32; 2]>,
}

impl BrowserOverlay {
    pub fn set_waker(&mut self, waker: Waker) {
        self.mention_fetch.set_waker(waker);
    }

    pub fn set_pane_size(&self, w: f32, h: f32) {
        self.pane_size.set([w, h]);
    }

    pub fn active_for(&self, tab_id: u64) -> bool {
        match &self.state {
            Overlay::Idle => false,
            Overlay::Commenting { tab_id: id, .. } | Overlay::Noting { tab_id: id, .. } => *id == tab_id,
        }
    }

    pub fn commenting_for(&self, tab_id: u64) -> bool {
        matches!(&self.state, Overlay::Commenting { tab_id: id, .. } if *id == tab_id)
    }

    pub(super) fn commenting_backend_id(&self, tab_id: u64) -> Option<i64> {
        match &self.state {
            Overlay::Commenting { tab_id: id, element, .. } if *id == tab_id => Some(element.backend_node_id),
            _ => None,
        }
    }

    pub(super) fn apply_described(&mut self, tab_id: u64, obj: &serde_json::Map<String, serde_json::Value>) -> bool {
        let Overlay::Commenting { tab_id: id, element, editor_initial_h, .. } = &mut self.state else {
            return false;
        };
        if *id != tab_id {
            return false;
        }
        let got_rect = super::comment_describe::apply_object(element, obj);
        if got_rect {
            editor_initial_h.set(0.0);
        }
        got_rect
    }

    pub(super) fn apply_rect(&mut self, tab_id: u64, rect: [f32; 4]) -> bool {
        let Overlay::Commenting { tab_id: id, element, editor_initial_h, .. } = &mut self.state else {
            return false;
        };
        if *id != tab_id {
            return false;
        }
        element.rect = rect;
        editor_initial_h.set(0.0);
        true
    }

    pub fn version_tag(&self) -> (u8, u64) {
        match &self.state {
            Overlay::Idle => (0, 0),
            Overlay::Commenting { tab_id, .. } => (1, *tab_id),
            Overlay::Noting { tab_id, .. } => (2, *tab_id),
        }
    }

    fn draft(&self) -> Option<&TextEdit> {
        match &self.state {
            Overlay::Idle => None,
            Overlay::Commenting { draft, .. } | Overlay::Noting { draft, .. } => Some(draft),
        }
    }

    fn draft_mut(&mut self) -> Option<&mut TextEdit> {
        match &mut self.state {
            Overlay::Idle => None,
            Overlay::Commenting { draft, .. } | Overlay::Noting { draft, .. } => Some(draft),
        }
    }

    fn field(&self) -> Option<&Field> {
        match &self.state {
            Overlay::Idle => None,
            Overlay::Commenting { field, .. } | Overlay::Noting { field, .. } => Some(field),
        }
    }
}

impl App {
    pub fn browser_overlay_active(&self, tab_id: u64) -> bool {
        self.workspace_panes.browser.overlay.active_for(tab_id)
    }

    pub fn browser_overlay_accent(&self, tab_id: u64) -> bool {
        self.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.inspecting)
            || self.workspace_panes.browser.overlay.commenting_for(tab_id)
    }

    /// Opens the spotlight + floating comment editor for a picked element (`overlay-machine.ts`).
    pub fn open_browser_element_comment(&mut self, element: PickedElement) {
        let tab_id = element.tab_id;
        let backend_id = element.backend_node_id;
        let needs_describe =
            element.rect[2] <= 0.0 || element.rect[3] <= 0.0 || element.selector.starts_with("node#");
        let Some(tab) = self.workspace_panes.browser.host.tab(tab_id) else {
            return;
        };
        let url = tab.url.clone();
        if url.is_empty() || url == "about:blank" {
            return;
        }
        // Hide Chromium's blue DevTools overlay so only our dimmed spotlight + focus outline shows.
        self.workspace_panes.browser.host.highlight_node(tab_id, None);
        let mut draft = TextEdit::new("");
        draft.focused = true;
        self.composer.focused = false;
        crate::plugin_host::blur_fields(self);
        self.workspace_panes.browser.overlay.state = Overlay::Commenting {
            tab_id,
            url,
            element,
            draft,
            field: Field::default(),
            editor_h: Cell::new(0.0),
            editor_initial_h: Cell::new(0.0),
        };
        if needs_describe && backend_id > 0 {
            super::comment_describe::PendingDescribe::start(self, tab_id, backend_id);
        }
    }

    /// Opens the floating `"Add page to session"` note editor (`overlay-machine.ts` `note.opened`).
    pub fn open_browser_page_note(&mut self, tab_id: u64) {
        if matches!(&self.workspace_panes.browser.overlay.state, Overlay::Noting { tab_id: id, .. } if *id == tab_id) {
            return;
        }
        let Some((url, title, inspecting)) = self.workspace_panes.browser.host.tab(tab_id).and_then(|tab| {
            (!tab.url.is_empty() && tab.url != "about:blank" && tab.load_error.is_none())
                .then(|| (tab.url.clone(), tab.title.clone(), tab.inspecting))
        }) else {
            return;
        };
        if inspecting {
            self.workspace_panes.browser.host.toggle_inspect(tab_id);
        }
        let mut draft = TextEdit::new("");
        draft.focused = true;
        self.composer.focused = false;
        crate::plugin_host::blur_fields(self);
        self.workspace_panes.browser.overlay.state = Overlay::Noting {
            tab_id,
            url,
            title,
            draft,
            field: Field::default(),
        };
    }

    pub fn close_browser_overlay(&mut self) {
        if let Overlay::Commenting { tab_id, .. } = self.workspace_panes.browser.overlay.state {
            self.workspace_panes.browser.host.highlight_node(tab_id, None);
        }
        self.workspace_panes.browser.overlay.describe.clear();
        self.workspace_panes.browser.overlay.state = Overlay::Idle;
    }

    /// Reveals the browser tab named by a composer `NoteComment` chip (`renderer.tsx:359`).
    pub fn reveal_browser_note_href(&mut self, href: &str, tx: &mut Tx) {
        let Some(tab_id) = href.split('#').next().and_then(|s| s.parse::<u64>().ok()) else {
            return;
        };
        if self.workspace_panes.browser.host.tab(tab_id).is_none() {
            return;
        }
        let key = panel_key(PLUGIN, &tab_id.to_string());
        self.open_side_tab(&key, None, false, false, tx);
        self.workspace_panes.browser.host.focus_tab(tab_id);
    }
}

/// Polls `@file` mention searches and closes overlays whose tab disappeared or switched away.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    let active_tab = match &app.workspace_panes.browser.overlay.state {
        Overlay::Idle => None,
        Overlay::Commenting { tab_id, .. } | Overlay::Noting { tab_id, .. } => Some(*tab_id),
    };
    if let Some(tab_id) = active_tab {
        let key = panel_key(PLUGIN, &tab_id.to_string());
        let gone = app.workspace_panes.browser.host.tab(tab_id).is_none() || !app.side_tab_visible(&key);
        if gone {
            app.workspace_panes.browser.overlay.state = Overlay::Idle;
            changed = true;
        }
    }
    changed |= super::comment_describe::PendingDescribe::poll(app);
    if let Some((attempt, items)) = app.workspace_panes.browser.overlay.mention_fetch.take() {
        step_field(
            app,
            items.map_or(FieldEvent::Failed { attempt }, |items| FieldEvent::Loaded { attempt, items }),
            tx,
        );
        changed = true;
    }
    let timed_out = app.workspace_panes.browser.overlay.field().and_then(|f| match f.mention {
        Mention::Loading { due, .. } => Some((due, f.attempt)),
        _ => None,
    });
    if let Some((due, attempt)) = timed_out {
        if tx.clock() + 1e-9 >= due {
            step_field(app, FieldEvent::Failed { attempt }, tx);
            changed = true;
        } else {
            tx.wake_at(due);
        }
    }
    changed
}

/// Tracks held `Ctrl` / `Cmd` so the compact comment action flips live between Add and Send/Steer/Queue.
pub fn modifiers(app: &mut App, mods: Mods, tx: &mut Tx) -> bool {
    let held = (mods.ctrl || mods.cmd) && !mods.alt && !mods.shift;
    let cfg = field_config(app);
    if app.workspace_panes.browser.overlay.field().is_none_or(|f| f.alternate == (held && cfg.alternate.is_some())) {
        return false;
    }
    step_field(app, FieldEvent::Modifier { held, config: cfg }, tx);
    tx.instant();
    true
}

fn field_config(app: &App) -> Config {
    let ov = &app.workspace_panes.browser.overlay;
    let blank = ov.draft().is_none_or(|d| d.text.trim().is_empty());
    let allow_empty = matches!(ov.state, Overlay::Commenting { .. });
    let can_send = app.can_send_quote();
    Config { alternate: Some(!can_send), submitting: false, allow_empty, blank }
}

fn delivery_label(app: &App) -> &'static str {
    match (app.quote_session_busy(), app.quote_follow_up()) {
        (false, _) => opencode_i18n::t!("rune.browser.comment.send"),
        (true, FollowUp::Queue) => opencode_i18n::t!("rune.browser.comment.steer"),
        (true, FollowUp::Steer) => opencode_i18n::t!("rune.browser.comment.queue"),
    }
}

fn step_field(app: &mut App, event: FieldEvent, tx: &mut Tx) -> bool {
    let (effects, handled) = {
        let ov = &mut app.workspace_panes.browser.overlay;
        let f = match &mut ov.state {
            Overlay::Idle => return false,
            Overlay::Commenting { field, .. } | Overlay::Noting { field, .. } => field,
        };
        comment::field_step(&mut ov.seq, f, event)
    };
    for effect in effects {
        match effect {
            FieldEffect::Search { attempt, query, due } => {
                tx.wake_at(due);
                match prepare_path_search(app, &query) {
                    PathSearch::Immediate(items) => {
                        step_field(app, FieldEvent::Loaded { attempt, items }, tx);
                    }
                    PathSearch::Live { server, directory } => {
                        request_path_search(
                            &mut app.workspace_panes.browser.overlay.mention_fetch,
                            server,
                            directory,
                            attempt,
                            query,
                        );
                    }
                }
            }
            FieldEffect::Insert { path, token } => {
                if let Some(draft) = app.workspace_panes.browser.overlay.draft_mut() {
                    let insert = format!("@{path} ");
                    draft.set_selection(token.start, token.end);
                    draft.insert(&insert);
                }
            }
            FieldEffect::Submit { alternate } => submit_overlay(app, alternate, tx),
            FieldEffect::Cancel => app.close_browser_overlay(),
        }
    }
    handled
}

fn submit_overlay(app: &mut App, send: bool, tx: &mut Tx) {
    let prev = std::mem::take(&mut app.workspace_panes.browser.overlay.state);
    let note = match prev {
        Overlay::Idle => return,
        Overlay::Commenting { tab_id, url, element, draft, .. } => {
            let live = app
                .workspace_panes
                .browser
                .host
                .tab(tab_id)
                .is_some_and(|t| t.url == url);
            super::comment_describe::element_note(tab_id, &url, &element, live, draft.text.trim())
        }
        Overlay::Noting { tab_id, url, title, draft, .. } => {
            super::comment_describe::page_note(tab_id, &url, &title, draft.text.trim())
        }
    };
    let key = app.quotes_mut().push(note);
    if send {
        if app.send_quote(key, tx).is_some() {
            app.quotes_mut().remove(key);
        }
    } else if !session_view::style::touch_controls() {
        app.composer.focused = true;
        app.kernel.slots.get_mut(&crate::panels::SIDE).focus.blur();
    }
}

fn on_edit_draft(app: &mut App) -> &mut TextEdit {
    let ov = &mut app.workspace_panes.browser.overlay;
    match &mut ov.state {
        Overlay::Commenting { draft, .. } | Overlay::Noting { draft, .. } => draft,
        Overlay::Idle => &mut ov.spare,
    }
}

fn on_input_change(app: &mut App, tx: &mut Tx) {
    let token = app.workspace_panes.browser.overlay.draft().and_then(comment::token);
    let now = tx.clock();
    step_field(app, FieldEvent::Synced { token, now }, tx);
}

fn on_input_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let key = key_of(ev, tx);
    if key == comment::FieldKey::Other {
        return false;
    }
    tx.instant();
    let cfg = field_config(app);
    step_field(app, FieldEvent::Key { key, config: cfg }, tx)
}

fn on_action_press(app: &mut App, force_alternate: Option<bool>, tx: &mut Tx) {
    tx.instant();
    let modified = force_alternate.unwrap_or_else(|| {
        let m = tx.mods();
        (m.ctrl || m.cmd) && !m.alt && !m.shift
    });
    let cfg = field_config(app);
    step_field(app, FieldEvent::Clicked { modified, config: cfg }, tx);
}

fn on_backdrop_press(app: &mut App) {
    if app.workspace_panes.browser.overlay.draft().is_none_or(|d| d.text.trim().is_empty()) {
        app.close_browser_overlay();
    }
}

/// Renders the dimmed spotlight and/or floating comment/note editor over `browser-page-stack`.
pub fn view(s: &App, p: &Pal, tab_id: u64) -> Option<El> {
    let ov = &s.workspace_panes.browser.overlay;
    if !ov.active_for(tab_id) {
        return None;
    }
    let [pane_w, pane_h] = ov.pane_size.get();
    let pane_w = if pane_w > 0.0 { pane_w } else { 640.0 };
    let pane_h = if pane_h > 0.0 { pane_h } else { 480.0 };
    let zoom = (*s.zoom).max(0.01);

    let act: editor::FieldAct<App> = Rc::new(|app: &mut App, ev: FieldEvent, tx: &mut Tx| {
        step_field(app, ev, tx);
    });

    match &ov.state {
        Overlay::Idle => None,
        Overlay::Commenting { element, draft, field, editor_h, editor_initial_h, .. } => {
            let [rx, ry, rw, rh] = element.rect.map(|v| v / zoom);
            let sx = rx.clamp(0.0, pane_w);
            let sy = ry.clamp(0.0, pane_h);
            let sw = rw.max(0.0).min((pane_w - sx).max(0.0));
            let sh = rh.max(0.0).min((pane_h - sy).max(0.0));

            let dim = p.scrim.alpha(p.scrim.a * 0.5);
            let spotlight_layer = if sw > 0.0 && sh > 0.0 {
                el().key("browser-comment-spotlight-layer")
                    .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .child(el().absolute(0.0, 0.0).w(pane_w).h(sy).fill(dim))
                    .child(el().absolute(0.0, sy + sh).w(pane_w).h((pane_h - (sy + sh)).max(0.0)).fill(dim))
                    .child(el().absolute(0.0, sy).w(sx).h(sh).fill(dim))
                    .child(el().absolute(sx + sw, sy).w((pane_w - (sx + sw)).max(0.0)).h(sh).fill(dim))
                    .child(
                        el().key("browser-comment-spotlight")
                            .absolute(sx, sy)
                            .size(sw, sh)
                            .radius(2.0)
                            .border(1.0, p.v2.border_focus),
                    )
            } else {
                el().key("browser-comment-spotlight-layer")
                    .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .fill(dim)
            };

            let width = (pane_w - GAP * 2.0).clamp(160.0, ELEMENT_EDITOR_W);
            let surface_h = (pane_h - GAP * 2.0).max(80.0);
            let init_h = {
                let h = editor_initial_h.get();
                if h > 0.0 { h.min(surface_h) } else { 80.0_f32.min(surface_h) }
            };
            let left = sx.clamp(GAP, (pane_w - width - GAP).max(GAP));
            let below = (sy + sh + GAP).max(GAP);
            let above = sy - GAP - init_h;
            let opening_top = if below + init_h <= pane_h - GAP {
                below
            } else if above >= GAP {
                above
            } else {
                (pane_h - init_h - GAP).max(GAP)
            };
            let bottom = (opening_top + init_h).min(pane_h - GAP);
            let max_h = (bottom - GAP).max(80.0);
            let cur_h = {
                let h = editor_h.get();
                if h > 0.0 { h.min(max_h) } else { init_h.min(max_h) }
            };
            let top = (bottom - cur_h).max(GAP);

            let selector = if element.selector.is_empty() {
                element.label.clone()
            } else {
                element.selector.clone()
            };
            let preview = row()
                .key("browser-comment-preview")
                .w_full()
                .child(el().w(2.0).h_full().fill(p.layer3))
                .child(
                    text(selector.clone())
                        .font(MONO)
                        .color(p.muted)
                        .wrap_anywhere()
                        .max_lines(2)
                        .dir(rune::Direction::Ltr)
                        .pl(10.0)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .help(selector)
                        .help_above(),
                );

            let input = session_view::style::field(draft, FIELD)
                .key("browser-comment-field")
                .label_static(opencode_i18n::t!("plugin.browser.comment.label"))
                .color(p.text)
                .w_full()
                .min_h(20.0)
                .max_h(160.0)
                .placeholder(opencode_i18n::t!("plugin.browser.comment.placeholder"), p.faint)
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .spellcheck(s.settings.spellcheck)
                .dir(rune::Direction::Auto)
                .dir_by_paragraph()
                .on_edit(on_edit_draft)
                .on_input_change(on_input_change)
                .on_input_key(on_input_key);
            let mentions = mention_rows::<App>(p, &field.mention, act);
            let field_col = col().grow(1.0).shrink().min_w(0.0).child(input).children(mentions);

            let empty = draft.text.trim().is_empty();
            let alternate = field.alternate;
            let available = s.can_send_quote();
            let primary_label = if empty {
                opencode_i18n::t!("plugin.browser.comment.attach")
            } else {
                opencode_i18n::t!("plugin.browser.comment.submit")
            };
            let alt_label = delivery_label(s);
            let alt_keys = [opencode_i18n::t!("common.key.ctrl").to_owned(), "⏎".to_owned()];
            let label = if alternate { alt_label } else { primary_label };
            let glyph = if alternate {
                icon(&ic::ARROW_UP, 16.0).color(p.q.submit_text)
            } else {
                icon(&CHECK, 16.0).color(if empty { p.icon_muted } else { p.text })
            };
            let face = if alternate {
                session_view::composer::submit_button(p, true).key("browser-comment-submit").size(20.0, 20.0).radius(4.0)
            } else if empty {
                el().key("browser-comment-submit").role("button").size(20.0, 20.0).radius(4.0).center().hover(p.hover).press(p.pressed)
            } else {
                el().key("browser-comment-submit")
                    .role("button")
                    .size(20.0, 20.0)
                    .radius(4.0)
                    .center()
                    .fill(if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral })
                    .elevate(neutral_lift(p))
                    .hover(p.hover)
                    .press(p.pressed)
            };
            let tip = if alternate {
                face.help(label).help_keys(alt_keys)
            } else {
                face.help(label).help_keys(["⏎".to_owned()]).help_row(alt_label).help_keys(alt_keys)
            };
            let disabled = alternate && !available;
            let mut action = tip.label(label).help_above().motion(Spring::NONE).child(glyph);
            if disabled {
                action = action.opacity(0.5).focusable(false);
            } else {
                action = action.on_press_tx(|app: &mut App, tx: &mut Tx| on_action_press(app, None, tx));
            }

            let input_row = row().w_full().gap(12.0).items_end().min_h(20.0).child(field_col).child(action);
            let shell = col().w_full().padding(12.0).gap(10.0).child(preview).child(input_row);
            let body = session_view::scroller::scroller("browser-comment-scroll")
                .bar(session_view::scroller::Bar::MenuScroll)
                .build::<App>(p, col().w_full().max_h(max_h), shell);
            let card = col()
                .key("browser-comment-editor")
                .absolute(left, top)
                .w(width)
                .radius(8.0)
                .fill(p.layer1)
                .elevate(p.floating)
                .clip_soft(0.0)
                .transition(Transition::none())
                .motion(Spring::NONE)
                .preserve_focus_on_press(true)
                .on_layout(|app: &mut App, [_, _, _, h]| {
                    if let Overlay::Commenting { editor_h, editor_initial_h, .. } = &app.workspace_panes.browser.overlay.state {
                        let ch = h.ceil();
                        editor_h.set(ch);
                        if editor_initial_h.get() == 0.0 && ch > 0.0 {
                            editor_initial_h.set(ch);
                        }
                    }
                })
                .on_press(|_: &mut App| {})
                .child(body);

            Some(
                el().key("browser-comment-backdrop")
                    .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .on_layout(|app: &mut App, [_, _, w, h]| {
                        app.workspace_panes.browser.overlay.pane_size.set([w, h]);
                    })
                    .on_press(on_backdrop_press)
                    .child(spotlight_layer)
                    .child(card),
            )
        }
        Overlay::Noting { url, title, draft, field, .. } => {
            let width = (pane_w - GAP * 2.0).clamp(180.0, PAGE_EDITOR_W);
            let max_h = (pane_h - GAP * 2.0).max(120.0);
            let host = url
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split(['/', '?', '#']).next())
                .unwrap_or("");
            let heading = if !title.is_empty() {
                title.clone()
            } else if !host.is_empty() {
                host.to_owned()
            } else {
                url.clone()
            };
            let footer_meta = row()
                .key("browser-note-header")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(6.0)
                .center_y()
                .dir(rune::Direction::Ltr)
                .child(icon(&ic::GLOBE, 14.0).color(p.icon_muted))
                .child(
                    text(heading)
                        .font(TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04))
                        .color(p.muted)
                        .truncate()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0),
                );

            let input = session_view::style::field(draft, FIELD)
                .key("browser-note-field")
                .label_static(opencode_i18n::t!("plugin.browser.page.add"))
                .color(p.text)
                .w_full()
                .min_h(80.0)
                .max_h(160.0)
                .placeholder(opencode_i18n::t!("plugin.browser.page.placeholder"), p.faint)
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .spellcheck(s.settings.spellcheck)
                .dir(rune::Direction::Auto)
                .dir_by_paragraph()
                .on_edit(on_edit_draft)
                .on_input_change(on_input_change)
                .on_input_key(on_input_key);
            let mentions = mention_rows::<App>(p, &field.mention, act);
            let field_col = col().w_full().child(input).children(mentions);

            let k = kit(p);
            let empty = draft.text.trim().is_empty();
            let alternate = field.alternate;
            let can_send = s.can_send_quote();
            let primary_label = opencode_i18n::t!("ui.lineComment.submit");
            let alt_label = delivery_label(s);
            let alt_keys = [opencode_i18n::t!("common.key.ctrl").to_owned(), "⏎".to_owned()];
            let submit_label = if alternate { alt_label } else { primary_label };
            let submit_disabled = empty || (alternate && !can_send);
            #[allow(deprecated)]
            let cancel_btn = crate::widgets::button(opencode_i18n::t!("ui.lineComment.cancel"), rune_kit::ButtonVariant::Ghost)
                .size(rune_kit::ButtonSize::Small)
                .on_press(|app: &mut App| app.close_browser_overlay())
                .build(k)
                .key("browser-note-cancel");
            #[allow(deprecated)]
            let submit_btn = crate::widgets::button(submit_label, rune_kit::ButtonVariant::Contrast)
                .size(rune_kit::ButtonSize::Small)
                .disabled(submit_disabled)
                .on_press_tx(|app: &mut App, tx: &mut Tx| on_action_press(app, None, tx))
                .build(k)
                .key("browser-note-submit")
                .help(primary_label)
                .help_keys(["⏎".to_owned()])
                .help_row(alt_label)
                .help_keys(alt_keys)
                .help_above();
            let footer = row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(footer_meta)
                .child(row().gap(6.0).center_y().child(cancel_btn).child(submit_btn));

            let shell = col().w_full().padding(12.0).gap(10.0).child(field_col).child(footer);
            let body = session_view::scroller::scroller("browser-note-scroll")
                .bar(session_view::scroller::Bar::MenuScroll)
                .build::<App>(p, col().w_full().max_h(max_h), shell);
            let card = col()
                .key("browser-note-editor")
                .pin(None, Some(GAP), Some(GAP), None)
                .w(width)
                .radius(6.0)
                .fill(p.layer1)
                .elevate(p.raised)
                .clip_soft(0.0)
                .cursor(Cursor::Default)
                .transition(Transition::none())
                .motion(Spring::NONE)
                .preserve_focus_on_press(true)
                .on_press(|_: &mut App| {})
                .child(body);

            Some(
                el().key("browser-note-backdrop")
                    .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .on_layout(|app: &mut App, [_, _, w, h]| {
                        app.workspace_panes.browser.overlay.pane_size.set([w, h]);
                    })
                    .on_press(on_backdrop_press)
                    .child(card),
            )
        }
    }
}
