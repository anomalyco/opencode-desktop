//! The narrow window's Files view (`gui-plugins/src/file/mobile.tsx`): a 40 pt header with All
//! files and a tab per open file, over the file tree at the panel's full width while browsing,
//! or the open file.

use rune::{Color, El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::SessionFiles;
use super::data::{OPEN_TAB_ID, file_tab_key, file_tab_path};
use crate::App;
use crate::palette::Pal;
use opencode_i18n::t;

/// What a session's Files view shows: the tree (All files), or its selected file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NarrowFiles {
    /// The selected file tab, or the tree while none is open.
    #[default]
    File,
    /// All files: the tree, whatever is open.
    Browse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NarrowFilesEvent {
    AllFiles,
    /// A file was opened or a file tab picked.
    Opened,
}

impl NarrowFiles {
    pub fn step(self, event: NarrowFilesEvent) -> NarrowFiles {
        match event {
            NarrowFilesEvent::AllFiles => NarrowFiles::Browse,
            NarrowFilesEvent::Opened => NarrowFiles::File,
        }
    }
}

/// `button-v2` small: 13 / 20 at 530.
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
/// `tabs-v2-trigger`: 13 / 16 at 440, -0.04 tracking.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

pub fn view(s: &App, p: &Pal, sid: &str, project_id: &str, width: f32) -> El {
    let empty = SessionFiles::default();
    let ws = s.files.sessions.get(sid).unwrap_or(&empty);
    let opened: Vec<String> = s
        .side
        .sessions
        .get(sid)
        .map(|st| st.tab_state.tabs.all.iter().filter_map(|k| k.strip_prefix("file:")).filter(|id| *id != OPEN_TAB_ID).map(str::to_owned).collect())
        .unwrap_or_default();
    let selected = s.side.sessions.get(sid).and_then(|st| {
        let active = st.tab_state.tabs.active.as_deref()?.strip_prefix("file:")?;
        opened.iter().find(|id| *id == active).cloned()
    });
    let selected = selected.or_else(|| opened.first().cloned());
    let browsing = ws.narrow == NarrowFiles::Browse || selected.is_none();
    let title = s
        .project(project_id)
        .map(|pr| opencode_kit::path::basename(&pr.directory).to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| t!("rune.files.workspace").into());
    let body = if browsing {
        super::view::files_sidebar(s, p, sid, &title, ws, None, width)
    } else {
        let path = selected.as_deref().and_then(file_tab_path);
        super::viewer::files_main(s, p, sid, &title, ws, path.as_deref(), false, false)
    };
    col()
        .key("narrow-files")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .child(header(p, sid, &opened, selected.as_deref().filter(|_| !browsing)))
        .child(col().key("narrow-files-body").w_full().grow(1.0).min_h(0.0).child(body))
}

/// `session-mobile-files-header`: All files (`Button` small ghost, `mx-2`), then the open files
/// as line tabs, under the strip's 1 px `border-base` rule.
fn header(p: &Pal, sid: &str, opened: &[String], selected: Option<&str>) -> El {
    let all = {
        let sid = sid.to_owned();
        row()
            .key("narrow-files-all")
            .role("button")
            .label_static(t!("plugin.file.tree.all"))
            .h(24.0)
            .mx(8.0)
            .px(9.0)
            .center_y()
            .radius(4.0)
            .hover(p.hover)
            .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                let ws = s.files.sessions.entry(sid.clone()).or_default();
                ws.narrow = ws.narrow.step(NarrowFilesEvent::AllFiles);
            })
            .child(text(t!("plugin.file.tree.all")).font(BUTTON).color(p.text))
    };
    let tabs = opened.iter().fold(row().key("narrow-files-tabs").h_full().gap(6.0).grow(1.0).min_w(0.0).scroll_x(), |strip, id| {
        let on = selected == Some(id.as_str());
        let name = file_tab_path(id).map(|path| opencode_kit::path::basename(&path).to_owned()).unwrap_or_default();
        let (pick, close, sid) = (id.clone(), id.clone(), sid.to_owned());
        let sid2 = sid.clone();
        strip.child(
            row()
                .key(format!("narrow-files-tab-{}", id.replace(['/', '\\', ':'], "_")))
                .role("tab")
                .label(name.clone())
                .focus_selected(on)
                .h_full()
                .max_w(192.0)
                .gap(4.0)
                .center_y()
                .hover_group()
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(path) = file_tab_path(&pick) {
                        s.open_workspace_file_for_session(&sid, &path, false, None, false, tx);
                    }
                })
                .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(if on { p.faint } else { Color::TRANSPARENT }))
                .child(row().h_full().padding(6.0).center_y().min_w(0.0).child(text(name).font(TAB).color(if on { p.text } else { p.muted }).group_color(p.text).truncate()))
                .child(
                    row()
                        .key("close")
                        .role("button")
                        .label_static(t!("common.closeTab"))
                        .size(20.0, 20.0)
                        .ml(-5.0)
                        .center()
                        .hover_group()
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            if let Some(path) = file_tab_path(&close).filter(|_| s.active_session_id() == Some(sid2.as_str())) {
                                s.close_side_tab(&file_tab_key(&path), tx);
                            }
                        })
                        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.faint)),
                ),
        )
    });
    row()
        .key("narrow-files-header")
        .w_full()
        .h(40.0)
        .min_h(40.0)
        .center_y()
        .child(all)
        .child(row().key("narrow-files-strip").grow(1.0).min_w(0.0).h_full().child(el().key("rule").pin(Some(-8.0), None, Some(-8.0), Some(0.0)).h(1.0).fill(p.border)).child(tabs))
}
