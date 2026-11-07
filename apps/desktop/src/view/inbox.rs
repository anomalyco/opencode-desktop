use super::*;
use crate::inbox_model::{Entry, InboxStatus, Sections, age, in_worktree};
use crate::popup::{self, Item, Menu};
use rune_kit::ScrollBar;

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
        return col().key("sidebar-inbox").role("list").label("Inbox").w_full().grow(1.0).shrink().min_h(0.0).child(
            row().key("inbox-loading").w_full().padding(8.0).justify_center().child(rune_kit::Loader::new().build(crate::palette::kit(p))),
        );
    }
    let sections = s.inbox_sections();
    let mut list = col().key("content").w_full().gap(2.0).pb(4.0);
    if !sections.current.is_empty() {
        list = list.child(col().key("inbox-current").w_full().gap(2.0).pb(10.0).children(cards(s, p, &sections.current)));
    }
    for (id, label, rows) in [("needs", "Needs attention", &sections.needs), ("unread", "Unread", &sections.unread), ("recent", "Recent", &sections.recent)] {
        if rows.is_empty() {
            continue;
        }
        let mut group = col()
            .key(format!("inbox-{id}"))
            .role("group")
            .label(label)
            .w_full()
            .gap(8.0)
            .pb(10.0)
            .child(header(p, id, label))
            .child(col().key("rows").w_full().gap(2.0).children(cards(s, p, rows)));
        if id == "recent" && sections.more {
            let label = if s.restore.inbox.recent_expanded { "Show less" } else { "Show more" };
            group = group.child(ghost_button(p, "inbox-more", label).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.toggle_inbox_recent();
            }));
        }
        list = list.child(group);
    }
    if sections.is_empty() {
        list = list.child(
            // `px-1.5 py-2`, a block whose words wrap at the sidebar's width (`inbox.tsx:421`).
            col().key("inbox-empty").w_full().px(6.0).py(8.0).child(text("Sessions you work on will appear here").font(LINE).color(p.faint).wrap().w_full()),
        );
    }
    if sections.archived {
        list = list.child(
            el().key("inbox-archived").w_full().pt(12.0).child(
                ghost_button(p, "inbox-archived-button", "Archived").on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.close_settings();
                    s.open_archived_search(tx);
                }),
            ),
        );
    }
    col()
        .key("sidebar-inbox")
        .role("list")
        .label("Inbox")
        .focus_group()
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            if (s.sidebar_inbox.height - r[3]).abs() > 0.5 {
                s.sidebar_inbox.height = r[3];
            } else {
                tx.live_only();
            }
        })
        .child(scroller(s, p, "inbox-scroll", Pane::List, list).grow(1.0))
        .children((!sections.working.is_empty()).then(|| working(s, p, &sections, inset)))
}

/// Which of the inbox's two scroll areas.
#[derive(Clone, Copy)]
enum Pane {
    List,
    Shelf,
}

/// A plain `overflow-y-auto` area: while its content overflows, Chromium's styled scroll bar
/// (`scrollbar.css`) takes an 8pt gutter from its width. Both heights are measured, so the
/// gutter comes and goes with the overflow.
fn scroller(s: &App, p: &Pal, key: &str, pane: Pane, content: El) -> El {
    let fit = match pane {
        Pane::List => s.sidebar_inbox.list,
        Pane::Shelf => s.sidebar_inbox.shelf,
    };
    let measure = move |s: &mut App, h: f32, tx: &mut Tx, content: bool| {
        let fit = match pane {
            Pane::List => &mut s.sidebar_inbox.list,
            Pane::Shelf => &mut s.sidebar_inbox.shelf,
        };
        let slot = if content { &mut fit.content } else { &mut fit.view };
        if (*slot - h).abs() > 0.5 {
            *slot = h;
        } else {
            tx.live_only();
        }
    };
    col()
        .key(key)
        .w_full()
        .shrink()
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| measure(s, r[3], tx, false))
        .child(
            content
                .pr(if fit.overflows() { SCROLLBAR_GUTTER } else { 0.0 })
                .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| measure(s, r[3], tx, true)),
        )
        // `scrollbar.css`: the plain `overflow-y: auto` box's own bar, in the gutter it takes.
        .scroll_bar_styled(rune_kit::ScrollBarStyle::native(p.border_muted, p.q.border_strong).inside())
}

