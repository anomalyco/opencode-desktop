use super::*;
use std::rc::Rc;

use crate::inbox_model::{Entry, InboxStatus, age};
use crate::inbox_order::{Group, Order};
use crate::popup::{self, Item, Menu};
use opencode_i18n::t;

// ---------------------------------------------------------------------------------------------
// Inbox: Desktop's Inbox layout in the sidebar (`shell/titlebar/inbox.tsx`): Needs attention,
// Unread and Recent in a scrolling list, and Working pinned below it.

/// A card's title (`text-[13px] leading-4 font-medium tracking-[-0.04px]`).
const TITLE: TextStyle = TextStyle::new(13.0, 500.0).lh(16.0).tracking(-0.04);
/// The project line under it, and a group's header (`text-[13px] leading-4`).
const LINE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const HEADER: TextStyle = TextStyle::new(13.0, 500.0).lh(16.0);
/// The age (`text-[12px] tabular-nums`).
const AGE: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0).tracking(-0.04).features("\"tnum\"");
/// Show more and Archived: a ghost-muted Button (`button.css`), 13/20, 440.
const BUTTON: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04).features("\"tnum\"");
/// The chevron's turn (`transition-transform`: 150 ms, Tailwind's default ease).
const TURN: Curve = Curve::tween(0.15, Easing::cubic(0.4, 0.0, 0.2, 1.0));

/// The inbox in the sidebar's list slot. `inset` is the sidebar's start and end padding, which
/// the Working shelf's hairline bleeds across to the panel's edge.
pub(crate) fn inbox(s: &App, p: &Pal, inset: (f32, f32)) -> El {
    if s.sidebar_inbox.loading() {
        // Until the first load settles: Desktop's `Loader`, centred 8 in (`flex justify-center p-2`).
        return col().key("sidebar-inbox").role("list").label_static(t!("settings.general.row.interfaceMode.inbox")).w_full().grow(1.0).shrink().min_h(0.0).child(
            row().key("inbox-loading").w_full().padding(8.0).justify_center().child(rune_kit::Loader::new().build(crate::palette::kit(p))),
        );
    }
    let order = s.inbox_order();
    let expanded = s.kernel.slots.get(&crate::restore::RESTORE).inbox.recent_expanded;
    let items = Rc::new(items(&order, expanded));
    // Only the rows in sight are built (a virtual list), whatever Show more shows: each row is
    // its session, and its card reads the session as it stands. The rows' places stand as the
    // groups' gaps did: 2 between cards, 8 under a header, 10 under a group and 2 between groups.
    let n = items.len();
    // The last group's own 10 of padding under it, then the list's 4.
    let pb = if matches!(items.last(), Some(Line::Card { .. } | Line::More)) { 14.0 } else { 4.0 };
    let version = (order.generation, expanded);
    let (keys, built, pal) = (items.clone(), items.clone(), *p);
    let (key_order, built_order) = (order.clone(), order.clone());
    let list = col()
        .key("content")
        .w_full()
        .pb(pb)
        .growth(rune::GrowthSpec::default())
        .virtual_rows(
            n,
            // Most rows are a card and the 2 above it.
            CARD_H + 2.0,
            move |_: &App, i| keys[i].key(&key_order).into(),
            move |s: &App, i| {
                let item = &built[i];
                el().w_full().pt(space_above(&built, i)).child(item.build(s, &pal, &built_order))
            },
        )
        .virtual_rows_version(version);
    col()
        .key("sidebar-inbox")
        .role("list")
        .label_static(t!("settings.general.row.interfaceMode.inbox"))
        .focus_group()
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        // Home and End go to the inbox's first and last rows, built or not: the arrows' own Home
        // and End reach only the rows built. A row not built is scrolled to, which builds it.
        .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| {
            if ev.mods != rune::Mods::default() || !matches!(ev.key, rune::Key::Home | rune::Key::End) {
                return false;
            }
            let Some((row, key)) = end_key(s, ev.key == rune::Key::End) else { return false };
            // Focus goes on once the scroll has built its row: the next poll (`inbox_model::poll`).
            tx.scroll_into_view(row);
            s.sidebar_inbox.focus_next = Some(key);
            tx.wake_after(std::time::Duration::ZERO);
            true
        })
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            if (s.sidebar_inbox.height - r[3]).abs() > 0.5 {
                s.sidebar_inbox.height = r[3];
            } else {
                tx.live_only();
            }
        })
        .child(scroller(p, "inbox-scroll", list).grow(1.0))
        .children((!order.group(Group::Working).is_empty()).then(|| working(s, p, &order, inset)))
}

