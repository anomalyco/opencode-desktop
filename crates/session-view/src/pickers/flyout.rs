//! The lit model's flyout beside its row in the model dialog (`select-dialog.tsx:1057-1121`,
//! `select-dialog.css:226-245`): Default and each of its efforts, the one it would run with
//! checked, then Favorite (or Unfavorite) with its shortcut, then what the model is. Resting on
//! the row opens it ([`super::hover`]); picking an effort picks the model with it. It appears at
//! once, 160 pt wide, 6 pt past the row's end with its first option level with the row
//! (`placeFlyout`, `select-dialog-machine.ts:348-359`).

use opencode_kit::model::capitalize;
use rune::{Align, Color, El, Side, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::hover::ModelHover;
use super::{Kind, PickerHost};
use crate::palette::Pal;
use crate::popover::{self as pop, MENU};
use crate::style::UI;

/// `FLYOUT_WIDTH_PX`.
const W: f32 = 160.0;
/// `FLYOUT_GAP_PX`: from the row's end.
const GAP: f32 = 6.0;
/// The model stats that close it: 11 on 16 (`select-dialog.css:238-244`).
const STATS: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0);
/// `keybind-v2-label`: 11, 530, line-height 1, 0.05 tracking, tabular figures (`keybind.css:55-75`).
const KEYCAP: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05).features("\"tnum\", \"lnum\"");

/// The flyout's actions, for any [`PickerHost`].
pub trait EffortFlyout: PickerHost + Sized + 'static {
    /// Opens the model `key`'s flyout beside its row, or closes it when it's open.
    fn toggle_effort_flyout(&mut self, key: &str) {
        self.pin_effort_flyout(key, &mut Tx::default());
        self.pickers_mut().search.focused = true;
    }

    /// Picks the model `key` with `effort` ("" for its default).
    fn choose_effort(&mut self, key: &str, effort: &str, tx: &mut Tx) {
        self.pickers_mut().stage_effort(key, effort);
        self.choose_picker_tx(key, tx);
    }
}

impl<S: PickerHost + 'static> EffortFlyout for S {}

/// What the flyout hangs from, inside the model row `key` while its flyout is open: a line
/// across the row 2 pt above it, so the flyout's first option, 2 pt into it, lands level with
/// the row.
pub fn anchor<S: PickerHost>(s: &S, key: &str) -> Option<El> {
    (s.pickers().hover.flyout_model() == Some(key) && s.pickers().open == Some(Kind::Model)).then_some(())?;
    Some(el().key("effort-anchor").pin(Some(0.0), Some(-MENU.inset), Some(0.0), None).h(0.0).transition(Transition::none()))
}

/// The flyout, when one is open.
pub fn view<S: PickerHost>(s: &S, p: &Pal) -> Option<El> {
    let pk = s.pickers();
    let flyout = pk.hover.flyout_model().filter(|_| pk.open == Some(Kind::Model))?;
    let model = pk.models.iter().find(|m| m.key == flyout)?;
    let lit = pk.hover.lit();
    let now = s.model_effort(flyout).unwrap_or_default();
    let mut list = col().key("effort-options").w_full().py(MENU.inset);
    // A model without efforts still lists Default, its only option.
    for effort in std::iter::once(String::new()).chain(model.variants.iter().cloned()) {
        let label = if effort.is_empty() { opencode_i18n::t!("common.default").to_owned() } else { capitalize(&effort) };
        let id = format!("effort-{}", if effort.is_empty() { "default" } else { effort.as_str() });
        let check = icon(&ic::CHECK, 14.0).color(p.icon);
        let indicator = row().key("indicator").size(pop::SLOT, pop::SLOT).center().children((effort == now).then_some(check));
        let key = flyout.to_owned();
        list = list.child(item::<S>(p, lit, &id, "menuitemradio", &label, indicator, move |s: &mut S, tx: &mut Tx| s.choose_effort(&key, &effort, tx)));
    }
    list = list.child(separator(p, "effort-separator"));
    let favorite = if pk.prefs.is_favorite(flyout) { opencode_i18n::t!("dialog.model.unfavorite") } else { opencode_i18n::t!("dialog.model.favorite") };
    let key = flyout.to_owned();
    list = list.child(item::<S>(p, lit, "effort-favorite", "menuitem", favorite, keycaps(p), move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        // The row moves between Favorites and its group, so the flyout would be left beside nothing.
        s.toggle_favorite(&key);
        s.model_flyout_dismissed(tx);
    }));
    list = list.child(separator(p, "details-separator"));
    list = list.child(details(p, &model.group, pk.limits.get(&model.key).copied().unwrap_or(0), &model.inputs, model.tools));
    let menu = super::hover::flyout::<S>(pop::surface::<S>(p, MENU, "effort-flyout", opencode_i18n::t!("ui.promptInput.chooseVariant")).w(W).child(list));
    Some(menu.anchored_fit("effort-anchor", Side::Right, Align::Start, GAP))
}

