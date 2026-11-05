use rune::{El, Tx, row, text};
use crate::{App, palette::Pal, view::UI_SMALL};
use super::State;

pub fn footer(s: &App, p: &Pal, id: &str) -> Option<El> {
    let state = s.unsent.state(id)?;
    let label = match state {
        State::Unconfirmed => "Not confirmed",
        State::Failed => "Failed to send",
        _ => return None,
    };
    let mut line = row().key("send-status").h(20.0).gap(8.0).center_y()
        .child(text(label).font(UI_SMALL).color(p.faint));
    if state == State::Failed {
        let retry = id.to_owned();
        line = line.child(action(p, "retry-send", "Retry", move |s| s.retry_send(&retry)));
        if s.composer.text.is_empty() && !s.attach.any() {
            let edit = id.to_owned();
            line = line.child(action(p, "edit-send", "Edit", move |s| s.edit_failed_send(&edit)));
        }
    }
    Some(line)
}

fn action(p: &Pal, key: &str, label: &str, run: impl Fn(&mut App) + 'static) -> El {
    row().key(key.to_owned()).role("button").label(label.to_owned()).help(label.to_owned())
        .h(20.0).px(4.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| { tx.instant(); run(s); })
        .child(text(label.to_owned()).font(UI_SMALL).color(p.muted))
}
