//! The models a new connection offers (`FirstConnectionModels`): pick one to start with, then
//! Continue in the footer.

use rune::{El, Tx, col, el, icon, row, spacer, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use opencode_kit::model::Model;
use opencode_ui::palette::{Pal, kit};
use opencode_ui::style::UI;

use super::connection_groups;
use crate::dialog::{Event, Step};
use crate::ui::{self, BODY, LABEL};
use crate::ConnectHost;

fn select_model(host: &mut dyn ConnectHost, key: &str, tx: &mut Tx) {
    tx.instant();
    crate::connect_event(host, Event::ModelSelected(key.to_owned()), tx);
}

fn toggle_group(host: &mut dyn ConnectHost, provider: &str, collapsed: bool, tx: &mut Tx) {
    tx.instant();
    crate::connect_event(host, Event::GroupCollapsed { provider: provider.to_owned(), collapsed }, tx);
}

fn confirm_model(host: &mut dyn ConnectHost, key: &str, tx: &mut Tx) {
    crate::connect_event(host, Event::ModelConfirmed(key.to_owned()), tx);
}

pub(super) fn view<H: ConnectHost + 'static>(s: &H, p: &Pal, id: &str) -> El {
    let Some(Step::Models { selected, collapsed }) = s.connect().dialog().and_then(|o| o.dialog.provider.as_ref()).map(|p| &p.step) else {
        return col();
    };
    let groups = connection_groups(s, id);
    let row_for = |m: &Model, last: bool| {
        let checked = *selected == m.key;
        let key = m.key.clone();
        // The list clips its rows, so the ring sits inside the row (`providers/connect/models.css`).
        let shell = col().key(format!("connected-model-{}", m.key)).mx(4.0).py(4.0).child(opencode_ui::focus::inset(
            p,
            row()
                .role("radio")
                .label(m.name.clone())
                .w_full()
                .h(40.0)
                .px(12.0)
                .center_y()
                .radius(4.0)
                .hover(p.layer3)
                .on_press_tx(move |s: &mut H, tx: &mut Tx| select_model(s, &key, tx))
                .child(text(m.name.clone()).font(UI).color(p.text).grow(1.0).shrink().truncate())
                .child(el().size(16.0, 16.0).center().children(checked.then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon)))),
        ));
        if last { shell } else { col().w_full().child(shell).child(el().h(0.5).mx(16.0).fill(p.border)) }
    };
    let list = |models: &[Model]| {
        let n = models.len();
        col()
            .key("connected-models")
            .w_full()
            .radius(8.0)
            .fill(p.layer2)
            .box_shadow(Pal::ring_in(p.border_muted))
            .clip_soft(0.0)
            .children(models.iter().enumerate().map(|(at, m)| row_for(m, at + 1 == n)))
    };
    let mut scroll = col()
        .w_full()
        .gap(12.0)
        .pb(16.0)
        .child(row().key("available-models-heading").gap(6.0).center_y().child(text(opencode_i18n::t!("provider.connect.models.available")).font(LABEL).color(p.text)));
    if groups.len() == 1 {
        scroll = scroll.child(list(&groups[0].1));
    } else {
        for (info, models) in &groups {
            let open = !collapsed.contains(&info.id);
            let pid = info.id.clone();
            let mut section = col().w_full().gap(8.0).pb(if open { 8.0 } else { 0.0 }).child(
                row()
                    .key(format!("connect-group-{}", info.id))
                    .role("button")
                    .h(28.0)
                    .pt(4.0)
                    .pr(8.0)
                    .pb(4.0)
                    .pl(4.0)
                    .gap(4.0)
                    .center_y()
                    .radius(4.0)
                    .hover(p.layer2)
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_group(s, &pid, open, tx))
                    .child(
                        el().size(20.0, 20.0).center().child(
                            icon(&ic::CHEVRON_DOWN, 14.0)
                                .color(p.icon_muted)
                                .rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })
                                .mirror_in_rtl(),
                        ),
                    )
                    .child(
                        row()
                            .gap(8.0)
                            .center_y()
                            .child(ui::provider_mark(&info.id, p.icon))
                            .child(text(info.name.clone()).font(LABEL).color(p.text)),
                    ),
            );
            if open {
                section = section.child(list(models));
            }
            scroll = scroll.child(section);
        }
    }
    let can = groups.iter().any(|(_, ms)| ms.iter().any(|m| m.key == *selected));
    let pick = selected.clone();
    let footer = row().key("first-provider-model-footer").w_full().h(60.0).px(16.0).center_y().child(spacer()).child(
        ui::button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast)
            .disabled(!can)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| confirm_model(s, &pick, tx))
            .build(kit(p))
            .key("connect-continue"),
    );
    col()
        .key("connect-body")
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .shrink()
                .min_h(0.0)
                .px(20.0)
                .child(text(opencode_i18n::t!("provider.connect.models.description")).font(BODY).color(p.muted).pb(20.0))
                // `settings-panel`: no bar, as Desktop hides it (`dialog.tsx:1241`, `settings.css:593-607`).
                .child(opencode_ui::scroller::scroller("first-provider-model-scroll").hidden(col().w_full().shrink().min_h(0.0), scroll)),
        )
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(footer)
}
