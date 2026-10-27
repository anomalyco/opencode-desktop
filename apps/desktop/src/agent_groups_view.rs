//! The grouped Subagent rows styles: agents started together drawn as one group, in boxes, a
//! stack, one card, a summary that opens, or chips (`agent_rows_view.rs` has the shared parts).

use rune::{El, Tx, col, icon, row, spacer, text};
use rune_kit::Kit;

use rune_theme_opencode::icons as ic;

use crate::App;
use crate::agent_rows::{Run, State};
use crate::agent_rows_view::{Call, arrive, count, live_row, mark, opens, rolling_slot, tally, time};
use crate::palette::Pal;
use crate::view::{UI, UI_SMALL};

/// One group of calls, ready to draw.
pub struct Group<'a> {
    pub p: &'a Pal,
    pub parent: &'a str,
    pub calls: &'a [Call<'a>],
    pub kit: Kit,
}

impl Group<'_> {
    fn key(&self) -> &str {
        &self.calls[0].key
    }

    fn runs(&self) -> Vec<&Run> {
        self.calls.iter().map(|c| &c.run).collect()
    }

    /// Running while any runs; then failed if any failed.
    fn overall(&self) -> State {
        let any = |st: State| self.calls.iter().any(|c| c.run.state == st);
        if any(State::Running) { State::Running } else if any(State::Failed) { State::Failed } else { State::Done }
    }

    /// The longest run's time.
    fn longest(&self) -> Option<String> {
        self.calls.iter().filter(|c| c.run.time.is_some()).max_by_key(|c| c.run.ms).and_then(|c| c.run.time.clone())
    }

    /// The step that changed last among the running agents, keyed so each new one rolls in.
    fn latest(&self) -> El {
        let p = self.p;
        match self.calls.iter().filter(|c| c.run.state == State::Running).max_by_key(|c| c.run.moved) {
            Some(c) => {
                let key = format!("latest-{}-{}", c.key, c.run.taken + c.run.steps.len());
                rolling_slot("now", 28.0, text(c.run.line()).key(key).font(UI.lh(20.0)).color(p.faint).truncate_fade())
            }
            None => rolling_slot("now", 28.0, tally(p, &self.kit, self.key(), &self.runs(), false)),
        }
    }

    /// One agent's row in `surface`, rising into place when it joins.
    fn member(&self, c: &Call, h: f32, on: rune::Color, surface: impl FnOnce(El) -> El) -> El {
        let line = surface(live_row(self.p, c, h, on).key("agent-line"));
        row().key(format!("member-{}", c.key)).w_full().child(opens(self.parent, c, line)).transition(arrive())
    }

    /// Group of boxes: a quiet "3 agents · 2 running · 1 done" line, then a box per agent.
    pub fn boxes(&self) -> El {
        let p = self.p;
        let head = row()
            .key("group-head")
            .w_full()
            .animate_layout()
            .h(24.0)
            .px(4.0)
            .gap(4.0)
            .center_y()
            .child(count(p, &self.kit, self.key(), self.calls.len()))
            .child(tally(p, &self.kit, self.key(), &self.runs(), true))
            .child(spacer())
            .children(time(p, self.longest()).map(|t| t.pr(8.0)));
        let mut list = col().key("agent-boxes").w_full().gap(4.0).child(head);
        for c in self.calls {
            list = list.child(self.member(c, 34.0, p.layer2, |e| e.px(12.0).radius(8.0).fill(p.layer2)));
        }
        row().w_full().child(list)
    }

    /// Stacked: a box per agent, 2pt apart, round on the outside and tight where they meet.
    pub fn stacked(&self) -> El {
        let p = self.p;
        let n = self.calls.len();
        let mut list = col().key("agent-stack").w_full().gap(2.0);
        for (j, c) in self.calls.iter().enumerate() {
            let (top, end) = (if j == 0 { 10.0 } else { 4.0 }, if j + 1 == n { 10.0 } else { 4.0 });
            list = list.child(self.member(c, 34.0, p.layer2, |e| e.px(12.0).corners([top, top, end, end]).fill(p.layer2)));
        }
        row().w_full().child(list)
    }

    /// One card, a live row per agent: no header, no dividers, just spacing.
    pub fn card(&self) -> El {
        let p = self.p;
        let mut card = col().key("agent-card").w_full().px(4.0).py(4.0).radius(10.0).fill(p.layer2);
        for c in self.calls {
            card = card.child(self.member(c, 32.0, p.layer2, |e| e.px(8.0).radius(6.0).hover(p.hover)));
        }
        row().w_full().child(card)
    }

    /// Summary: "3 agents · Read live.ts" with the latest step rolling in, settling to
    /// "3 agents · 2 done · 1 failed"; a click opens a row per agent, instantly.
    pub fn summary(&self, open: bool) -> El {
        let p = self.p;
        let gk = self.key().to_owned();
        let head = row()
            .key("summary")
            .w_full()
            .animate_layout()
            .h(36.0)
            .px(12.0)
            .gap(6.0)
            .center_y()
            .radius(8.0)
            .role("button")
            .label(if open { "Hide agents" } else { "Show agents" })
            .hover_group()
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if !s.agent_rows.open.remove(&gk) {
                    s.agent_rows.open.insert(gk.clone());
                }
            })
            .child(mark(p, self.overall(), p.layer2).mr(2.0))
            .child(count(p, &self.kit, self.key(), self.calls.len()))
            .child(text("·").key("dot").font(UI.lh(20.0)).color(p.faint))
            .child(self.latest())
            .children(time(p, self.longest()))
            .child(icon(if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }, 16.0).key("disclosure").color(p.icon_faint));
        let mut card = col().key("agent-summary").w_full().radius(8.0).fill(p.layer2).child(head);
        if open {
            let mut rows = col().key("agents").w_full().px(4.0).pb(4.0);
            for c in self.calls {
                let line = live_row(p, c, 30.0, p.layer2).key("agent-line").px(8.0).radius(6.0).hover(p.hover);
                rows = rows.child(row().key(format!("member-{}", c.key)).w_full().child(opens(self.parent, c, line)));
            }
            card = card.child(rows);
        }
        row().w_full().child(card)
    }

    /// Chips: one line with the latest step from any agent, over a chip per agent.
    pub fn chips(&self) -> El {
        let p = self.p;
        let head = row()
            .key("ticker")
            .w_full()
            .animate_layout()
            .h(28.0)
            .gap(6.0)
            .center_y()
            .child(mark(p, self.overall(), p.layer2).mr(2.0))
            .child(count(p, &self.kit, self.key(), self.calls.len()))
            .child(text("·").key("dot").font(UI.lh(20.0)).color(p.faint))
            .child(self.latest())
            .children(time(p, self.longest()));
        let mut chips = row().key("chips").w_full().flex_wrap().gap(6.0);
        for c in self.calls {
            let r = &c.run;
            let chip = row()
                .key("chip")
                .h(26.0)
                .pl(6.0)
                .pr(10.0)
                .gap(6.0)
                .center_y()
                .radius(13.0)
                .fill(p.bg_base)
                .border(0.5, p.border_muted)
                .hover(p.hover)
                .child(mark(p, r.state, p.bg_base))
                .child(text(c.title.to_owned()).key("title").font(UI_SMALL.lh(16.0)).color(p.text).truncate_fade().max_w(200.0))
                .children(time(p, r.time.clone()));
            chips = chips.child(row().key(format!("member-{}", c.key)).animate_layout().child(opens(self.parent, c, chip)).transition(arrive()));
        }
        let card = col().key("agent-chips").w_full().px(12.0).pt(4.0).pb(10.0).gap(4.0).radius(10.0).fill(p.layer2).child(head).child(chips);
        row().w_full().child(card)
    }
}
