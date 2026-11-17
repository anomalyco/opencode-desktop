//! The Usage dialog as Desktop draws it (`usage/dialog.tsx`, `usage/dialog.css`): a `<Dialog fit>`
//! 512 wide with Usage in its header, OpenCode Go's windows and the Console budgets in panels
//! with their spend and a track each, the callout over them when paid models are paused, and
//! the footer's reset time beside View details.

use rune::{Color, Cx, El, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, Loader};

use super::limits::{self, Action, Callout, Footer, Label, Note, Row, Sharing};
use super::Load;
use crate::App;
use crate::palette::{Elevate as _, Pal, kit};
use crate::pickers::PickerHost as _;
use crate::widgets;
use opencode_i18n::{plural, t};

/// `dialog-title`: 15 on 20 at 530.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// `Line`: 13 on 16 at 440, one line.
const LINE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const LINE_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// The callout's and the action's bodies: 13 on 20.
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// The section's workspace badge: 11 on 12 at 530.
const BADGE: TextStyle = TextStyle::new(11.0, 530.0).lh(12.0).tracking(0.05);

/// The dialog over its scrim, centred as Desktop's dialogs are.
pub fn layer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    // Closed, it reads nothing: a read of the window's width would run the view on every resize.
    if s.usage.open.is_none() {
        return Vec::new();
    }
    // Escape and a press outside close it (`modal`).
    dialog(s, p, widgets::dialog_window(cx, *s.zoom)).map_or_else(Vec::new, |d| widgets::modal("usage", p, d.modal(true, |s: &mut App| s.close_usage()), None))
}

/// The dialog in a window `win`, while open.
pub(super) fn dialog(s: &App, p: &Pal, win: [f32; 2]) -> Option<El> {
    let open = s.usage.open.as_ref()?;
    let (body, footer) = match &open.load {
        Load::Ready(info) | Load::Refreshing { info, .. } => {
            let v = limits::view(info, open.model.as_ref(), open.credits);
            (content(s, p, &v), Some(footer(p, &v, &info.workspace.usage_url)))
        }
        load => (fallback(p, load), None),
    };
    // `<Dialog fit>` (`usage/dialog.tsx`): the normal size, as tall as its content up to the
    // window less 32; past that its content scrolls with the bar hidden (`dialog-content`).
    let content = col().w_full().child(header(p)).child(col().key("usage-body").w_full().gap(16.0).child(body)).children(footer);
    let card = col()
        .key("usage-dialog")
        .role("dialog")
        .label_static(t!("usage.title"))
        .radius(12.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .child(session_view::scroller::scroller("usage-content").hidden(col().w_full().shrink().min_h(0.0), content));
    Some(widgets::sized(card, widgets::DialogSize::Normal, widgets::Fit::Content, win))
}

/// `DialogHeader` with its close button and the title group.
fn header(p: &Pal) -> El {
    let close = el().key("usage-close").role("button").label_static(t!("ui.common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.close_usage();
        })
        .child(icon(&widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted));
    row().key("usage-header").w_full().padding(16.0).items_start()
        .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0).child(text(t!("usage.title")).font(TITLE).color(p.text)))
        .child(close)
}

/// Loading, a failed read with Try again, or signed out (`dialog.tsx:128-158`): centred in a row
/// at least 120 tall, 16 in at the sides and foot. Signed out, the panel stretches to the row's
/// height and keeps its lines' width, so a long line runs it past the row's insets, centred, as
/// Chromium lays out the flex item.
fn fallback(p: &Pal, load: &Load) -> El {
    if let Load::SignedOut = load {
        let (title, body) = (t!("usage.signedOut.title"), t!("usage.signedOut.body"));
        return col().key("usage-state").w_full().px(12.0).pb(12.0).child(message(p, title, body));
    }
    let inner = match load {
        Load::Failed => col().gap(12.0).items_center()
            .child(text(t!("usage.error")).font(LINE).color(p.muted))
            .child(widgets::button(t!("usage.retry"), ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.retry_usage();
            }).build(kit(p)).key("usage-retry")),
        _ => col().child(Loader::new().build(kit(p))),
    };
    row().key("usage-state").w_full().min_h(120.0).px(16.0).pb(16.0).center().child(inner)
}

fn message(p: &Pal, title: &str, body: &str) -> El {
    panel(p, 8.0).child(col().w_full().px(16.0).gap(4.0)
        .child(line(title, LINE_MED, p.text)).child(text(body.to_owned()).font(LINE.lh(20.0)).color(p.muted)))
}

