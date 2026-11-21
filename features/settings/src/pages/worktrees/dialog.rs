//! Settings › Worktrees' confirmations (`workspaces.tsx`): `DialogDeleteWorkspace`, which
//! checks the worktree before Delete, and `DialogDeleteWorkspaces` for Delete all and Delete
//! without sessions. Both are Desktop's `<Dialog fit>` over its scrim.

use opencode_i18n::t;
use opencode_kit::path;
use opencode_ui::widgets::{self, DialogSize, Fit, dialog_window, sized};
use rune::{Color, Cx, El, TextStyle, Tx, col, el, row, spacer, text};
use rune_kit::ButtonVariant;
use opencode_ui::focus::FocusExt;
use opencode_ui::palette::{Elevate as _, Pal, kit};

use super::delete::{Event, Phase, Scope};
use super::{dispatch, messages};
use crate::host::SettingsHost;

/// `dialog-title`: 15 on 20 at 530, -0.13 tracking.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// The folder: `font-mono font-medium leading-4` at the description's 13.
const FOLDER: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).mono();

fn cancel_confirm(s: &mut dyn SettingsHost, attempt: u64, tx: &mut Tx) {
    tx.instant();
    dispatch(s, Event::ConfirmCancelled { attempt });
}

fn retry_inspect(s: &mut dyn SettingsHost, attempt: u64, tx: &mut Tx) {
    tx.instant();
    dispatch(s, Event::InspectRetried { attempt });
}

fn accept_confirm(s: &mut dyn SettingsHost, attempt: u64, tx: &mut Tx) {
    tx.instant();
    dispatch(s, Event::ConfirmAccepted { attempt });
}

/// The confirmation open now, if any.
pub fn dialog<H: SettingsHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let machine = &s.settings_pages().worktrees.delete;
    let (attempt, title, lines, retry, can_delete, label) = match &machine.phase {
        Phase::ConfirmingMany { attempt, scope, targets, .. } => {
            let n = targets.len();
            let (title, confirmation, warning) = match scope {
                Scope::Unused => (
                    t!("settings.workspaces.deleteWithoutSessions"),
                    opencode_i18n::plural!("settings.workspaces.deleteWithoutSessions.confirm", n),
                    t!("settings.workspaces.deleteWithoutSessions.warning"),
                ),
                _ => (
                    t!("settings.workspaces.deleteAll"),
                    opencode_i18n::plural!("settings.workspaces.deleteAll.confirm", n),
                    t!("settings.workspaces.deleteAll.warning"),
                ),
            };
            let lines = col().gap(8.0).w_full().child(body(p, confirmation)).child(body(p, warning.to_owned()));
            (*attempt, title.to_owned(), lines, false, true, t!("settings.workspaces.delete.button"))
        }
        Phase::ConfirmingOne { attempt, target, .. } => {
            let (checking, failed, can_delete, inspection) = machine.confirm_view();
            let found = if failed { vec![t!("workspace.status.error").to_owned()] } else { inspection.map(messages).unwrap_or_default() };
            let folder = col()
                .gap(4.0)
                .w_full()
                .child(text(if checking { t!("workspace.status.checking") } else { t!("workspace.delete.location") }).font(widgets::BODY).color(p.faint))
                .child(
                    row().w_full().child(
                        col()
                            .shrink()
                            .min_w(0.0)
                            .px(4.0)
                            .py(2.0)
                            .radius(4.0)
                            .fill(p.text.alpha(p.text.a * 0.08))
                            .child(text(target.directory.chars().flat_map(|c| [c, '\u{200b}']).collect::<String>()).font(FOLDER).color(p.text).wrap()),
                    ),
                );
            let lines = col()
                .gap(8.0)
                .w_full()
                .child(folder)
                .child(body(p, t!("settings.workspaces.delete.warning").to_owned()))
                .children(found.into_iter().map(|m| body(p, m)));
            let name = path::basename(&target.directory).to_owned();
            (*attempt, t!("workspace.delete.confirm", name = name), lines, failed, can_delete, t!("workspace.delete.button"))
        }
        _ => return Vec::new(),
    };
    let k = kit(p);
    let cancel = widgets::button(t!("common.cancel"), ButtonVariant::Neutral)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| cancel_confirm(s, attempt, tx))
        .build(k)
        .key("worktree-delete-cancel")
        .autofocus();
    let again = retry.then(|| {
        widgets::button(t!("common.retry"), ButtonVariant::Neutral)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| retry_inspect(s, attempt, tx))
            .build(k)
            .key("worktree-delete-retry")
    });
    let delete = widgets::button(label, ButtonVariant::Danger)
        .disabled(!can_delete)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| accept_confirm(s, attempt, tx))
        .build(k)
        .key("worktree-delete-confirm");
    let delete = if p.dark && p.theme == 0 && can_delete {
        delete.fill(opencode_ui::palette::flat(Color::WHITE.alpha(0.06), p.layer1))
    } else {
        delete
    };
    let inner = col()
        .w_full()
        .clip_rounded(12.0)
        .child(
            row()
                .key("worktree-delete-header")
                .w_full()
                .padding(16.0)
                .items_start()
                .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0).child(text(title.clone()).font(TITLE).color(p.text).wrap().w_full()).child(lines))
                .child(close::<H>(p, attempt)),
        )
        .child(row().key("worktree-delete-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).children(again).child(delete));
    let card = col()
        .key("worktree-delete-card")
        .role("dialog")
        .label(title)
        .radius(12.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .modal_tx(true, move |s: &mut H, tx: &mut Tx| cancel_confirm(s, attempt, tx))
        .child(inner);
    let card = sized(card, DialogSize::Normal, Fit::Content, dialog_window(cx, s.zoom()));
    widgets::modal("worktree-delete", p, card, None)
}

fn close<H: SettingsHost + 'static>(p: &Pal, attempt: u64) -> El {
    el().key("worktree-delete-close")
        .role("button")
        .label_static(t!("common.close"))
        .size(20.0, 20.0)
        .mt(-2.0)
        .mr(-2.0)
        .radius(4.0)
        .center()
        .focus_default(p)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| cancel_confirm(s, attempt, tx))
        .child(rune::icon(&widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

fn body(p: &Pal, words: String) -> El {
    text(words).font(widgets::BODY).color(p.muted).wrap().w_full()
}
