use opencode_kit::model::Attachment;
use rune::{ClipboardContent, ClipboardImage, Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, Tx, col};
use rune_knobs::Knobs;

use super::attachments::push_unique;
use super::history::{Entry, PromptHistory, arrow_up};
use super::{COMPOSER_MAX_H, ComposerHost, composer, is_stop_shortcut};
use crate::palette::pal;
use crate::{TimelineHost, ViewState};

#[derive(Default)]
struct TestHost {
    knobs: Knobs,
    clock: f64,
    busy: bool,
    stopped: usize,
    sent: Vec<(String, usize)>,
    edit: TextEdit,
    files: Vec<Attachment>,
    history: PromptHistory<String>,
    view: ViewState,
}

impl TimelineHost for TestHost {
    fn knobs(&self) -> &Knobs {
        &self.knobs
    }
    fn clock(&self) -> f64 {
        self.clock
    }
    fn active_session_id(&self) -> Option<&str> {
        Some("ses_test")
    }
    crate::delegate_view_state!(view);
}

impl ComposerHost for TestHost {
    fn composer_edit(&self) -> &TextEdit {
        &self.edit
    }
    fn composer_edit_mut(&mut self) -> &mut TextEdit {
        &mut self.edit
    }
    fn composer_busy(&self) -> bool {
        self.busy
    }
    fn composer_send(&mut self, tx: &mut Tx) {
        let text = self.edit.text.trim().to_owned();
        if text.is_empty() && self.files.is_empty() {
            return;
        }
        let entry = Entry::of(&self.edit);
        let n_files = std::mem::take(&mut self.files).len();
        self.sent.push((text, n_files));
        let focused = self.edit.focused;
        self.edit = TextEdit::new("");
        self.edit.focused = focused;
        self.history.add(entry, false);
        tx.instant();
    }
    fn composer_stop(&mut self, tx: &mut Tx) {
        self.busy = false;
        self.stopped += 1;
        tx.instant();
    }
    fn composer_attachments(&self) -> &[Attachment] {
        &self.files
    }
    fn composer_remove_attachment(&mut self, id: &str) {
        self.files.retain(|f| crate::images::identity(f) != id);
    }
    fn composer_paste_attachment(&mut self, a: Attachment) -> bool {
        push_unique(&mut self.files, a);
        true
    }
    fn composer_on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.busy && is_stop_shortcut(ev) {
            self.composer_stop(tx);
            return true;
        }
        if let Some(up) = arrow_up(ev) {
            let scope = "ses_test".to_owned();
            if self.history.step(&mut self.edit, &scope, false, up) {
                tx.instant();
                return true;
            }
        }
        false
    }
}

fn app(s: &TestHost, _cx: &mut Cx<TestHost>) -> El {
    let p = pal(true);
    col().key("root").w_full().h_full().center().fill(p.bg_base).child(composer(s, p))
}

fn rect(rt: &Runtime<TestHost>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

#[test]
fn grows_caps_sends_recalls_pastes_and_stops() {
    let mut rt = Runtime::new(TestHost::default(), app, (960.0, 640.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!((rect(&rt, "composer")[3] - 88.0).abs() < 0.5);

    let (x, y) = rt.find("prompt").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    // Shift+Enter inserts a newline and grows by 20pt.
    rt.key(KeyEvent { key: Key::Text("first".into()), mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } });
    rt.key(KeyEvent { key: Key::Text("second".into()), mods: Mods::default() });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!((rect(&rt, "composer")[3] - 108.0).abs() < 0.5);

    // Long text caps editor at 180pt.
    rt.update(|s, _| s.edit = TextEdit::new((0..30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    assert!((rect(&rt, "editor")[3] - COMPOSER_MAX_H).abs() < 0.5);

    // Plain Enter sends, clears, and records history; Up recalls it.
    rt.update(|s, _| {
        s.edit = TextEdit::new("ship it");
        s.edit.focused = true;
    });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sent, vec![("ship it".to_owned(), 0)]);
    assert_eq!(rt.state.edit.text, "");

    rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.text, "ship it");
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.text, "");

    // Pasting an image attaches a chip and enables Send even with empty text.
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let img = ClipboardImage::from_png(png).unwrap();
    rt.paste_content(ClipboardContent::default().with_image(img));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.files.len(), 1);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("attachments")));
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sent.last(), Some(&("".to_owned(), 1)));

    // Busy session shows Stop glyph and stops on press or Esc.
    rt.update(|s, _| s.busy = true);
    rt.frame(1.0 / 60.0);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("stop-glyph")));
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.stopped, 1);
    assert!(!rt.state.busy);
}