fn content(s: &App, p: &Pal, v: &limits::View) -> El {
    let workspace = v.workspace.clone().unwrap_or_else(|| t!("usage.workspace.fallback").to_owned());
    let mut out = col().key("usage-content").w_full().gap(16.0);
    if let Some(c) = v.callout {
        out = out.child(callout(p, c, &workspace));
    }
    if let Some(go) = &v.go {
        let mut panel = panel(p, 16.0);
        for (k, g) in go.iter().enumerate() {
            if k > 0 {
                panel = panel.child(divider(p));
            }
            let status = t!("usage.go.used", percent = g.meter.percent.round() as i64);
            let resets = t!("usage.go.resetsIn", duration = duration(g.meter.resets_at, *s.now_ms));
            let title = match g.window {
                "rolling" => t!("usage.go.rolling"),
                "weekly" => t!("usage.go.weekly"),
                _ => t!("usage.go.monthly"),
            };
            panel = panel.child(item()
                .child(row().w_full().center_y().child(line(title, LINE_MED, p.text).grow(1.0)).child(spend(p, &status, &resets, g.meter.limited)))
                .child(track(p, g.meter.percent / 100.0, g.meter.limited)));
        }
        out = out.child(section(p, t!("usage.section.go"), None, false, panel));
    }
    let budgets = if v.budgets.is_empty() {
        message(p, t!("usage.empty.title"), &t!("usage.empty.body", workspace = workspace))
    } else {
        let mut panel = panel(p, 16.0);
        for (k, r) in v.budgets.iter().enumerate() {
            if k > 0 {
                panel = panel.child(divider(p));
            }
            panel = panel.child(budget(p, r, &workspace));
        }
        if let Some(a) = &v.action {
            panel = panel.child(action(p, a));
        }
        panel
    };
    out.child(section(p, t!("usage.section.console"), v.workspace.as_deref(), v.dimmed, budgets))
}

fn budget(p: &Pal, r: &Row, workspace: &str) -> El {
    let (status, detail) = if r.reached {
        (t!("usage.budget.reached").to_owned(), t!("usage.budget.spentOf", spent = money(r.spent, false), limit = money(r.limit, true)))
    } else {
        (t!("usage.budget.left", amount = money(r.left, false)), t!("usage.budget.of", limit = money(r.limit, true)))
    };
    item()
        .child(row().w_full().items_start()
            .child(col().grow(1.0).shrink().min_w(0.0).gap(2.0)
                .child(line(&label(&r.label), LINE_MED, p.text))
                .child(text(scope(r, workspace)).font(LINE).color(p.muted).wrap().w_full()))
            .child(spend(p, &status, &detail, r.reached)))
        .child(track(p, r.fill, r.reached))
}

/// `label`: what a budget counts, in words.
fn label(l: &Label) -> String {
    match l {
        Label::All => t!("usage.budget.all").to_owned(),
        Label::Each => t!("usage.budget.each").to_owned(),
        Label::Vendors { vendors, each, price: Some(price) } => {
            let (vendors, price) = (opencode_i18n::list(vendors), money(*price, true));
            if *each { t!("usage.budget.vendorsPriceEach", vendors = vendors, price = price) } else { t!("usage.budget.vendorsPrice", vendors = vendors, price = price) }
        }
        Label::Vendors { vendors, each, price: None } => {
            let vendors = opencode_i18n::list(vendors);
            if *each { t!("usage.budget.vendorsEach", vendors = vendors) } else { t!("usage.budget.vendors", vendors = vendors) }
        }
        Label::Price { price, each } => {
            let price = money(*price, true);
            if *each { t!("usage.budget.priceEach", price = price) } else { t!("usage.budget.price", price = price) }
        }
        Label::Models { names, each } => {
            let models = if names.len() > 2 {
                plural!("usage.budget.modelsMore", names.len() - 2, first = names[0], second = names[1])
            } else {
                opencode_i18n::list(names)
            };
            if *each { t!("usage.budget.modelsEach", models = models) } else { models }
        }
    }
}

