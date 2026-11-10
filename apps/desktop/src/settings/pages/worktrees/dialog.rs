//! Settings › Worktrees' confirmations (`workspaces.tsx`): `DialogDeleteWorkspace`, which
//! checks the worktree before Delete, and `DialogDeleteWorkspaces` for Delete all and Delete
//! without sessions. Both are Desktop's `<Dialog fit>` over its scrim: the title group in the
//! header, Cancel (neutral, focused) and Delete (danger) at the footer's end.

use opencode_kit::path;
use rune::{Color, El, TextStyle, Tx, col, el, row, spacer, text};
use rune_kit::ButtonVariant;

use super::delete::{Event, Phase, Scope};
use super::{dispatch, messages};
use crate::App;
use crate::palette::{Elevate as _, Pal, kit};
use opencode_i18n::t;

/// `dialog-title`: 15 on 20 at 530, -0.13 tracking.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// The folder: `font-mono font-medium leading-4` at the description's 13 (`text-xs` loses to it).
const FOLDER: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).mono();

/// The confirmation open now, if any.
pub fn dialog(s: &App, p: &Pal) -> Vec<El> {
    let machine = &s.settings_pages.worktrees.delete;
    let (attempt, title, lines, retry, can_delete, label) = match &machine.phase {
        Phase::ConfirmingMany { attempt, scope, targets, .. } => {
            let n = targets.len();
            let (title, confirmation, warning) = match scope {
                Scope::Unused => (t!("settings.workspaces.deleteWithoutSessions"), opencode_i18n::plural!("settings.workspaces.deleteWithoutSessions.confirm", n), t!("settings.workspaces.deleteWithoutSessions.warning")),
                _ => (t!("settings.workspaces.deleteAll"), opencode_i18n::plural!("settings.workspaces.deleteAll.confirm", n), t!("settings.workspaces.deleteAll.warning")),
            };
            let lines = col().gap(8.0).w_full()
                .child(body(p, confirmation))
                .child(body(p, warning.to_owned()));
            (*attempt, title.to_owned(), lines, false, true, t!("settings.workspaces.delete.button"))
        }
        Phase::ConfirmingOne { attempt, target, .. } => {
            let (checking, failed, can_delete, inspection) = machine.confirm_view();
            let found = if failed { vec![t!("workspace.status.error").to_owned()] } else { inspection.map(messages).unwrap_or_default() };
            let folder = col().gap(4.0).w_full()
                .child(text(if checking { t!("workspace.status.checking") } else { t!("workspace.delete.location") }).font(crate::widgets::BODY).color(p.faint))
                // `w-fit max-w-full break-all`: as wide as the folder, breaking between any two
                // characters at the dialog's width (a zero-width space after each, as it isn't selectable).
                .child(row().w_full().child(col().shrink().min_w(0.0).px(4.0).py(2.0).radius(4.0).fill(p.text.alpha(p.text.a * 0.08))
                    .child(text(target.directory.chars().flat_map(|c| [c, '\u{200b}']).collect::<String>()).font(FOLDER).color(p.text).wrap())));
            let lines = col().gap(8.0).w_full()
                .child(folder)
                .child(body(p, t!("settings.workspaces.delete.warning").to_owned()))
                .children(found.into_iter().map(|m| body(p, m)));
            let name = path::basename(&target.directory).to_owned();
            (*attempt, t!("workspace.delete.confirm", name = name), lines, failed, can_delete, t!("workspace.delete.button"))
        }
        _ => return Vec::new(),
    };
    let k = kit(p);
    let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Neutral)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dispatch(s, Event::ConfirmCancelled { attempt });
        })
        .build(k)
        .key("worktree-delete-cancel")
        .autofocus();
    let again = retry.then(|| crate::widgets::button(t!("common.retry"), ButtonVariant::Neutral)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dispatch(s, Event::InspectRetried { attempt });
        })
        .build(k)
        .key("worktree-delete-retry"));
    let delete = crate::widgets::button(label, ButtonVariant::Danger)
        .disabled(!can_delete)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dispatch(s, Event::ConfirmAccepted { attempt });
        })
        .build(k)
        .key("worktree-delete-confirm");
    // Dark's neutral fill is `alpha-light-6` over the dialog's `layer-01`, flattened, as the
    // session delete's danger button (`session_menu_view::confirm`).
    let delete = if p.dark && p.theme == 0 && can_delete { delete.fill(session_view::dock::flat(Color::WHITE.alpha(0.06), p.layer1)) } else { delete };
    let card = col()
        .key("worktree-delete-card")
        .role("dialog")
        .label(title.clone())
        .w(480.0)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .modal_tx(true, move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dispatch(s, Event::ConfirmCancelled { attempt });
        })
        .child(row().key("worktree-delete-header").w_full().padding(16.0).items_start()
            .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0)
                .child(text(title).font(TITLE).color(p.text).wrap().w_full())
                .child(lines))
            .child(close(p, attempt)))
        .child(row().key("worktree-delete-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).children(again).child(delete));
    vec![
        el().key("worktree-delete-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim),
        el().key("worktree-delete-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// The header's close button: 20 square, 2 out at the top and end (`dialog-close-button`).
fn close(p: &Pal, attempt: u64) -> El {
    el().key("worktree-delete-close").role("button").label(t!("common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dispatch(s, Event::ConfirmCancelled { attempt });
        })
        .child(rune::icon(&crate::widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

/// The description's 13 on 20, muted.
fn body(p: &Pal, words: String) -> El {
    text(words).font(crate::widgets::BODY).color(p.muted).wrap().w_full()
}