/// A card's height (`InboxSessionCard`, 54pt): the list's estimate for rows it hasn't laid out.
const CARD_H: f32 = 54.0;

/// One row of the inbox's list.
#[derive(Clone, Copy, Debug)]
enum Line {
    Header(Group),
    /// The card of `order.rows[at]`, `pos` of `size` in its group (A11Y: true for rows not built).
    Card { at: usize, pos: usize, size: usize },
    /// Show more / Show less under Recent.
    More,
    Empty,
    Archived,
}

/// The list's rows in order: the open session's card when no group shows it, then each group's
/// header and cards (and Show more under Recent), the empty line, and Archived.
fn items(order: &Order, expanded: bool) -> Vec<Line> {
    let mut out = Vec::new();
    let card = |out: &mut Vec<Line>, shows: &[usize]| {
        let size = shows.len();
        out.extend(shows.iter().enumerate().map(|(k, &at)| Line::Card { at, pos: k + 1, size }));
    };
    card(&mut out, &order.shows(Group::Current, expanded));
    for g in [Group::Needs, Group::Unread, Group::Recent] {
        let shows = order.shows(g, expanded);
        if shows.is_empty() {
            continue;
        }
        out.push(Line::Header(g));
        card(&mut out, &shows);
        if g == Group::Recent && order.more {
            out.push(Line::More);
        }
    }
    if order.is_empty() {
        out.push(Line::Empty);
    }
    if order.archived {
        out.push(Line::Archived);
    }
    out
}
/// The inbox's first or (`last`) last row a key can reach, in the list or, open, the Working
/// shelf below it: the row to scroll to (the list's own key for one of its rows), and the
/// element to focus.
fn end_key(s: &App, last: bool) -> Option<(String, String)> {
    let order = s.inbox_order();
    let items = items(&order, s.kernel.slots.get(&crate::restore::RESTORE).inbox.recent_expanded);
    let reach = |l: &Line| match l {
        Line::Card { at, .. } => Some(order.rows[*at].key.clone()),
        Line::More => Some("inbox-more".to_owned()),
        Line::Archived => Some("inbox-archived-button".to_owned()),
        Line::Header(_) | Line::Empty => None,
    }
    .map(|key| (l.key(&order), key));
    let shelf = if s.kernel.slots.get(&crate::restore::RESTORE).inbox.working_expanded { order.group(Group::Working) } else { Default::default() };
    let shelved = |r: &crate::inbox_order::Shown| (r.key.clone(), r.key.clone());
    if last {
        shelf.last().map(shelved).or_else(|| items.iter().rev().find_map(reach))
    } else {
        items.iter().find_map(reach).or_else(|| shelf.first().map(shelved))
    }
}
/// The room above row `i`, as the groups' gaps and padding laid it out.
fn space_above(items: &[Line], i: usize) -> f32 {
    let Some(prev) = i.checked_sub(1).map(|j| items[j]) else { return 0.0 };
    match (prev, items[i]) {
        // Under a group: its 10 of padding and the list's 2.
        (Line::Card { .. } | Line::More, Line::Header(_) | Line::Archived) => 12.0,
        (Line::Header(_), _) | (_, Line::More) => 8.0,
        _ => 2.0,
    }
}

impl Line {
    /// The list's key for the row: `line-` and its element's, so a key names one element.
    fn key(&self, order: &Order) -> String {
        format!("line-{}", self.element_key(order))
    }