/// `scope`: who spends against it, then its notes, ` · ` apart.
fn scope(r: &Row, workspace: &str) -> String {
    let limit = money(r.limit, true);
    let amount = match (r.sharing, r.per_model) {
        (Sharing::You, false) => t!("usage.budget.you", limit = limit, workspace = workspace),
        (Sharing::You, true) => t!("usage.budget.youEach", limit = limit, workspace = workspace),
        (Sharing::Everyone, false) => t!("usage.budget.everyone", limit = limit, workspace = workspace),
        (Sharing::Everyone, true) => t!("usage.budget.everyoneEach", limit = limit, workspace = workspace),
        (Sharing::Group, false) => t!("usage.budget.group", limit = limit, workspace = workspace),
        (Sharing::Group, true) => t!("usage.budget.groupEach", limit = limit, workspace = workspace),
    };
    std::iter::once(amount).chain(r.notes.iter().map(|n| note(n, r))).collect::<Vec<_>>().join(" · ")
}

fn note(n: &Note, r: &Row) -> String {
    match n {
        Note::RunsOutFirst => t!("usage.note.runsOutFirst").to_owned(),
        Note::Paused => t!("usage.note.paused", label = label(&r.label)),
        Note::IncludesWebSearch => t!("usage.note.webSearch").to_owned(),
        Note::PausedBy { shared: true } => t!("usage.note.pausedByWorkspace").to_owned(),
        Note::PausedBy { shared: false } => t!("usage.note.pausedByBudget").to_owned(),
        Note::CurrentModel(name) => t!("usage.note.currentModel", model = name),
    }
}

/// The danger callout over the sections (`usage-callout`): a 2 pt bar beside the panel.
fn callout(p: &Pal, c: Callout, workspace: &str) -> El {
    let (title, body) = match c {
        Callout::Credits => (t!("usage.callout.credits.title", workspace = workspace), t!("usage.callout.credits.body").to_owned()),
        Callout::Workspace { resets_at } => (t!("usage.callout.workspace.title").to_owned(), t!("usage.callout.workspace.body", workspace = workspace, time = time(resets_at))),
        Callout::Personal { resets_at } => (t!("usage.callout.personal.title").to_owned(), t!("usage.callout.personal.body", workspace = workspace, time = time(resets_at))),
    };
    let ink = reached(p);
    row().key("usage-callout").w_full().px(16.0).gap(2.0)
        .child(el().w(2.0).self_stretch().fill(p.v2.danger.fg))
        .child(col().grow(1.0).shrink().min_w(0.0).gap(6.0).padding(12.0).corners([0.0, 6.0, 6.0, 0.0]).fill(p.v2.danger.bg)
            .child(line(&title, LINE_MED, ink))
            .child(text(body).font(BODY).color(ink).wrap().w_full()))
}

fn action(p: &Pal, a: &Action) -> El {
    let (title, body) = match a {
        Action::OtherModels { vendors } => (
            t!("usage.action.otherModels.title").to_owned(),
            if vendors.is_empty() { t!("usage.action.otherModels.body").to_owned() } else { t!("usage.action.otherModels.vendors", vendors = opencode_i18n::list(vendors)) },
        ),
        Action::OtherModelsEach { vendors, limit, suggestion } => {
            let limit = money(*limit, true);
            let title = if vendors.is_empty() { t!("usage.action.each.title").to_owned() } else { t!("usage.action.each.titleVendors", vendors = opencode_i18n::list(vendors)) };
            let body = match suggestion {
                Some((model, left)) => t!("usage.action.each.suggestion", limit = limit, model = model, left = money(*left, false)),
                None => t!("usage.action.each.body", limit = limit),
            };
            (title, body)
        }
    };
    row().key("usage-action").w_full().px(16.0).gap(16.0).center_y()
        .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0).child(line(&title, LINE_MED, p.text)).child(text(body).font(BODY).color(p.muted).wrap().w_full()))
        .child(widgets::button(t!("usage.action.switch"), ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.close_usage();
            s.open_picker(crate::pickers::Kind::Model);
        }).build(kit(p)).key("usage-switch"))
}

/// The footer: when the budgets reset, and View details (`platform.openExternal(usageURL)`).
fn footer(p: &Pal, v: &limits::View, url: &str) -> El {
    let words = match v.footer {
        Footer::CreditsOnly => t!("usage.footer.creditsOnly", workspace = v.workspace.clone().unwrap_or_else(|| t!("usage.workspace.fallback").to_owned())),
        Footer::BudgetsReset(at) => t!("usage.footer.budgetsReset", time = time(at)),
        Footer::Resets(at) => t!("usage.footer.resets", time = time(at)),
    };
    let url = url.to_owned();
    // A ghost `button-v2` at size normal in the muted ink (`class="text-v2-text-text-muted!"`).
    let details = row().key("usage-details").role("button").label_static(t!("usage.footer.details")).h(28.0).px(11.0).radius(6.0).center()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url(&url);
        })
        .child(text(t!("usage.footer.details")).font(widgets::BODY_MED).color(p.muted));
    row().key("usage-footer").w_full().padding(16.0).gap(8.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).h(28.0).center_y().child(line(&words, LINE, p.faint)))
        .child(details)
}