/// The width a styled Chromium scroll bar takes from layout (`--scrollbar-size`).
const SCROLLBAR_GUTTER: f32 = 8.0;

/// Working sessions, in a shelf pinned below the list: at most 40% of the inbox, behind a
/// full-bleed hairline.
fn working(s: &App, p: &Pal, sections: &Sections<'_>, inset: (f32, f32)) -> El {
    let open = s.restore.inbox.working_expanded;
    let mut shelf = col()
        .key("inbox-working")
        .role("group")
        .label("Working")
        .w_full()
        .gap(8.0)
        .pt(7.0)
        // From 1pt into the sidebar to 1pt short of the panel, past the 8pt gutter (`--sidebar-divider-bleed`).
        .child(el().key("inbox-working-rule").absolute(1.0 - inset.0, 0.0).w(s.sidebar_w - 2.0 + 8.0).h(1.0).fill(p.border_muted))
        .child(working_header(s, p, open));
    if s.sidebar_inbox.height > 0.0 {
        shelf = shelf.max_h(s.sidebar_inbox.height * 0.4);
    }
    if open {
        let rows = col().key("content").w_full().gap(2.0).children(cards(s, p, &sections.working));
        shelf = shelf.child(scroller(s, p, "inbox-working-rows", Pane::Shelf, rows));
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
        text("Working").key("label").font(HEADER).color(p.muted).group_color(p.text)
    } else {
        session_view::working::shimmer(s, p, "Working", HEADER, 0.0)
    };
    row()
        .key("inbox-working-header")
        .role("button")
        .label("Working")
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
                .rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })
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

fn cards(s: &App, p: &Pal, rows: &[Entry<'_>]) -> Vec<El> {
    rows.iter().map(|e| card(s, p, e)).collect()
}

/// `InboxSessionCard`: 54pt, two lines. The title, then the project, its worktree and branch
/// marks, and at the end the status and the age.
fn card(s: &App, p: &Pal, e: &Entry<'_>) -> El {
    let info = e.session;
    let selected = s.inbox_selected(e);
    let project = s.project_for_session(info).or_else(|| s.project_at(&info.directory));
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
        .child(text(name.clone()).font(LINE).color(p.muted).shrink().min_w(0.0).truncate());
    if in_worktree(s, info) {
        place = place.child(icon(&ic::OUTLINE_WORKTREE, 16.0).key("worktree").color(p.icon_muted));
    }
    if s.sidebar_inbox.branches.get(&info.directory).is_some() {
        place = place.child(icon(&ic::BRANCH, 16.0).key("branch").color(p.icon_muted));
    }
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
    if let Some(label) = age(e.status, e.since, s.now_ms) {
        end = end.child(text(label).key("age").font(AGE).color(p.faint));
    }
    let id = info.id.clone();
    let menu_id = info.id.clone();
    let archived = info.archived.is_some();
    let mut card = col()
        .key(format!("ib-{}", info.id))
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
        .on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open_menu(s, &menu_id, press, tx))
        .child(
            row()
                .key("head")
                .w_full()
                .h(16.0)
                .gap(8.0)
                .center_y()
                .child(el().key("avatar").size(16.0, 16.0).center().child(crate::icons::badge(s, p, project, &name)))
                .child(text(title).font(TITLE).color(p.text).grow(1.0).shrink().min_w(0.0).truncate()),
        )
        .child(row().key("sub").w_full().h(16.0).pl(24.0).gap(8.0).center_y().child(place).child(end));
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

/// A card's right-click menu: Archive (or Unarchive), then Delete….
fn open_menu(s: &mut App, id: &str, at: rune::PointerPress, tx: &mut Tx) {
    let archived = s.session(id).is_some_and(|i| i.archived.is_some());
    let menu = Menu::new("Session")
        .item(if archived { Item::new("unarchive", "Unarchive") } else { Item::new("archive", "Archive") })
        .separator()
        .item(Item::new("delete", "Delete…"));
    let id = id.to_owned();
    tx.instant();
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        match picked {
            Some("archive") => s.archive_session(&id, tx),
            Some("unarchive") => s.unarchive_session(&id),
            Some("delete") => s.menus.confirm = Some(id.clone()),
            _ => {}
        }
    });
}