    fn element_key(&self, order: &Order) -> String {
        match self {
            Line::Header(g) => format!("inbox-{}-header", group_id(*g)),
            Line::Card { at, .. } => order.rows[*at].key.clone(),
            Line::More => "inbox-more".into(),
            Line::Empty => "inbox-empty".into(),
            Line::Archived => "inbox-archived".into(),
        }
    }

    fn build(&self, s: &App, p: &Pal, order: &Order) -> El {
        match *self {
            Line::Header(g) => header(p, group_id(g), group_label(g)),
            Line::Card { at, pos, size } => match s.inbox_entry_of(&order.rows[at]) {
                Some(e) => card(s, p, &e).position_in_set(pos, size),
                // Gone since the rows were read: the next reading drops it.
                None => el().key(self.element_key(order)).h(CARD_H),
            },
            Line::More => {
                let label = if s.kernel.slots.get(&crate::restore::RESTORE).inbox.recent_expanded { t!("sidebar.inbox.showLess") } else { t!("sidebar.inbox.showMore") };
                ghost_button(p, "inbox-more", label).on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    let opening = !s.kernel.slots.get(&crate::restore::RESTORE).inbox.recent_expanded;
                    s.toggle_inbox_recent();
                    // Open, the button goes to the list's end, past the rows built: focus goes on
                    // to the first row it brought, which stands where the button stood.
                    let order = s.inbox_order();
                    if opening && let Some(&at) = order.shows(Group::Recent, true).get(crate::inbox_model::RECENT_LIMIT) {
                        tx.focus(order.rows[at].key.clone(), false);
                    }
                })
            }
            // `px-1.5 py-2`, a block whose words wrap at the sidebar's width (`inbox.tsx:421`).
            Line::Empty => col().key("inbox-empty").w_full().px(6.0).py(8.0).child(text(t!("sidebar.inbox.empty")).font(LINE).color(p.faint).wrap().w_full()),
            Line::Archived => el().key("inbox-archived").w_full().pt(12.0).child(
                ghost_button(p, "inbox-archived-button", t!("sidebar.inbox.section.archived")).on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.close_settings();
                    s.open_archived_search(tx);
                }),
            ),
        }
    }
}

fn group_id(g: Group) -> &'static str {
    match g {
        Group::Current => "current",
        Group::Needs => "needs",
        Group::Unread => "unread",
        Group::Recent => "recent",
        Group::Working => "working",
    }
}

fn group_label(g: Group) -> &'static str {
    match g {
        Group::Needs => t!("sidebar.inbox.section.needsAttention"),
        Group::Unread => t!("sidebar.inbox.section.unread"),
        Group::Working => t!("sidebar.inbox.section.working"),
        Group::Current | Group::Recent => t!("sidebar.inbox.section.recent"),
    }
}

/// Which of the inbox's two scroll areas.
/// A plain `overflow-y-auto` area, with `scrollbar.css`'s bar in the gutter it takes while it overflows.
fn scroller(p: &Pal, key: &str, content: El) -> El {
    session_view::scroller::scroller(key).build::<App>(p, col().w_full().shrink().min_h(0.0), content)
}

/// Working sessions, in a shelf pinned below the list: at most 40% of the inbox, behind a
/// full-bleed hairline.
fn working(s: &App, p: &Pal, order: &Order, inset: (f32, f32)) -> El {
    let open = s.kernel.slots.get(&crate::restore::RESTORE).inbox.working_expanded;
    let mut shelf = col()
        .key("inbox-working")
        .role("group")
        .label_static(t!("sidebar.inbox.section.working"))
        .w_full()
        .gap(8.0)
        .pt(7.0)
        // From 1pt into the sidebar to 1pt short of the panel, past the 8pt gutter (`--sidebar-divider-bleed`).
        .child(el().key("inbox-working-rule").absolute(1.0 - inset.0, 0.0).w(*s.sidebar_w - 2.0 + 8.0).h(1.0).fill(p.border_muted))
        .child(working_header(s, p, open));
    if s.sidebar_inbox.height > 0.0 {
        shelf = shelf.max_h(s.sidebar_inbox.height * 0.4);
    }
    if open {
        let rows = col().key("content").w_full().gap(2.0).children(order.group(Group::Working).iter().filter_map(|r| s.inbox_entry_of(r)).map(|e| card(s, p, &e)));
        shelf = shelf.child(scroller(p, "inbox-working-rows", rows));
    }
    shelf
}