/// `Section`: its muted label (and the workspace's badge) over its panel, 12 apart.
fn section(p: &Pal, label: &str, badge: Option<&str>, dimmed: bool, body: El) -> El {
    let head = row().w_full().gap(8.0).pl(16.0).pr(20.0).center_y()
        .child(line(label, LINE, p.muted))
        .children(badge.map(|b| row().h(16.0).px(4.0).radius(2.0).center_y().fill(p.layer3).child(text(b.to_owned()).font(BADGE).color(p.muted))));
    let s = col().w_full().gap(12.0).child(head).child(col().w_full().px(12.0).child(body));
    if dimmed { s.opacity(0.6) } else { s }
}

/// `Panel`: 8 pt corners, a 0.5 `border-base` edge on `bg-base`, 16 above and below.
fn panel(p: &Pal, gap: f32) -> El {
    col().w_full().py(16.0).gap(gap).radius(8.0).fill(p.bg_base).border(0.5, p.border).border_layout()
}

fn item() -> El {
    col().w_full().gap(16.0).px(16.0)
}

fn divider(p: &Pal) -> El {
    el().w_full().h(0.5).fill(p.border)
}

fn line(words: &str, font: TextStyle, color: Color) -> El {
    text(words.to_owned()).font(font).color(color).truncate().shrink().min_w(0.0)
}

/// `Spend`: the status, a 1.125 pt dot, then the detail.
fn spend(p: &Pal, status: &str, detail: &str, reached_: bool) -> El {
    row().gap(6.0).center_y()
        .child(line(status, LINE_MED.features("\"tnum\""), if reached_ { reached(p) } else { p.text }))
        .child(el().size(3.0, 3.0).center().child(el().size(1.125, 1.125).radius(0.5625).fill(p.icon_muted)))
        .child(line(detail, LINE, p.muted))
}

/// `Track`: an 8 pt bar on `layer-03`, filled to `fill` in `bg-inverse`, or the reached red.
fn track(p: &Pal, fill: f64, reached_: bool) -> El {
    let f = fill.clamp(0.0, 1.0) as f32;
    row().w_full().h(8.0).radius(2.0).fill(p.layer3).clip_soft(0.0)
        .child(el().h(8.0).grow(f).fill(if reached_ { reached(p) } else { p.v2.bg_inverse }))
        .child(spacer().grow(1.0 - f))
}

/// `--usage-reached`: the design's `red-900` in light, the danger ink in dark (`dialog.css`).
fn reached(p: &Pal) -> Color {
    if p.dark { p.v2.danger.fg } else { session_view::palette::token(false, "red-900").unwrap_or(p.v2.danger.fg) }
}

/// `format.money`: US dollars in the locale's currency pattern (`$1,234.50`, `1.234,50 $`,
/// `1 234,50 $US`), cents always or (`auto`) only off a whole amount.
fn money(value: f64, auto: bool) -> String {
    opencode_i18n::usd(value, if auto && value.fract() == 0.0 { 0 } else { 2 })
}

/// `format.time`: `Nov 1 at 12:00 AM (GMT-7)`.
fn time(at: i64) -> String {
    let m = session_view::turn_errors::moment(rune::LocalTime::at(at));
    let (date, clock) = (opencode_i18n::date::format(opencode_i18n::date::Style::MonthDay, m), opencode_i18n::date::format(opencode_i18n::date::Style::Time, m));
    t!("usage.time", date = date, time = clock, zone = opencode_i18n::date::zone(m.offset))
}

/// `duration`: what's left until `until`, at least a minute.
fn duration(until: i64, now: i64) -> String {
    let minutes = ((until - now) / 60_000).max(1);
    if minutes >= 1_440 {
        return t!("usage.duration.days", days = minutes / 1_440, hours = minutes % 1_440 / 60);
    }
    if minutes >= 60 {
        return t!("usage.duration.hours", hours = minutes / 60, minutes = minutes % 60);
    }
    t!("usage.duration.minutes", minutes = minutes)
}
