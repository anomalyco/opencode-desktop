use super::super::*;

pub(in crate::view) fn small_button(p: &Pal, key: String, label: &str, primary: bool) -> El {
    row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(if primary { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI_MED).color(p.text))
}

pub(in crate::view) fn requests(s: &App, p: &Pal, id: &str) -> El {
    let mut dock = col().key("requests").w_full().center().px(24.0).gap(8.0);
    // Questions take the composer's place (`question.rs`).
    for r in s.pending_for(id).into_iter().filter(|r| matches!(r, Pending::Permission { .. })) {
        dock = dock.child(request_card(p, r));
    }
    dock
}

pub(in crate::view) fn request_card(p: &Pal, r: &Pending) -> El {
    let rid = r.id().to_owned();
    let card = col()
        .key(format!("request-{rid}"))
        .w_full()
        .max_w(CONTENT_W)
        .padding(14.0)
        .gap(10.0)
        .radius(12.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .border(0.5, p.accent);
    match r {
        Pending::Permission { action, resources, message, .. } => {
            let mut c = card.child(text(format!("Allow {action}?")).font(UI_MED).color(p.text));
            if !message.is_empty() {
                c = c.child(text(message.clone()).font(UI).color(p.muted).wrap());
            }
            for (i, res) in resources.iter().take(6).enumerate() {
                c = c.child(text(res.clone()).key(format!("res-{i}")).font(CODE).color(p.muted).truncate());
            }
            let b = |key: &str, label: &str, decision: &'static str, primary: bool| {
                let rid = rid.clone();
                small_button(p, format!("{key}-{rid}"), label, primary).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.reply_permission(&rid, decision);
                })
            };
            c.child(
                row()
                    .key("actions")
                    .gap(6.0)
                    .child(b("once", "Allow once", "once", true))
                    .child(b("always", "Always allow", "always", false))
                    .child(b("reject", "Reject", "reject", false)),
            )
        }
        Pending::Form { .. } => card,
    }
}