/// A group's 24pt header: 13/16 medium, muted.
fn header(p: &Pal, id: &str, label: &str) -> El {
    row().key(format!("inbox-{id}-header")).h(24.0).w_full().px(8.0).gap(6.0).center_y().child(text(label).font(HEADER).color(p.muted).truncate())
}

/// The Working header: a toggle whose chevron turns shut, and whose label shimmers while shut.
fn working_header(s: &App, p: &Pal, open: bool) -> El {
    let label = if open {
        text(t!("sidebar.inbox.section.working")).key("label").font(HEADER).color(p.muted).group_color(p.text)
    } else {
        session_view::working::shimmer(s, p, t!("sidebar.inbox.section.working"), HEADER, 0.0)
    };
    row()
        .key("inbox-working-header")
        .role("button")
        .label_static(t!("sidebar.inbox.section.working"))
        .expanded(open)
        .h(24.0)
        .w_full()
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_inbox_working();
        })
        .child(label)
        .child(
            icon(&ic::FILL_TRIANGLE_DOWN, 14.0)
                .key("chevron")
                .color(p.muted)
                .group_color(p.text)
                .rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl()
                .rotate_motion(TURN),
        )
}

/// A ghost-muted, full-width Button with its label at the start (`w-full justify-start px-2`).
fn ghost_button(p: &Pal, key: &str, label: &str) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .w_full()
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label).font(BUTTON).color(p.muted))
}

/// `InboxSessionCard`: 54pt, two lines. The title, then the project, its worktree and branch
/// marks, and at the end the status and the age.
fn card(s: &App, p: &Pal, e: &Entry<'_>) -> El {
    let info = e.session;
    let selected = s.inbox_selected(e);
    let project = e.project;
    let name = project_or_dir(project, &info.directory);
    let title = crate::tabs::tab_title(&info.title);
    let mut place = row()
        .key("place")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .center_y()
        .clip_soft(0.0)
        .child(text(name.clone()).font(LINE).color(p.muted).shrink().min_w(0.0).truncate().dir(rune::Direction::Auto));
    if e.worktree {
        place = place.child(icon(&ic::OUTLINE_WORKTREE, 16.0).key("worktree").color(p.icon_muted).help(info.directory.clone()).help_side(rune::Side::Right));
    }
    if let Some(branch) = e.branch {
        place = place.child(icon(&ic::BRANCH, 16.0).key("branch").color(p.icon_muted).help(branch.to_owned()).help_side(rune::Side::Right));
    }
    // The session's server, where tabs of several servers mix (`servers.tag`): a 16 pt server
    // glyph at the line's start, under the avatar, naming it on hover (`inbox.tsx:617-623`).
    let server = crate::servers::runtime::tag(e.server).map(|name| {
        el().key("server").absolute(0.0, 0.0).size(16.0, 16.0).center().label(name.clone()).help(name).help_side(rune::Side::Right).child(icon(&SERVER, 16.0).color(p.icon_muted))
    });
    let mut end = row().key("end").gap(6.0).center_y();
    match e.status {
        InboxStatus::Ready => {}
        InboxStatus::Working => end = end.child(activity_ring(p)),
        status => {
            let fill = match status {
                InboxStatus::Approval | InboxStatus::Input => p.warning,
                InboxStatus::Failed => p.danger,
                _ => p.bg_accent,
            };
            end = end.child(el().key("dot").size(7.0, 7.0).radius(3.5).fill(fill));
        }
    }
    if let Some(label) = age(e.status, e.since, *s.now_ms) {
        end = end.child(text(label).key("age").font(AGE).color(p.faint));
    }
    let key = e.key();
    let (id, menu_id) = (key.clone(), key.clone());
    let archived = info.archived.is_some();
    let mut card = col()
        .key(format!("ib-{key}"))
        .role("button")
        .label(title.clone())
        .w_full()
        .h(54.0)
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .focusable(true)
        .focus_selected(selected)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_inbox_session(&id, tx))
        .on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open_menu(s, &menu_id, archived, press, tx))
        .child(
            row()
                .key("head")
                .w_full()
                .h(16.0)
                .gap(8.0)
                .center_y()
                .child(el().key("avatar").size(16.0, 16.0).center().child(crate::icons::badge(s, p, project, &name)))
                // Title and project in their own direction (`inbox.tsx:610`, `:625`).
                .child(text(title).font(TITLE).color(p.text).grow(1.0).shrink().min_w(0.0).truncate().dir(rune::Direction::Auto)),
        )
        .child(row().key("sub").w_full().h(16.0).pl(24.0).gap(8.0).center_y().children(server).child(place).child(end));
    // The routed session's tab becomes this card as the layout switches, Settings open over it or
    // not (`shell-active-{id}`): 28 tall in Side tabs, 54 here, on SETTLE; GLIDE from Top tabs.
    if matches!(&*s.route, Route::Session { id: r, server } if server == e.server && s.root_of(r) == info.id) {
        card = card.matched(format!("{}-{key}", super::tab_strip::ACTIVE)).matched_crop();
        if let Some(spring) = s.switch_spring() {
            card = card.motion(spring);
        }
    }
    if selected {
        card = card.fill(p.pressed);
    } else {
        card = card.hover(p.hover);
    }
    if archived && !selected {
        card = card.opacity(0.5);
    }
    card
}

