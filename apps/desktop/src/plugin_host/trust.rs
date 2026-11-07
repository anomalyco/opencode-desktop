//! The first-load trust prompt. A native plugin runs with the user's full access, so a library
//! that didn't ship with the app (`plugins-override/`, `<data>/plugins/`) waits in
//! `PendingTrust` until the user OKs it here, drawn as the app's other confirmations are (a
//! scrim and a card). The OK is for that file: its SHA-256 is remembered, and a changed library
//! asks again. A library Windows marked as downloaded is unblocked on the way.

use rune::{Color, El, TextStyle, Tx, col, el, row, spacer, text};

use super::lifecycle::PluginState;
use super::{motw, save_state, swap};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};

/// The slot whose prompt shows now: the first waiting one not put off this launch.
fn shown(app: &App) -> Option<usize> {
    let host = &app.plugins;
    host.slots.iter().position(|s| s.state == PluginState::PendingTrust && !host.trust_later.contains(&s.manifest.id))
}

/// The prompt, over everything (`view::app`'s layers).
pub fn dialog(s: &App, p: &Pal) -> Vec<El> {
    let Some(i) = shown(s) else { return Vec::new() };
    let slot = &s.plugins.slots[i];
    let (id, name) = (slot.manifest.id.clone(), slot.manifest.name.clone());
    let button = |key: &str, label: &str, ink: Color, fill: Option<Color>| {
        let mut b = row().key(key.to_owned()).role("button").label(label.to_owned()).h(28.0).px(12.0).radius(6.0).center().hover(p.hover);
        if let Some(f) = fill {
            b = b.fill(f).hover(f.alpha(0.9));
        }
        b.child(text(label.to_owned()).font(UI_MED).color(ink))
    };
    let enable = if slot.blocked { "Unblock and enable" } else { "Enable" };
    let version = if slot.manifest.version.is_empty() { String::new() } else { format!(" {}", slot.manifest.version) };
    let mut card = col()
        .key("plugin-trust-card")
        .w(420.0)
        .padding(20.0)
        .gap(6.0)
        .radius(12.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(8.0, 32.0, p.shadow_far)
        .child(text(format!("Enable native plugin \"{name}\"?")).font(TextStyle::new(15.0, 600.0).lh(24.0)).color(p.text))
        .child(
            text(format!("{name}{version} runs as native code with your full access to this computer, like an app you install. Only enable plugins you trust."))
                .font(UI)
                .color(p.muted)
                .wrap(),
        );
    if !slot.manifest.description.is_empty() {
        card = card.child(text(slot.manifest.description.clone()).font(UI).color(p.muted).wrap().mt(6.0));
    }
    if slot.blocked {
        card = card.child(
            row()
                .key("plugin-trust-blocked")
                .mt(8.0)
                .gap(8.0)
                .padding(10.0)
                .radius(8.0)
                .fill(p.layer1)
                .border(0.5, p.border_muted)
                .child(el().size(8.0, 8.0).radius(4.0).fill(p.warning).mt(5.0))
                .child(text("Blocked by Windows: this file came from another computer. Enabling it also unblocks it.").font(UI).color(p.text).wrap().shrink()),
        );
    }
    card = card.child(text(slot.dir.display().to_string()).font(crate::view::UI_SMALL).color(p.faint).wrap().selectable(true).mt(6.0));
    let (later, ok) = (id.clone(), id);
    card = card.child(
        row()
            .key("plugin-trust-footer")
            .gap(8.0)
            .mt(14.0)
            .child(spacer())
            .child(button("plugin-trust-later", "Not now", p.text, None).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.plugins.trust_later.insert(later.clone());
            }))
            .child(button("plugin-trust-enable", enable, p.on_fill, Some(p.bg_accent)).on_press_tx(move |s: &mut App, tx: &mut Tx| accept(s, &ok, tx))),
    );
    vec![
        el().key("plugin-trust-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.dim),
        el().key("plugin-trust-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// Trusts plugin `id`'s library as it is now, unblocks it if Windows marked it, and loads it.
fn accept(app: &mut App, id: &str, tx: &mut Tx) {
    tx.instant();
    let Some(i) = app.plugins.slots.iter().position(|s| s.manifest.id == id) else { return };
    let slot = &mut app.plugins.slots[i];
    if slot.blocked
        && let Some(path) = slot.library().map(std::path::Path::to_path_buf)
    {
        match motw::unblock(&path) {
            Ok(()) => slot.blocked = false,
            Err(e) => {
                slot.error = Some(e.clone());
                crate::status_toast::fail(app, "Couldn't unblock the plugin", e);
                return;
            }
        }
    }
    let Some(hash) = slot.hash.clone() else { return };
    let path = slot.library().map(std::path::Path::to_path_buf);
    let host = &mut app.plugins;
    host.trusted.insert(id.to_owned(), hash);
    host.disabled.remove(id);
    // The library loads off the app's thread (its initialisers run there too); `poll` sets it
    // up once it's in, so the prompt closes in this frame.
    host.trust_later.insert(id.to_owned());
    match path {
        Some(path) => super::hot::load_in_background(host.wake.clone(), host.ready.clone(), i, path, host.shadow_dir.clone(), id.to_owned()),
        None => {
            swap::load(app, i, None);
            swap::settle(app);
        }
    }
    save_state(app);
}