#[test]
fn enter_in_an_empty_composer_stops_a_busy_session_once() {
    let mut rt = Runtime::new(TestHost::default(), app, (960.0, 640.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(1.0 / 60.0);
    let enter = KeyEvent { key: Key::Enter, mods: Mods::default() };

    // Idle and empty: nothing happens.
    rt.key(enter.clone());
    rt.frame(1.0 / 60.0);
    assert!(rt.state.sent.is_empty() && rt.state.stopped == 0);

    // Busy with words: Enter still sends.
    rt.update(|s, _| {
        s.busy = true;
        s.edit.text = "steer".into();
    });
    rt.frame(1.0 / 60.0);
    rt.key(enter.clone());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.sent, vec![("steer".to_owned(), 0)]);
    assert!(rt.state.busy);

    // A held Enter's repeats don't stop it; whitespace alone is empty, so a fresh press does.
    rt.repeat_key(enter.clone());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.stopped, 0);
    rt.update(|s, _| s.edit.text = "  ".into());
    rt.frame(1.0 / 60.0);
    rt.key(enter);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.stopped, 1);
    assert!(!rt.state.busy);
}

#[test]
fn story_host_launches_sent_bubble_from_composer_and_docks_flush_under_timeline() {
    use crate::fixture::SESSION_ID;
    use crate::story::{Host, view};

    let mut rt = Runtime::new(
        Host { show_composer: true, ..Host::default() },
        view,
        (1100.0, 820.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }

    let rect = |rt: &Runtime<Host>, key: &str| {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap().rect
    };
    let fade = rect(&rt, "timeline-fade");
    let dock = rect(&rt, "dock");
    assert!((fade[1] + fade[3] - dock[1]).abs() < 1.0, "timeline-fade meets dock: {fade:?} vs {dock:?}");

    let prompt_x = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("prompt"))
        .unwrap()
        .rect[0]
        + 4.0;
    let prompt_y = rect(&rt, "prompt")[1] + 4.0;
    rt.pointer_move(prompt_x, prompt_y);
    rt.pointer_down(prompt_x, prompt_y);
    rt.pointer_up(prompt_x, prompt_y);
    rt.frame(1.0 / 120.0);
    rt.key(KeyEvent { key: Key::Text("Launch me smoothly.".into()), mods: Mods::default() });
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    let source_y = rect(&rt, "prompt")[1];
    let before_ids: Vec<String> = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.key.filter(|k| k.starts_with("u-")))
        .collect();

    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    let mut ys = Vec::new();
    let mut new_row = None;
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let els = rt.elements();
        let row_key = new_row.get_or_insert_with(|| {
            els.iter()
                .filter_map(|e| e.key.clone())
                .find(|k| k.starts_with("u-") && !before_ids.contains(k))
                .expect("sent user row")
        });
        let suffix = format!("/{row_key}/");
        if let Some(b) = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("bubble") && e.path.contains(&suffix)) {
            ys.push(b.rect[1]);
        }
    }
    let first_y = ys[0];
    let last_y = *ys.last().unwrap();
    assert!(
        (first_y - source_y).abs() < 40.0 && first_y > last_y + 40.0,
        "sent bubble starts near prompt ({source_y}) and rises to {last_y}: {ys:?}"
    );
    assert!(rt.state.ends().at_end(SESSION_ID));
}
