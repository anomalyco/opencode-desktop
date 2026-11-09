use super::{AfterCheck, Phase, State};
use crate::{App, palette::Pal, view::UI_SMALL};
use rune::{El, Tx, row, text};

pub fn footer(s: &App, p: &Pal, id: &str) -> Option<El> {
    let state = s.unsent.state(id)?;
    if let Some(sent) = s.unsent.durable(id) {
        let label = match &sent.phase {
            Phase::Sending { then: Some(AfterCheck::Edit), .. } | Phase::Unconfirmed { then: Some(AfterCheck::Edit), .. } => "Checking delivery before editing".into(),
            Phase::Sending { then: Some(AfterCheck::Delete), .. } | Phase::Unconfirmed { then: Some(AfterCheck::Delete), .. } => "Checking delivery before deleting".into(),
            Phase::Sending { then: None, .. } => "Sending…".into(),
            Phase::Waiting { why } if why == "waiting for connection" => "Not sent · waiting for connection".into(),
            Phase::Waiting { why } if !why.is_empty() => format!("Not sent · {why} · retrying"),
            Phase::Waiting { .. } => "Not sent · waiting to send".to_string(),
            Phase::Unconfirmed { why, .. } if why.is_empty() => "Checking delivery…".into(),
            Phase::Unconfirmed { why, .. } => format!("Checking delivery · {why}"),
            Phase::Failed { why, .. } => format!("Not sent · {why}"),
            Phase::Accepted => return None,
        };
        let mut line = row().key("send-status").h(20.0).gap(8.0).center_y().child(text(label).font(UI_SMALL).color(p.faint));
        if state == State::Failed {
            let id = id.to_owned();
            line = line.child(action(p, "retry-send", opencode_i18n::t!("rune.send.retry"), move |s| s.outbox_action(&id, "outbox-retry")));
        }
        return Some(line);
    }
    let label = match state {
        State::Unconfirmed => opencode_i18n::t!("rune.send.unconfirmed"),
        State::Failed => opencode_i18n::t!("rune.send.failed"),
        _ => return None,
    };
    let mut line = row().key("send-status").h(20.0).gap(8.0).center_y().child(text(label).font(UI_SMALL).color(p.faint));
    if state == State::Failed {
        let retry = id.to_owned();
        line = line.child(action(p, "retry-send", opencode_i18n::t!("rune.send.retry"), move |s| s.retry_send(&retry)));
        if s.composer.text.is_empty() && !s.attach.any() {
            let edit = id.to_owned();
            line = line.child(action(p, "edit-send", opencode_i18n::t!("common.edit"), move |s| s.edit_failed_send(&edit)));
        }
    }
    Some(line)
}

fn action(p: &Pal, key: &str, label: &str, run: impl Fn(&mut App) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .help(label.to_owned())
        .h(20.0)
        .px(4.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            run(s);
        })
        .child(text(label.to_owned()).font(UI_SMALL).color(p.muted))
}