/// A card's right-click menu: Archive (or Unarchive), then Delete…, each on the card's server.
fn open_menu(s: &mut App, key: &str, archived: bool, at: rune::PointerPress, tx: &mut Tx) {
    let (server, id) = s.inbox_target(key);
    let target = crate::session_menu::Target::on(&server, &id, crate::session_menu::Spot::Row);
    let read = crate::servers::runtime::with_server(s, &server, |s| crate::session_menu::lines(s, &target).into_iter().find_map(|line| match line {
        crate::session_menu::Line::Item(i) if matches!(i.id, "read" | "unread") => Some(Item::new(i.id, i.title)),
        _ => None,
    })).flatten();
    // `inbox.tsx:185-209`: Rename, Export…, the read mark, then Archive and Delete….
    let menu = Menu::new(t!("command.category.session"))
        .item(Item::new("rename", t!("common.rename")))
        .item(Item::new("export", format!("{}\u{2026}", t!("common.export"))).enabled(!s.exporting(&server, &id)))
        .items(read).separator()
        .item(if archived { Item::new("unarchive", t!("sidebar.inbox.unarchive")) } else { Item::new("archive", t!("common.archive")) })
        .separator()
        .item(Item::new("delete", format!("{}…", t!("common.delete"))));
    let key = key.to_owned();
    tx.instant();
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        match picked {
            Some("rename") => s.open_rename_dialog(&target),
            Some(action @ ("export" | "read" | "unread")) => crate::session_menu::run(s, &target, action, tx),
            Some("archive") => s.on_inbox_row(&key, |s, id| s.archive_session(id, tx)),
            Some("unarchive") => s.on_inbox_row(&key, |s, id| s.unarchive_session(id)),
            Some("delete") => {
                let (server, id) = s.inbox_target(&key);
                s.ask_delete_on(&server, &id);
            }
            _ => {}
        }
    });
}

/// Desktop's `server` glyph (`additional-icons.ts`), on a 20 pt grid drawn at 16.
static SERVER: Icon = Icon {
    name: "server",
    view: 20.0,
    body: r#"<rect x="3.35547" y="1.92969" width="13.2857" height="16.1429" stroke="currentColor"/><rect x="3.35547" y="11.9297" width="13.2857" height="6.14286" stroke="currentColor"/><rect x="12.8555" y="14.2852" width="1.42857" height="1.42857" fill="currentColor"/><rect x="10" y="14.2852" width="1.42857" height="1.42857" fill="currentColor"/>"#,
};