/// A `menu-v2-item` (`menu.css:35-161`): 28 pt, 12 pt before its words and 6 after, 4 pt
/// corners, on the hover overlay while the pointer is on it, inside the flyout's 2 pt inset.
fn item<S: PickerHost>(p: &Pal, lit: Option<&str>, id: &str, role: &'static str, label: &str, end: El, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let option = id.to_owned();
    let lit = lit == Some(id);
    let r = row().key(id.to_owned()).role(role).label(label.to_owned()).h(MENU.row_h).w_full().pl(MENU.pad_l).pr(MENU.pad_r).gap(8.0).center_y().radius(MENU.row_radius)
        .fill(if lit { p.hover } else { Color::TRANSPARENT })
        .transition(Transition::none())
        .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            tx.instant();
            super::hover::option_hovered(s, &option, inside);
        })
        .on_press_tx(press)
        .child(text(label.to_owned()).key("label").font(UI).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
        .child(end);
    row().key(format!("{id}-inset")).w_full().px(MENU.inset).child(r)
}

/// `menu-v2-separator`: 1 pt across the whole flyout, 2 pt above and below.
fn separator(p: &Pal, key: &str) -> El {
    el().key(key.to_owned()).w_full().h(1.0).my(MENU.inset).fill(p.border_muted)
}

/// Favorite's shortcut as Desktop's neutral `Keybind` (`keybind.css`): 2 pt apart, each key 14
/// tall and at least 14 wide with 4 pt sides and 2 pt corners on `bg-layer-03`, uppercase in
/// `text-muted`.
fn keycaps(p: &Pal) -> El {
    let caps = crate::keymap::Bind::Favorite.keybind().chords().first().map(|c| crate::keymap::caps(*c)).unwrap_or_default();
    caps.iter().fold(row().key("keys").gap(2.0).center_y(), |r, c| {
        r.child(row().h(14.0).min_w(14.0).px(4.0).center().radius(2.0).fill(p.layer3).child(text(c.to_uppercase()).font(KEYCAP).color(p.muted)))
    })
}

/// `ModelDetails` compact and standalone (`details.tsx:150-172`): its provider and context
/// window, then what it reads and whether it calls tools, 12 pt in, 8 above and below.
fn details(p: &Pal, provider: &str, context: u64, inputs: &[String], tools: bool) -> El {
    let summary = opencode_i18n::t!("model.details.summary", provider = provider, context = compact(context));
    let abilities: Vec<&str> = inputs.iter().map(|i| match i.as_str() {
        "text" => opencode_i18n::t!("model.details.input.text"),
        "image" => opencode_i18n::t!("model.details.input.image"),
        "pdf" => opencode_i18n::t!("model.details.input.pdf"),
        "audio" => opencode_i18n::t!("model.details.input.audio"),
        _ => opencode_i18n::t!("model.details.input.video"),
    }).chain(tools.then(|| opencode_i18n::t!("model.details.tools"))).collect();
    col().key("effort-details").w_full().px(12.0).py(8.0)
        .child(text(summary).key("summary").font(STATS).color(p.muted).wrap())
        .children((!abilities.is_empty()).then(|| text(abilities.join(", ")).key("abilities").font(STATS).color(p.muted).wrap()))
}

/// A token count as `Intl.NumberFormat`'s compact notation with no fractions, truncated:
/// 200K, 1M.
fn compact(n: u64) -> String {
    [(1_000_000_000_000, "T"), (1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")]
        .into_iter()
        .find(|(at, _)| n >= *at)
        .map_or_else(|| n.to_string(), |(at, unit)| format!("{}{unit}", n / at))
}
