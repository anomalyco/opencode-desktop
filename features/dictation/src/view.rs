//! Composer toolbar mic/stop control and below-toolbar dictation status line.
//!
//! Element construction stays non-generic (`&dyn DictationHost`); only [`toolbar`] and [`status`]
//! are generic over `H: DictationHost + 'static` to bind 1-line `rune::El` press callbacks.

use opencode_ui::focus::FocusExt;
use hex_client::Fix;
use rune::{El, Icon, MotionIntent, Spring, Tx, el, icon, row, text};
use opencode_ui::palette::Pal;
use opencode_ui::style::{UI_SMALL, activity_ring_sized};

use crate::machine::{Phase, Words};
use crate::{DictationHost, apply_fix, resolve_backend, toggle};

const MIC: Icon = Icon {
    name: "outline-mic",
    view: 16.0,
    body: r#"<path d="M8 1.5C6.89543 1.5 6 2.39543 6 3.5V8C6 9.10457 6.89543 10 8 10C9.10457 10 10 9.10457 10 8V3.5C10 2.39543 9.10457 1.5 8 1.5Z" stroke="currentColor"/><path d="M3.5 7.5V8C3.5 10.4853 5.51472 12.5 8 12.5C10.4853 12.5 12.5 10.4853 12.5 8V7.5" stroke="currentColor" stroke-linecap="square"/><path d="M8 12.5V14.5" stroke="currentColor" stroke-linecap="square"/>"#,
};

fn toolbar_button(host: &dyn DictationHost, p: &Pal) -> Option<(El, bool)> {
    let d = host.dictation();
    let backend = resolve_backend(host);
    // Off Windows the mic hides without Hex: the first toolbar starts the watcher.
    // Its waker comes with the window's first frame (`on_ready`).
    if !hex_client::MIC_WITHOUT_HEX
        && !d.watching.get()
        && let Some(wake) = host.dictation_waker()
    {
        backend.watch(Some(&wake));
        d.watching.set(true);
    }
    if !d.shows_mic(backend) {
        return None;
    }
    let finishing = matches!(d.phase, Phase::Finishing { .. });
    // The mic and the stop swap at once on the press, as iOS's do.
    let still = Spring::NONE;
    let mut b = el()
        .key("mic")
        .role("button")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .motion(still)
        .motion_intent(MotionIntent::CUT)
        .preserve_focus_on_press(true)
        // Desktop's composer has no Tab stop after its variant select: Tab goes on to the
        // titlebar, so the mic keeps out of the Tab order (its shortcut still toggles it).
        .tab_index(-1)
        // Inside the composer's clip its keyboard ring sits 2 in (`editor.css:70-73`).
        .focus_inset(p);
    b = match &d.phase {
        Phase::Idle | Phase::Failed(_) => b
            .label_static(opencode_i18n::t!("rune.command.prompt.dictate"))
            .help(opencode_i18n::t!("rune.command.prompt.dictate"))
            .help_keys(host.dictate_keycaps())
            .child(icon(&MIC, 16.0).color(p.icon_muted).motion(still)),
        Phase::Finishing { .. } => b
            .label_static(opencode_i18n::t!("rune.dictation.finishing"))
            .fill(p.layer2)
            .child(activity_ring_sized(p, 14.0)),
        // iOS's stop.fill in a tertiary fill; the same square as the run's Stop.
        Phase::Starting { .. } | Phase::Listening { .. } => b
            .label_static(opencode_i18n::t!("rune.dictation.stop"))
            .help(opencode_i18n::t!("rune.dictation.stop"))
            .help_keys(host.dictate_keycaps())
            .fill(p.layer2)
            .child(el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text).motion(still)),
    };
    Some((b, !finishing))
}

/// What sits before Send, as in the iOS app's composer: the mic, a quiet stop while it listens,
/// and a spinner in the stop while the words come. Nothing while Hex isn't there (except on
/// Windows, where the mic offers Hex's download).
pub fn toolbar<H: DictationHost + 'static>(host: &H, p: &Pal) -> Option<El> {
    let (mut b, clickable) = toolbar_button(host, p)?;
    if clickable {
        b = b.hover(p.hover).press(p.pressed).on_press_tx(|s: &mut H, tx: &mut Tx| toggle(s, tx));
    }
    // Its slot rides the composer's edge on the panel's spring; the button's own swap lands at once.
    Some(el().key("mic-slot").size(28.0, 28.0).child(b))
}

struct StatusView {
    line: El,
    fix: Option<(El, Fix)>,
}

fn status_view(host: &dyn DictationHost, p: &Pal) -> Option<StatusView> {
    let line = row().key("dictation-status").px(16.0).pb(12.0).gap(8.0).center_y();
    match &host.dictation().phase {
        Phase::Starting { slow: true, .. } => Some(StatusView {
            line: line
                .child(activity_ring_sized(p, 12.0))
                .child(text(opencode_i18n::t!("rune.dictation.preparing")).font(UI_SMALL).color(p.muted)),
            fix: None,
        }),
        Phase::Listening { heard: Some(w), .. } | Phase::Finishing { heard: Some(w), .. } => {
            heard_line(line, p, w).map(|line| StatusView { line, fix: None })
        }
        Phase::Failed(why) => {
            // In a narrow composer the reason gives way, so its fix stays whole.
            let line = line.child(text(why.says.clone()).font(UI_SMALL).color(p.danger).truncate());
            Some(StatusView {
                line,
                fix: why.fix.map(|fix| (fix_button_el(p, fix), fix)),
            })
        }
        _ => None,
    }
}

/// Under the toolbar, as in iOS: "Preparing dictation…" while Hex is slow to start, the words so
/// far while it listens, or what went wrong with what to do about it.
pub fn status<H: DictationHost + 'static>(host: &H, p: &Pal) -> Option<El> {
    let view = status_view(host, p)?;
    match view.fix {
        Some((button, fix)) => {
            Some(view.line.child(button.on_press_tx(move |s: &mut H, tx: &mut Tx| apply_fix(s, fix, tx))))
        }
        None => Some(view.line),
    }
}

/// The words so far: the settled text, cut at its start when the line is full, then the faint
/// tail Hex may still change.
fn heard_line(line: El, p: &Pal, w: &Words) -> Option<El> {
    if w.stable.is_empty() && w.tentative.is_empty() {
        return None;
    }
    let words = row()
        .key("dictation-heard")
        .shrink()
        .child(text(w.stable.clone()).font(UI_SMALL).color(p.muted).truncate_head())
        .child(text(w.tentative.clone()).font(UI_SMALL).color(p.faint));
    Some(line.child(words))
}

/// What a problem offers: Hex's download, Hex's window, or the microphone privacy page.
fn fix_button_el(p: &Pal, fix: Fix) -> El {
    row()
        .key("dictation-fix")
        .role("button")
        .label(fix.label().to_owned())
        .h(20.0)
        .px(4.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .preserve_focus_on_press(true)
        .child(text(fix.label()).font(UI_SMALL).color(p.text))
}
