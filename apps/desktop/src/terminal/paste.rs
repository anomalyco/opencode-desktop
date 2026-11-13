//! A paste into a terminal, and when it asks first.
//!
//! | The paste | The program asked for bracketed paste | It didn't |
//! | --- | --- | --- |
//! | one line | sent | sent |
//! | one line and a line break | sent (the program sees a paste, runs nothing) | sent without the break, so it waits for Enter |
//! | several lines, or control characters | sent | asks first, showing what runs; then sent without the last break |
//! | the paste-end sequence (`ESC [201~`) | asks first: it would end the paste early | asks first |
//!
//! Without bracketed paste a shell reads each line break as Enter, so a pasted script runs line
//! by line as it arrives; Ghostty's own `clipboard-paste-protection` asks in the same cases.

use rune::{El, Family, TextStyle, Tx, col, el, row, spacer, text};
use rune_kit::ButtonVariant;

use crate::App;
use crate::palette::{Elevate as _, Pal};
use opencode_i18n::t;

/// `dialog-title`: 15 on 20 at 530, as the session delete confirmation's.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// The pasted lines, in the code font.
const CODE: TextStyle = TextStyle::new(12.0, 400.0).lh(18.0).family(Family::Mono);
/// Lines of a paste the question shows.
const SHOWN_LINES: usize = 8;
/// Characters of each shown line.
const SHOWN_CHARS: usize = 120;

/// What a paste does: goes to the program as this text, or waits for the user.
#[derive(Debug, PartialEq, Eq)]
pub enum Paste {
    Send(String),
    Ask,
}

/// A paste waiting for the user's answer: into which terminal, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub ws: String,
    pub tab: String,
    pub text: String,
}

/// What pasting `text` does in a program that has (`bracketed`) or hasn't asked for bracketed paste.
pub fn judge(text: &str, bracketed: bool) -> Paste {
    if text.contains("\x1b[201~") {
        return Paste::Ask;
    }
    if bracketed {
        return Paste::Send(text.to_owned());
    }
    let body = without_last_break(text);
    let runs = body.contains(['\n', '\r']) || body.chars().any(|c| c.is_control() && c != '\t');
    if runs { Paste::Ask } else { Paste::Send(body.to_owned()) }
}

/// `text` without one line break at its end.
pub fn without_last_break(text: &str) -> &str {
    text.strip_suffix("\r\n").or_else(|| text.strip_suffix(['\n', '\r'])).unwrap_or(text)
}

/// The paste's lines as the question shows them: control characters as their pictures (`␛`, `␃`).
fn shown(text: &str) -> (Vec<String>, usize) {
    let lines: Vec<&str> = without_last_break(text).split(['\n']).map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let picture = |c: char| match c {
        '\t' => ' ',
        c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or('?'),
        '\u{7f}' => '\u{2421}',
        c => c,
    };
    let shown = lines.iter().take(SHOWN_LINES).map(|l| {
        let mut s: String = l.chars().map(picture).take(SHOWN_CHARS).collect();
        if l.chars().count() > SHOWN_CHARS {
            s.push('…');
        }
        s
    });
    (shown.collect(), lines.len())
}

impl App {
    /// Pastes `text` into terminal `tab`, or asks first ([`judge`]).
    pub(crate) fn paste_into_terminal(&mut self, ws: &str, tab: &str, text: String, tx: &mut Tx) {
        let Some(t) = self.terminal_tab_mut(ws, tab) else { return };
        match judge(&text, t.session.snapshot().bracketed_paste) {
            Paste::Send(text) => {
                t.session.paste(text.clone());
                if t.fixture_shell
                    && let Some(st) = &t.stream
                {
                    st.feed(text.as_bytes());
                }
                self.terminal.reset_blink(*self.clock);
            }
            Paste::Ask => {
                tx.instant();
                self.terminal.pending_paste = Some(Pending { ws: ws.to_owned(), tab: tab.to_owned(), text });
            }
        }
    }

    /// The answer to the paste question: send it (without its last line break) or drop it.
    fn answer_paste(&mut self, send: bool, tx: &mut Tx) {
        tx.instant();
        let Some(p) = self.terminal.pending_paste.take() else { return };
        if send
            && let Some(t) = self.terminal_tab_mut(&p.ws, &p.tab)
        {
            t.session.paste(without_last_break(&p.text).to_owned());
        }
        self.terminal.claim_focus = true;
    }
}

/// The question a paste that would run commands asks, over the window (the session delete
/// confirmation's dialog: 480 wide, title, description, footer with Cancel focused).
pub fn dialog(s: &App, p: &Pal) -> Vec<El> {
    let Some(pending) = &s.terminal.pending_paste else { return Vec::new() };
    let (lines, count) = shown(&pending.text);
    let kit = crate::palette::kit(p);
    let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.answer_paste(false, tx))
        .build(kit)
        .key("terminal-paste-cancel")
        .autofocus();
    let paste = crate::widgets::button(t!("rune.terminal.paste.confirm"), ButtonVariant::Contrast)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.answer_paste(true, tx))
        .build(kit)
        .key("terminal-paste-confirm");
    let title = if count == 1 { t!("rune.terminal.paste.titleOne").to_owned() } else { t!("rune.terminal.paste.title", count = count.to_string()) };
    let mut preview = col().key("terminal-paste-preview").w_full().padding(8.0).radius(4.0).fill(p.layer2).clip_soft(0.0);
    for (i, line) in lines.iter().enumerate() {
        preview = preview.child(text(line.clone()).key(format!("terminal-paste-line-{i}")).font(CODE).color(p.text).truncate());
    }
    if count > SHOWN_LINES {
        preview = preview.child(text(t!("rune.terminal.paste.more", count = (count - SHOWN_LINES).to_string())).font(crate::widgets::BODY).color(p.muted));
    }
    let card = col()
        .key("terminal-paste-card")
        .role("alertdialog")
        .label(title.clone())
        .w(480.0)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .modal_tx(true, |s: &mut App, tx: &mut Tx| s.answer_paste(false, tx))
        .child(
            col()
                .key("terminal-paste-header")
                .w_full()
                .padding(16.0)
                .gap(8.0)
                .child(text(title).font(TITLE).color(p.text))
                .child(text(t!("rune.terminal.paste.description")).font(crate::widgets::BODY).color(p.muted).wrap().w_full())
                .child(preview),
        )
        .child(row().key("terminal-paste-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).child(paste));
    vec![
        el().key("terminal-paste-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim),
        el().key("terminal-paste-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_asks_only_when_it_would_run_something() {
        assert_eq!(judge("ls", false), Paste::Send("ls".into()));
        assert_eq!(judge("ls\r\n", false), Paste::Send("ls".into()), "a trailing break waits for Enter");
        assert_eq!(judge("ls\npwd", false), Paste::Ask);
        assert_eq!(judge("a\x1b[2Jb", false), Paste::Ask, "control characters");
        assert_eq!(judge("ls\npwd\n", true), Paste::Send("ls\npwd\n".into()), "bracketed: the program sees one paste");
        assert_eq!(judge("x\x1b[201~rm -rf .\n", true), Paste::Ask, "would end the bracket early");
        assert_eq!(judge("a\tb", false), Paste::Send("a\tb".into()));
    }

    #[test]
    fn the_question_shows_lines_and_control_characters() {
        let (lines, count) = shown("echo one\r\necho \x1b two\n");
        assert_eq!((lines, count), (vec!["echo one".to_owned(), "echo \u{241b} two".to_owned()], 2));
    }
}
