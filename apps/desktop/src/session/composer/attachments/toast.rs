//! The toast that lists the uploads in flight (`attachments/uploads.tsx` `UploadToast`): one
//! persistent toast while any file is copying, a row per file with its name, its percent and a
//! bar, and a button that cancels it. It goes when the last upload settles. It stacks with the
//! status toasts (`status_toast`) as Sonner stacks them.

use rune::{El, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::attachments::uploads::Upload;
use crate::palette::{Elevate as _, Pal};

/// `upload-row-label`: 13 on the base line, -0.04 tracking; the name at 530.
const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const PERCENT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04).features("tnum");

/// The upload toast's card, which `status_toast::view` places in the stack; its rows hide while
/// it's behind the front toast of a collapsed stack.
pub fn card(s: &App, p: &Pal, content_shows: bool) -> El {
    let list = s.attach.uploads.list();
    let content = col()
        .key("upload-toast-content")
        .w_full()
        .gap(4.0)
        .opacity(if content_shows { 1.0 } else { 0.0 })
        .opacity_motion(rune::Anim::new(rune::Curve::tween(rune_kit::toast_stack::CONTENT_FADE_SECS, rune::Easing::EASE)))
        .children(list.iter().enumerate().map(|(i, u)| upload_row(p, u, i > 0)));
    col()
        .role("status")
        .padding(12.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(content)
}

/// `upload-row`: the name and percent over the bar and its cancel, 4 apart; 8 more above every
/// row after the first.
fn upload_row(p: &Pal, u: &Upload, after: bool) -> El {
    let percent = u.percent();
    let label = row()
        .key("upload-row-label")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(text(u.name.clone()).key("upload-row-name").font(NAME).color(p.text).truncate().shrink().min_w(0.0))
        .child(el().grow(1.0))
        .child(text(opencode_i18n::t!("prompt.toast.uploading.percent", percent = percent)).key("upload-row-percent").font(PERCENT).color(p.muted));
    // `upload-progress`: 4 tall, round, `bg-layer-02`; its bar `icon-base`, its width moving
    // over 160 ms ease-out.
    let bar = el()
        .key("upload-progress-bar")
        .h_full()
        .w_frac(percent.min(100) as f32 / 100.0)
        .radius(2.0)
        .fill(p.icon)
        .motion(Spring::new(0.16, 0.0));
    let track = el().key("upload-progress").role("progressbar").label(u.name.clone()).grow(1.0).h(4.0).radius(2.0).clip_soft(0.0).fill(p.layer2).child(bar);
    let id = u.id.clone();
    let cancel = row()
        .key("upload-row-cancel")
        .role("button")
        .label(opencode_i18n::t!("prompt.toast.uploading.cancelFile", filename = u.name.clone()))
        .size(16.0, 16.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .hover_group()
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.cancel_upload(&id);
        })
        .child(icon(&ic::OUTLINE_XMARK, 12.0).color(p.icon_muted))
        .child(icon(&ic::OUTLINE_XMARK, 12.0).color(p.icon).absolute(2.0, 2.0).group_opacity(0.0, 1.0));
    let track_row = row().key("upload-row-track").w_full().gap(8.0).center_y().child(track).child(cancel);
    col().key(format!("upload-row-{}", u.id)).w_full().gap(4.0).mt(if after { 8.0 } else { 0.0 }).child(label).child(track_row)
}
