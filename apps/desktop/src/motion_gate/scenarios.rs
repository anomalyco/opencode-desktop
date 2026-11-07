//! The matrix: each action, the circumstances it runs in, and what's allowed with a reason.

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use serde_json::json;

use rune::motion_gate::{self, Allow, Job};
use super::{Circ, DT, Dock, Length, Prompt, Run, SID};
use crate::App;

/// Findings that are the design, matched on kind, element and cell (substrings, "" for any).
/// Every row needs its reason.
const ALLOW: &[Allow] = &[
    // (kind, element contains, cell contains, why)
    ("jerk", "diagram-lightbox", "diagram-full-size/", "an open diagram flies out of its card and back on the image viewer's 0.25 s spring (IMAGE_FLIGHT), from rest over several hundred points, so its first frames speed up by up to ~20 pt a frame (diagram::lightbox)"),
    ("jerk", "fit/", "diagram-full-size/", "the open diagram's lines and labels ride its 0.25 s flight and ⌘0's spring back to the fit (diagram::lightbox)"),
    ("lands-unseen", "fit/", "diagram-full-size/", "two fingers pan the open diagram, a frame at a time, until parts of it are outside the window, where a pan back brings them (diagram::lightbox)"),
    ("fade-snap", "t-msg_zzwide1-0/#0/#0/b1/#0", "diagram-full-size/", "the reply's card shows again in the frame the open diagram, landed exactly over it, unmounts: one picture handed over in place (diagram::poll)"),
    ("pop-out", "window/diagram-stage", "diagram-full-size/", "the open diagram unmounts once it has landed exactly over its card, which shows again in that frame (diagram::poll)"),
    ("snap", "(pixels)", "diagram-full-size/", "once a pinch rests, the scaled diagram's text draws crisp again at its new size, in place (El::scale)"),
    ("stop-start", "", "steer-live/", "a steer lands as separate events a few frames apart (its echo, the running reply's tool row, the next step and its tool), each a row arriving that the timeline glides to in turn; no text is streaming then (p191)"),
    ("stop-start", "", "demo/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "fold/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "queue/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "queue-lands/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "rewind/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "select/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "send/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "send-live/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "side-region/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "steer/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "steer-lands/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "steers-land/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "sends-behind-steers/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("stop-start", "", "stream/", "a streaming reply opens each line it wraps onto on its own ~80 ms spring (Timeline › Line grow, p252), so the pinned timeline and Working step down a line at a time and rest between lines rather than creeping at the stream's pace"),
    ("jerk", "timeline/", "compaction-open-at-bottom/", "the Compacted summary folds on the tool group's ~180 ms spring (p162, t1012), so its speed changes by up to ~20 pt a frame and the rows under it ride it; shutting, the part below the timeline goes at once, out of sight, and from the stuck divider near the end the timeline's end holds as the divider settles onto the rows above (El::fold_glide)"),
    ("jerk", "timeline/", "tool-group-fold/", "Kit asked for a fold of about 180 ms (t1012): what shows of a 40-tool group folds that fast, so its speed changes by up to ~20 pt a frame and the rows under it ride it; shutting, the part below the timeline goes at once, out of sight (El::fold_glide)"),
    ("fade-snap", "turn-actions/#0/#0/working", "", "Working and Copy share a footprint and swap at once: a crossfade lays the copy icon's outline over the W (view::turn_actions)"),
    ("fade-snap", "turn-actions/#0/#0/copy", "", "Working and Copy share a footprint and swap at once: a crossfade lays the copy icon's outline over the W (view::turn_actions)"),
    ("jump", "turn-actions/#0/#0/working", "working-relabel", "new words take the Working label's place at once, as a new status does; the cell checks the shimmer's rhythm through the change (working::finish_pass)"),
    ("text-swap", "working/#0/label", "", "new words take the Working label's place at once, as a new status does (working::finish_pass)"),
    ("text-swap", "sheen/lit/", "working-relabel", "the shimmer's lit copy of the Working label takes the label's new words at once, as the label does (working::finish_pass)"),
    ("jump", "sheen/lit/", "working-relabel", "the shimmer's lit copy of the Working label widens with the label's new words at once, as the label does (working::finish_pass)"),
    ("load-motion", "scroll/rows", "tool-steps/", "Changes opening at a step's change measures the rows above it over the first two frames, so the list grows upward out of sight; every row on screen holds still (code_view::DiffViewer)"),
    ("text-swap", "files-crumb/", "tool-steps/", "a tool step opens its file's tab at once, and Esc puts the tab before back at once; the header names it as the page under it changes (tool_open)"),
    ("pop-out", "window/activity-panel", "capsule-open-long", "the capsule's list opens and closes at once, as Desktop's menu does (subagents::panel)"),
    ("pop-in", "window/activity-dismiss", "capsule-open-long", "an invisible layer under the open list that closes it on a click outside (view::app)"),
    ("pop-out", "window/activity-dismiss", "capsule-open-long", "an invisible layer under the open list that closes it on a click outside (view::app)"),
    ("pop-in", "window/todo-", "todo-pill", "the plan's list and the layer that closes it on a click outside open at once, as the capsule's list does (todos::panel)"),
    ("pop-in", "todo-glyph/disc", "todo-pill", "a finishing todo's fill mounts drawing exactly the half its glyph already showed, then sweeps round (todos::disc)"),
    ("pop-out", "window/todo-", "todo-pill", "the plan's list and the layer that closes it on a click outside close at once, as the capsule's list does (todos::panel)"),
    ("text-swap", "todo-pill/todo-now", "", "the pill names the next todo at once, as Desktop's pill does (todos::pill)"),
    ("text-swap", "-time-ses_", "", "a run time changes at once each second, as Desktop's do; a digit rolling every second smears it (subagents::run_time)"),
    ("jump", "working/#0/label", "working-relabel", "new words take the Working label's place at once, as a new status does; the cell checks the shimmer's rhythm through the change (working::finish_pass)"),
    ("pop-in", "queue-list/queued-", "queue/", "a newly queued row takes over from the composer's words in the frame they clear (Tx::launch from \"prompt\") and rises as the queue's height springs (queue::panel); the gate doesn't pair the words that leave with the row that takes them"),
    ("pop-in", "queue-list/queued-", "queue-lands/", "a newly queued row takes over from the composer's words in the frame they clear (Tx::launch from \"prompt\"), as in the queue cells"),
    ("pop-out", "queue-list/queued-", "queue-lands/", "a delivered row hands its words to its bubble in the frame it clears (Tx::launch from queued-…/label, Arrivals::launch); the gate doesn't pair the row that leaves with the bubble that takes its words"),
    ("pop-in", "timeline/t-msg_0older", "older-history/", "at the very top, a page that lands above shows its last row in the gap over the first, as Desktop's does; nothing on screen moves (session_pages::load_older)"),
    ("pop-in", "timeline/u-", "scrolled-up", "read further up, a send flies nowhere: the timeline cuts to its end and the message is there (App::land)"),
    ("pop-in", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
    ("pop-out", "quote-toolbar", "", "the Quote button appears over a settled selection at once and goes at once, as Desktop's does (quote::toolbar)"),
    ("pop-in", "header-actions/find-bar", "", "the Find bar opens at once on ⌘F, as Desktop's utility bars do (find::bar)"),
    ("pop-out", "header-actions/find-bar", "", "the Find bar closes at once on Esc or close (find::bar)"),
    ("jump", "header/crumbs", "find/", "crumbs make room for the Find bar in the frame ⌘F opens it (view::header)"),
    ("jump", "crumbs/", "find/", "the header title shortens at once when the Find bar takes its header slot (view::header)"),
    ("jump", "header/header-actions", "find/", "the header's actions make room for the Find bar in the frame ⌘F opens it (view::header)"),
    ("pop-in", "find-bar/find-count", "", "the match count appears at once with the first typed character (find::bar)"),
    ("jump", "find-bar/", "find/", "the Find field and count resize at once as typed characters change the match count (find::bar)"),
    ("text-swap", "find-bar/find-count", "", "the match count updates at once on each keystroke or step (find::bar)"),
    ("pop-in", "#mark-", "find/", "text match highlights paint and clear at once, as CSS Highlights do (find)"),
    ("pop-out", "#mark-", "find/", "text match highlights paint and clear at once, as CSS Highlights do (find)"),
    ("jump", "#mark-", "find/", "each typed letter widens or narrows the highlighted match boxes in that keystroke's frame (find)"),
    ("text-swap", "#mark-", "find/", "each typed letter extends the highlighted substring in that keystroke's frame (find)"),
    ("pop-in", "composer/attachments", "dropzone/", "a dropped file's chip lands at once and the composer grows to hold it, as a picked file's does; the overlay's fade is what this cell checks (dropzone::drop)"),
    ("jump", "dock/composer", "dropzone/", "the composer grows at once to hold a dropped file's chip, as for a picked file (dropzone::drop)"),
    ("jump", "session/dock", "dropzone/", "the composer grows at once to hold a dropped file's chip, as for a picked file (dropzone::drop)"),
    ("jump", "session/timeline-area", "dropzone/", "the timeline gives the composer the room for a dropped file's chip at once (dropzone::drop)"),
    ("fade-snap", "bubble/menu", "", "a prompt's corner chevron shows and hides with its bubble's hover at once, as Desktop's does (timeline::user_bubble)"),
    ("fade-snap", "bubble/meta", "", "a prompt's meta line shows and hides with its bubble's hover at once, together with the chevron, as Desktop's copy wrapper does (message-part.css:279-283; timeline::user_bubble)"),
    ("fade-snap", "turn-actions/#0/#0/meta", "", "a reply footer's model and time show and hide with the reply's hover at once, as Desktop's text-part-copy-wrapper does: 'Hover actions show and hide at once, without a fade' (message-part.css:584-588; timeline::turn_actions)"),
    ("pop-out", "window/popup-", "rewind/", "a prompt's menu and its click-outside layer close in the frame Esc or a pick closes them, as every popover closes by default (popover::motion CLOSE, t1150) and menu-v2 closes with no exit (menu.css has no transition); the rows a pick reverts still leave on their exits (rewind::toggle_message_menu)"),
    ("jump", "agent-line", "agents|agent-steered|agent-wakes|demo/", "a subagent chip drops its spinner in the frame its run ends, so its name and task close up 24 pt, and a Delegating agent chip widens into the agent's at once, as Desktop's <Show when={view().running}> and <Show when={view().delegating}> swap them (tool-renderer.tsx:1801-1803, 1821-1844; subagent_card::row_for)"),
    ("jump", "words/title", "agents|agent-steered|demo/", "the chip's name closes up the spinner's 24 pt in the frame its run ends, as Desktop's <Show when={view().running}> does (tool-renderer.tsx:1801-1803; subagent_card::row_for)"),
    ("jump", "words/task", "agents|agent-steered|demo/", "the chip's task closes up the spinner's 24 pt in the frame its run ends, as Desktop's <Show when={view().running}> does (tool-renderer.tsx:1801-1803; subagent_card::row_for)"),
    ("pop-in", "agent-line/words", "agent-wakes|demo/", "a Delegating agent chip turns into the agent's chip in place at once, as Desktop's <Show when={view().delegating}> swaps them (tool-renderer.tsx:1821-1844; subagent_card::row_for)"),
    ("snap", "terminal-pane (pixels)", "terminal-dock/", "the focused terminal's bar cursor blinks on and off, as Desktop's terminal does (cursorBlink: true while focused, gui-plugins/src/terminal/terminal.tsx:120-127, 437; terminal::core)"),
    ("load-motion", "ring/d", "", "the Soft Radar spinner's dots keep sweeping from the frame it shows, as Desktop's spinner-radar.ts does (style::activity_ring); one that shows for less than its 1.8 s turn never wraps, so the gate doesn't tell its loop from a fade"),
    ("jump", "header-location/location-trigger", "find/", "a narrow header gives up its project's name in the frame ⌘F opens the Find bar, so the title keeps room, as Desktop's header collapses (view::header_fit)"),
    ("pop-out", "location-trigger/location-project", "find/", "the project's name leaves the location button in that frame (view::header_fit)"),
    ("text-swap", "title/label", "demo/", "the demo opens its fresh session, and the server's rename lands, at once in the header (demo::start, session.renamed)"),
    ("jerk", "window/panel", "side-region/|review/|files/|tool-steps/", "the side region opens on Kit's 0.3 s SIDE_SPRING from rest across the chat's width, so its first frames speed up by up to ~30 pt a frame and the chat panel's edge rides it (panels::view)"),
    ("jerk", "window/side-region", "side-region/|review/|files/|tool-steps/", "the side region opens on Kit's 0.3 s SIDE_SPRING from rest across the chat's width, so its first frames speed up by up to ~30 pt a frame (panels::view)"),
    ("load-motion", "pane-side/", "tool-steps/", "a step's file opens the side region on Kit's SIDE_SPRING, as the toggle does (Desktop's layout.open flies the panel): its tab and pane load in the frame the flight sets off and ride it in (panels::view)"),
    ("load-motion", "side-tabs-side/", "tool-steps/", "a step's file opens the side region on Kit's SIDE_SPRING, as the toggle does: the new file tab loads in the frame the flight sets off and rides it in (panels::view)"),
    ("load-motion", "icon/icon-mono", "tool-steps/", "the new file tab's icon rides the side region's SIDE_SPRING flight in, as its tab does (panels::view)"),
    ("pop-in", "window/titlebar-band", "layout-switch/", "switching to top tabs, the titlebar row mounts at once under the panel, which covers it; what shows of it flies there from the sidebar (view::shell)"),
    ("pop-out", "actions/", "layout-switch/", "switching to top tabs, Home, Search sessions and New session hand over to the titlebar's buttons, which fly from their rows in that frame (El::matched); the gate doesn't pair the row that leaves with the flight that takes its place (view::sidebar)"),
    ("pop-out", "tabs-list/t-", "layout-switch/", "switching to top tabs, each side tab hands over to its top tab, which flies from its row in that frame (El::matched); the gate doesn't pair the row that leaves with the flight that takes its place"),
    ("jerk", "tab-strip", "layout-switch/", "switching to top tabs, the far tabs and the New tab button fly up to ~1,000 pt on the 0.32 s GLIDE from rest, so their first frames speed up by up to ~15 pt a frame (view::GLIDE)"),
    ("jerk", "tabs-list/t-", "layout-switch/", "switching to side tabs, the far tabs fly up to ~1,000 pt on the 0.32 s GLIDE from rest, as they do back to the top (view::GLIDE)"),
    ("jerk", "actions/new", "layout-switch/", "switching to side tabs, New session flies from the far end of the top tabs on the 0.32 s GLIDE from rest, as the tabs do (view::GLIDE)"),
    ("pop-in", "status-toast", "", "toasts appear and dismiss at once, as Desktop's instant utility UI does (status_toast::view)"),
    ("pop-out", "status-toast", "", "toasts appear and dismiss at once, as Desktop's instant utility UI does (status_toast::view)"),
    ("jerk", "editor-pad/prompt", "long-prompt", "a 26-line paste grows the editor to its 180 pt cap from rest, and the send drops it back: a spring's first frames over a long way (view::composer). The fold and the flight are what these cells check"),
    ("jump", "option-", "model-efforts", "the lit row shows its effort and star at once, so its words give them the room in that frame, as Desktop's row does (session_view::pickers::view::picker)"),
    ("pop-in", "option-", "model-efforts", "the lit row's effort and star show at once with its highlight, as Desktop's row does (session_view::pickers::view::picker)"),
    ("pop-out", "option-", "model-efforts", "the row the pointer leaves drops its effort and star at once with its highlight (session_view::pickers::view::picker)"),
    ("pop-in", "window/effort-flyout", "model-efforts", "a model's efforts open beside its row at once, as Desktop's flyout does (model_picker::flyout, p169)"),
    ("pop-out", "window/effort-flyout", "model-efforts", "the efforts close at once, as Desktop's flyout does (model_picker::hover)"),
    ("jump", "window/effort-flyout", "model-efforts", "once one flyout has opened, the next row's efforts take its place at once, as Desktop's do (model_picker::hover)"),
    ("pop-out", "window/picker-model", "model-efforts", "Esc closes the list in its frame, as every popover closes by default (popover::motion CLOSE, t1150)"),
    ("jump", "timeline-fade", "dropzone/", "the timeline gives the composer the room for a dropped file's chip at once (dropzone::drop)"),
    ("jump", "-scroll-area/scroll-", "dropzone/", "the timeline gives the composer the room for a dropped file's chip at once (dropzone::drop)"),
    ("jump", "th/c", "demo/", "a streaming table sizes its columns to each finished row, so they widen at once as a row lands (rune_kit markdown tables)"),
    ("jump", "r0/c", "demo/", "a streaming table sizes its columns to each finished row, so they widen at once as a row lands (rune_kit markdown tables)"),
    ("jump", "r1/c", "demo/", "a streaming table sizes its columns to each finished row, so they widen at once as a row lands (rune_kit markdown tables)"),
];

struct Action {
    name: &'static str,
    circs: fn() -> Vec<Circ>,
    run: fn(&mut Run),
}

/// Pinned and scrolled up on the long session, short and empty sessions, each wide and narrow.
fn places(base: Circ) -> Vec<Circ> {
    let mut v = Vec::new();
    for narrow in [false, true] {
        for scrolled in [false, true] {
            v.push(Circ { length: Length::Long, scrolled, narrow, ..base });
        }
        for length in [Length::Short, Length::Empty] {
            v.push(Circ { length, narrow, ..base });
        }
    }
    v
}

/// `places`, plus each prompt shape and the docks that can be open.
fn sends(base: Circ, docks: &[Dock]) -> Vec<Circ> {
    let mut v = places(base);
    for prompt in [Prompt::Lines, Prompt::Image, Prompt::Long] {
        for length in [Length::Long, Length::Empty] {
            v.push(Circ { prompt, length, ..base });
        }
    }
    for &dock in docks {
        for scrolled in [false, true] {
            v.push(Circ { dock, scrolled, ..base });
        }
    }
    v
}

fn streaming() -> Circ {
    Circ { unplayed: true, ..Circ::default() }
}

/// Pinned to the end of the long session, wide and narrow.
fn pinned() -> Vec<Circ> {
    vec![Circ::default(), Circ { narrow: true, ..Circ::default() }]
}

fn wide_and_narrow(base: Circ) -> Vec<Circ> {
    let mut v = Vec::new();
    for narrow in [false, true] {
        for scrolled in [false, true] {
            v.push(Circ { narrow, scrolled, ..base });
        }
    }
    v
}

/// The scroll bar on the long session: wide, narrow, and zoomed.
fn scroll_bar_circs() -> Vec<Circ> {
    vec![Circ::default(), Circ { narrow: true, ..Circ::default() }, Circ { zoomed: true, ..Circ::default() }]
}

const ACTIONS: &[Action] = &[
    Action { name: "send-receipts", circs: pinned, run: |r| {
        r.rt.update(|s, tx| { tx.instant(); crate::send_failed::stories::setup(s); });
        r.idle(0.5);
        r.restart();
        r.rt.update(|s, _| crate::send_failed::stories::confirmed(s));
        r.play(0.5);
        assert_eq!(r.rt.state.unsent.state(crate::send_failed::stories::FIRST), Some(crate::send_failed::State::Failed));
        assert_eq!(r.rt.state.unsent.state(crate::send_failed::stories::SECOND), Some(crate::send_failed::State::Accepted));
    } },
    Action { name: "stop-background", circs: pinned, run: |r| {
        r.rt.update(|s, _| {
            crate::stop_stories::background(s);
            s.running.insert(SID.into());
        });
        r.idle(0.5);
        r.restart();
        r.event("session.execution.succeeded", json!({"sessionID": SID}));
        r.play(0.5);
        assert!(!r.rt.state.busy(SID));
        assert_eq!(r.rt.state.running_subagents(SID).len(), 6);
        assert!(r.rt.elements().iter().any(|e| !e.exiting && e.key.as_deref() == Some("submit") && e.label.as_deref() == Some("Send")));
    } },
    Action { name: "send", circs: || sends(Circ::default(), &[Dock::Pending]), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // Sends on a live server's order: the inbox echoes the prompt as a steer while it flies,
    // the run takes it up, and each step's start re-reads the page (`served.rs`). The first
    // reply thinks before it answers; the second send's re-read brings that reply back as the
    // server stored it, under the rows it already has.
    Action { name: "send-live", circs: || sends(Circ::default(), &[]), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(0.5);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "send-twice", circs: || places(Circ::default()), run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play(0.15);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // Read further up, a send flies nowhere: the timeline cuts to its end, the message there.
    Action { name: "send-from-up", circs: || vec![Circ { scrolled: true, ..Circ::default() }, Circ { scrolled: true, narrow: true, prompt: Prompt::Lines, ..Circ::default() }], run: |r| {
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
        assert!(r.rt.state.ends.at_end(SID), "the timeline shows its end after a send");
        let sent = r.rt.state.transcripts.get(SID).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. }))).map(|m| m.id().to_owned());
        let bubble = sent.and_then(|id| r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bubble") && e.path.split('/').any(|p| p == format!("u-{id}"))));
        assert!(bubble.is_some_and(|b| b.visible[3] > 0.0), "the sent message shows at the end");
    } },
    Action { name: "steer", circs: || sends(streaming(), &[Dock::Pending]), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    // A steer on a live server's order while it flies: the inbox echoes it under the client's
    // id, the running reply streams a tool row above it, the run takes it up, and the next
    // step's start re-reads the page (`served.rs`). Zoomed too, as the flight draws under it.
    Action { name: "steer-live", circs: || {
        let mut v = places(streaming());
        v.push(Circ { zoomed: true, ..streaming() });
        v.push(Circ { zoomed: true, scrolled: true, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.items.last()).map(|i| i.id.clone()) else { return };
        let text = r.circ.body();
        let item = json!({ "type": "user", "delivery": "steer", "payload": { "text": text, "metadata": { "displayText": text } } });
        r.event("session.inbox.enqueued", json!({ "sessionID": SID, "inboxID": id, "item": item }));
        let running = r.rt.state.transcripts.get(SID)
            .and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::Assistant(_))))
            .map(|m| m.id().to_owned());
        let tool = |r: &mut Run, reply: &str, call: &str| {
            r.event("session.tool.input.started", json!({ "sessionID": SID, "assistantMessageID": reply, "id": call, "name": "read" }));
            r.play(0.03);
            r.event("session.tool.called", json!({ "sessionID": SID, "assistantMessageID": reply, "id": call, "input": { "filePath": "src/net/reconnect.ts" }, "executed": true }));
        };
        if let Some(running) = running {
            r.play(0.03);
            tool(r, &running, "call_steer_live_a");
        }
        r.play(0.03);
        r.event_now("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        r.play(0.03);
        let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" });
        r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzsteerlive", "agent": "build", "model": model }));
        r.play(0.05);
        tool(r, "msg_zzzzsteerlive", "call_steer_live_b");
        r.play_out(1.0);
    } },
    Action { name: "queue", circs: || sends(streaming(), &[Dock::Pending]), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(true);
        r.play_out(1.0);
    } },
    Action { name: "steer-lands", circs: || places(streaming()), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.items.last()).map(|i| i.id.clone()) else { return };
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        r.play_out(1.0);
    } },
    // Two steers wait under Working; the first's delivery slides it up over Working while the
    // second stays below.
    Action { name: "steers-land", circs: || places(streaming()), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.items.last()).map(|i| i.id.clone()) else { return };
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        r.play_out(1.0);
    } },
    // A steer waits, the run stops, and a second send joins it under Working (t1361); then the
    // server delivers both in send order, so neither moves past the other.
    Action { name: "sends-behind-steers", circs: || places(streaming()), run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        r.event("session.execution.interrupted", json!({ "sessionID": SID, "reason": "user" }));
        r.play(0.4);
        r.type_prompt();
        r.enter(false);
        r.play(0.8);
        let ids: Vec<String> = r.rt.state.inboxes.get(SID).map(|i| i.steering().map(|i| i.id.clone()).collect()).unwrap_or_default();
        for id in ids {
            r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
            r.play(0.1);
        }
        r.play_out(1.0);
    } },
    // A shell blocks the run: "Move to background ⌃B" fades in beside Working, and ⌃B (or a
    // steer) moves it, so the call returns and the hint fades out (`background.rs`).
    Action { name: "background-key", circs: || wide_and_narrow(Circ::default()), run: |r| {
        background_shell(r);
        r.act("ctrl-b", |rt| rt.key(KeyEvent { key: Key::Text("b".into()), mods: Mods { ctrl: true, ..Mods::default() } }));
        r.play_out(1.0);
    } },
    Action { name: "background-steer", circs: || wide_and_narrow(Circ::default()), run: |r| {
        background_shell(r);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.6);
    } },
    // Zoomed too: the delivery flight draws under the window's scale.
    Action { name: "queue-lands", circs: || {
        let mut v = places(streaming());
        v.push(Circ { dock: Dock::Pending, ..streaming() });
        v.push(Circ { prompt: Prompt::Long, ..streaming() });
        v.push(Circ { zoomed: true, ..streaming() });
        v.push(Circ { zoomed: true, narrow: true, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play(1.2);
        r.type_prompt();
        r.enter(true);
        r.play(0.8);
        // The oldest queued prompt: the one just queued, or the dock's own when it had one.
        let Some(id) = r.rt.state.inboxes.get(SID).and_then(|i| i.queued().next()).map(|i| i.id.clone()) else { return };
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": id }));
        // A flight from the queue's left end to a right-aligned bubble crosses ~400 pt; its
        // spring's tail outlasts a 1 s tail.
        r.play_out(1.5);
    } },
    Action { name: "stream", circs: || {
        let mut v = places(streaming());
        v.push(Circ { dock: Dock::Question, ..streaming() });
        v.push(Circ { dock: Dock::Pending, ..streaming() });
        v
    }, run: |r| {
        r.start_stream();
        r.play_out(1.0);
    } },
    Action { name: "settings-search-reveal", circs: pinned, run: super::settings_search::reveal },
    Action { name: "stream-pace-slow", circs: pinned, run: super::stream_pace::slow },
    Action { name: "stream-pace-medium", circs: pinned, run: super::stream_pace::medium },
    Action { name: "stream-pace-fast", circs: pinned, run: super::stream_pace::fast },
    Action { name: "stream-pace-bursty", circs: pinned, run: super::stream_pace::bursty },
    Action { name: "line-reveal-clip", circs: pinned, run: super::line_reveal::clip },
    Action { name: "line-reveal-grow-then-show", circs: pinned, run: super::line_reveal::grow_then_show },
    Action { name: "line-reveal-soft-edge", circs: pinned, run: super::line_reveal::soft_edge },
    Action { name: "line-reveal-held-room", circs: pinned, run: super::line_reveal::held_room },
    Action { name: "stream-outline", circs: || wide_and_narrow(Circ::default()), run: super::genui::stream_outline },
    Action { name: "stream-diagram", circs: || wide_and_narrow(Circ::default()), run: super::genui::stream_diagram },
    Action { name: "diagram-full-size", circs: || {
        let base = Circ::default();
        vec![base, Circ { narrow: true, ..base }, Circ { zoomed: true, ..base }]
    }, run: super::diagram::open_zoom_close },
    Action { name: "tool-expand", circs: || wide_and_narrow(Circ::default()), run: |r| {
        // The narrow window's pinned tail can hold no tool group: scroll one into view.
        for _ in 0..4 {
            if trigger(r, None).is_some() { break; }
            r.swipe_up(160.0, 0.6);
        }
        let Some(group) = tap_trigger(r, None) else { return };
        r.play(0.6);
        tap_trigger(r, Some(&group));
        r.play(0.6);
    } },
    // Kit's capture (t1107): a one-thought, one-shell group lands shut before its steps unmount,
    // and they flashed over the prompts below it. Pressed open, shut, open, shut at his pace,
    // no step shows below the row on any frame.
    Action { name: "tool-group-shut-short", circs: || {
        let base = Circ { short_group: true, ..Circ::default() };
        vec![base, Circ { zoomed: true, ..base }]
    }, run: |r| {
        use crate::{short_group::GROUP, tool_group_shut_tests::overhang};
        let els = r.rt.elements();
        let path = els.iter().find(|e| e.key.as_deref() == Some("trigger") && e.path.ends_with(&format!("{GROUP}/#0/trigger"))).map(|e| e.path.clone()).expect("the short group's header");
        for secs in [0.68, 0.43, 0.48, 0.9] {
            tap_trigger(r, Some(&path));
            for _ in 0..(secs / (1.0 / 60.0)) as usize {
                r.play(1.0 / 60.0);
                let over = overhang(&r.rt.elements(), GROUP);
                assert!(r.rt.state.expanded.contains(GROUP) || over.is_empty(), "a shut group's steps show below its row: {over:?}");
            }
        }
    } },
    // A 40-tool group near the end opens and shuts in about 0.2 s with its header held where it
    // was pressed; a second press mid-motion springs back from there; scrolled into, its header sticks to
    // the top, and a press there shuts it in place (t988).
    Action { name: "tool-group-fold", circs: || {
        let base = Circ { long_group: true, ..Circ::default() };
        vec![base, Circ { narrow: true, ..base }, Circ { zoomed: true, ..base }]
    }, run: |r| {
        let group = format!("/{}", crate::tool_group_stories::GROUP);
        let path = trigger(r, None).filter(|t| t.0.contains(&group)).map(|t| t.0).expect("the long group's header");
        let held = |r: &Run| trigger(r, Some(&path)).map(|t| t.2);
        let near = |a: Option<f32>, b: Option<f32>| matches!((a, b), (Some(a), Some(b)) if (a - b).abs() < 0.5);
        let at = held(r);
        // The group's own height, header and steps, as presented.
        let row_h = |r: &Run| r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(crate::tool_group_stories::GROUP)).map_or(0.0, |e| e.rect[3]);
        let shut_h = row_h(r);
        tap_trigger(r, Some(&path));
        r.play(0.3);
        assert!(near(held(r), at), "{}: {:?} vs {:?}", "opening holds the header where it was pressed", held(r), at);
        let open_h = row_h(r);
        r.play(0.3);
        assert!(open_h > shut_h + 300.0 && (row_h(r) - open_h).abs() < 0.5, "a 40-tool group is open within 0.3 s: {shut_h} -> {open_h} -> {}", row_h(r));
        tap_trigger(r, Some(&path));
        r.play(0.3);
        assert!(near(held(r), at), "{}: {:?} vs {:?}", "shutting holds the header where it was pressed", held(r), at);
        assert!((row_h(r) - shut_h).abs() < 0.5, "and shut within 0.3 s: {} vs {shut_h}", row_h(r));
        r.play(0.3);
        tap_trigger(r, Some(&path));
        r.play(0.06);
        tap_trigger(r, Some(&path));
        r.play(0.5);
        assert!(near(held(r), at), "{}: {:?} vs {:?}", "a press mid-motion springs back with the header held", held(r), at);
        assert!(row_h(r) < shut_h + 0.5, "reversed mid-fold, it shuts again: {} vs {shut_h}", row_h(r));
        tap_trigger(r, Some(&path));
        r.play(0.5);
        // Scroll the header 200 pt past the timeline's top, into the open group.
        let viewport = format!("scroll-{SID}");
        let els = r.rt.elements();
        let top_of = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).map_or(0.0, |e| e.rect[1]);
        let dy = held(r).unwrap_or(0.0) - top_of(&viewport) + 200.0;
        r.swipe_down(dy, 0.6);
        let stuck = r.rt.elements().into_iter().find(|e| e.path == path).map(|e| e.stuck).unwrap_or(0.0);
        assert!(stuck > 0.0, "scrolled into the open group, its header sticks to the top");
        let top = held(r);
        tap_trigger(r, Some(&path));
        r.play(0.8);
        // It stays where it stuck unless the rows under it run out first: then the timeline's
        // end holds and the header settles down onto the rows above it.
        let end = r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("turn-actions")).map(|e| e.rect[1] + e.rect[3]);
        let floor = r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(viewport.as_str())).map(|e| e.rect[1] + e.rect[3]);
        let at_end = matches!((end, floor), (Some(e), Some(f)) if e <= f + 0.5);
        assert!(near(held(r), top) || (at_end && held(r) > top), "shutting from the stuck header leaves it where it stuck: {:?} vs {top:?}", held(r));
    } },
    // Kit's capture (p277): a one-shell group half under the timeline's top edge opens with its
    // shell call right under its row, which comes down to the edge rather than chasing it up.
    Action { name: "tool-group-open-at-top", circs: || {
        let base = Circ { shell_fold: true, ..Circ::default() };
        vec![base, Circ { zoomed: true, ..base }]
    }, run: |r| {
        let trigger = format!("{}/trigger", crate::shell_fold::GROUP);
        r.act("scroll the row half under the top", |rt| rt.update(move |_, tx| tx.scroll_key_to(trigger.clone(), -10.0)));
        r.play(0.5);
        let group = format!("/{}/", crate::shell_fold::GROUP);
        let path = r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some("trigger") && e.path.contains(&group)).map(|e| e.path).expect("the group's row");
        tap_trigger(r, Some(&path)).expect("the row shows enough to press");
        r.play(0.6);
        let els = r.rt.elements();
        let head = els.iter().find(|e| e.path == path).map(|e| e.rect).expect("the group's row");
        let step = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("tool-call_zzshellfold")).map(|e| e.visible);
        let shows = step.map_or(0.0, |v| v[1] + v[3] - v[1].max(head[1] + head[3]));
        assert!(shows >= 24.0 * r.rt.state.zoom, "the shell call shows under its row: {step:?} under {head:?}");
    } },
    // A long prompt lands folded; Read more opens it on the growth spring with its top held,
    // and Show less closes it again.
    Action { name: "fold", circs: || {
        let long = Circ { prompt: Prompt::Long, ..Circ::default() };
        let mut v = wide_and_narrow(long);
        v.push(Circ { length: Length::Empty, ..long });
        v
    }, run: |r| {
        // Folding is Settings › Experimental's Fold long messages, off by default as Desktop's.
        r.rt.update(|s, _| s.settings.fold_user = true);
        r.idle(0.5);
        r.restart();
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
        let opened = |r: &Run| r.rt.state.expanded.iter().any(|k| k.starts_with("user-expanded:"));
        r.act("Read more", |rt| Run::tap(rt, "fold-toggle"));
        assert!(opened(r), "Read more opens the prompt");
        r.play(0.8);
        // Down to Show less at the end of the opened prompt, its top now above the timeline:
        // closing holds the toggle where it's pressed.
        r.swipe_up(-1200.0, 0.8);
        r.act("Show less", |rt| Run::tap(rt, "fold-toggle"));
        assert!(!opened(r), "Show less folds it again");
        r.play(0.8);
    } },
    Action { name: "send-during-reflow", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.type_prompt();
        if tap_trigger(r, None).is_none() { return; }
        r.step_n(2);
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "working", circs: || {
        let mut v = places(Circ::default());
        v.push(Circ { dock: Dock::Question, ..Circ::default() });
        v
    }, run: |r| {
        let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzworking" });
        r.event("session.execution.started", m.clone());
        r.play(1.0);
        r.event("session.execution.succeeded", m);
        r.play(1.0);
    } },
    // The Working label changes mid-shimmer: the sweep keeps its rhythm, and once the run
    // ends nothing asks for frames.
    Action { name: "working-relabel", circs: || places(Circ::default()), run: |r| {
        let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzworking" });
        r.event("session.execution.started", m.clone());
        r.play(0.6);
        r.act("relabel", |rt| rt.update(|s, _| {
            let now = s.clock;
            s.working.relabel("Running the tests", now);
        }));
        r.play(1.4);
        r.event("session.execution.succeeded", m);
        r.quiet();
        r.play(1.5);
    } },
    Action { name: "question", circs: || {
        let mut v = places(Circ::default());
        v.extend(wide_and_narrow(Circ { dock: Dock::Pending, ..Circ::default() }));
        v
    }, run: |r| {
        let q = r.rt.state.fixture["question"].clone();
        r.event("form.created", q);
        r.play(0.8);
        r.event("form.replied", json!({ "id": "frm_fx0001", "sessionID": SID }));
        r.play(0.8);
    } },
    // Into "Type your own answer" and out: a digit opens it, the words wrap and the row grows,
    // ↑ goes to the answer above, ↓ and a key come back, Esc leaves. All of it at once.
    Action { name: "question-custom", circs: || vec![Circ { dock: Dock::Question, ..Circ::default() }, Circ { dock: Dock::Question, narrow: true, ..Circ::default() }], run: |r| {
        let key = |r: &mut Run, what: &str, key: Key| r.act(what, |rt| rt.key(KeyEvent { key, mods: Mods::default() }));
        key(r, "4", Key::Text("4".into()));
        r.play(0.3);
        r.act("a long answer", |rt| rt.update(|s, _| {
            let t = s.typed_mut("frm_fx0001", "q0");
            *t = rune::TextEdit::new("Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression shows up somewhere");
            t.focused = true;
        }));
        r.play(0.3);
        key(r, "↑", Key::Up);
        r.play(0.3);
        key(r, "↑", Key::Up);
        r.play(0.3);
        key(r, "↓", Key::Down);
        r.play(0.3);
        key(r, "!", Key::Text("!".into()));
        r.play(0.3);
        key(r, "esc", Key::Escape);
        r.play(0.5);
    } },
    // The agent asks: nothing joins the timeline while the dock asks, then the answer lands as
    // the call's row, each question over its answer (four questions), or as "Questions dismissed".
    Action { name: "question-answered", circs: || {
        let mut v = places(Circ::default());
        v.push(Circ { dock: Dock::Pending, ..Circ::default() });
        v
    }, run: |r| answer_question(r, 1) },
    Action { name: "question-dismissed", circs: || wide_and_narrow(Circ::default()), run: |r| answer_question(r, 4) },
    // The queued prompt is delivered while the question shows under it: the queue closes
    // into the question dock, then the question closes back to the composer.
    Action { name: "queue-lands-over-question", circs: || wide_and_narrow(Circ { dock: Dock::Pending, ..Circ::default() }), run: |r| {
        let q = r.rt.state.fixture["question"].clone();
        r.event("form.created", q);
        r.play(0.8);
        r.event("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": "msg_zzpending" }));
        r.play(1.5);
        r.event("form.replied", json!({ "id": "frm_fx0001", "sessionID": SID }));
        // The dock closing ~360 pt settles in about 0.75 s.
        r.play_out(1.5);
    } },
    Action { name: "revert", circs: || {
        let mut v = wide_and_narrow(Circ::default());
        v.push(Circ { length: Length::Short, ..Circ::default() });
        v
    }, run: |r| {
        let Some(to) = r.rt.state.transcripts.get(SID).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. }))).map(|m| m.id().to_owned()) else { return };
        let staged = json!({ "sessionID": SID, "revert": { "messageID": to } });
        r.event("session.revert.staged", staged.clone());
        r.play(0.8);
        r.event("session.revert.cleared", json!({ "sessionID": SID }));
        r.play(0.8);
        r.event("session.revert.staged", staged);
        r.play(0.5);
        r.event("session.revert.committed", json!({ "sessionID": SID, "to": to }));
        r.play(0.8);
    } },
    Action { name: "rewind", circs: || {
        let mut v = wide_and_narrow(Circ::default());
        v.push(Circ { length: Length::Short, ..Circ::default() });
        v.push(Circ { zoomed: true, ..Circ::default() });
        v
    }, run: super::rewind::run },
    Action { name: "reconnect", circs: || wide_and_narrow(Circ::default()), run: super::reconnect::run },
    // A compaction runs and ends as a quiet divider: its light while it runs, then its label's
    // ending. One a person asked for looks the same as an automatic one.
    Action { name: "compaction", circs: || places(Circ::default()), run: |r| compaction(r, "manual", 1.6) },
    Action { name: "compaction-quiet", circs: || places(Circ::default()), run: |r| compaction(r, "auto", 1.6) },
    // A quiet one that ends inside its grace fades in once, as Compacted.
    Action { name: "compaction-quiet-grace", circs: || wide_and_narrow(Circ::default()), run: |r| compaction(r, "auto", 0.2) },
    // Opened at the timeline's end, the Compacted divider holds and its summary pushes the end
    // down; scrolled into, it sticks, and shuts from there in place (p162).
    Action { name: "compaction-open-at-bottom", circs: || {
        let base = Circ { long_summary: true, ..Circ::default() };
        vec![base, Circ { narrow: true, ..base }, Circ { zoomed: true, ..base }]
    }, run: super::compaction_fold::open_at_bottom },
    Action { name: "switch-tab", circs: || wide_and_narrow(Circ::default()), run: |r| {
        let other = r.rt.state.tabs.iter().map(|t| t.id.clone()).find(|t| t != SID).expect("another tab");
        r.act("open another tab", |rt| rt.update(move |s, tx| s.open(&other, tx)));
        r.loaded();
        r.play(1.0);
        r.act("open this tab", |rt| rt.update(|s, tx| s.open(SID, tx)));
        r.loaded();
        r.play(1.0);
    } },
    // Thirty subagents: the capsule's list opens over the composer, scrolls inside its menu and
    // closes again.
    Action { name: "capsule-open-long", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.act("thirty agents run", |rt| rt.update(|s, _| crate::capsule_stories::run_thirty(s)));
        r.play(0.6);
        r.act("open the list", |rt| Run::tap(rt, "activity-chip"));
        r.play(0.6);
        r.act("scroll the list", |rt| {
            let (x, y) = rt.find("activity-scroll").expect("the list");
            rt.wheel(x, y, -120.0);
        });
        r.play(0.6);
        r.act("close it", |rt| Run::tap(rt, "activity-chip"));
        r.play_out(0.6);
    } },
    Action { name: "todo-pill", circs: || wide_and_narrow(Circ::default()), run: super::todos::todo_pill },
    // A permission takes the composer's place, the next one swaps in, and the composer comes back.
    Action { name: "permission", circs: || {
        let mut v = wide_and_narrow(Circ::default());
        v.push(Circ { dock: Dock::Pending, ..Circ::default() });
        v
    }, run: super::permission::permission },
    // A long question keeps to its share of the pane and scrolls its answers under the keys (p273).
    Action { name: "question-long", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }, Circ { zoomed: true, ..Circ::default() }], run: super::question_long::walk },
    // Kit's capture (t1129): thirty subagents and a plan run while a tool group in the last turn
    // opens and shuts; while the timeline shows its end, nothing in it shows under a pill.
    Action { name: "working-under-pills", circs: || {
        let base = Circ { short_group: true, ..Circ::default() };
        vec![base, Circ { narrow: true, ..base }, Circ { zoomed: true, narrow: true, ..base }]
    }, run: super::pills::working_clear },
    Action { name: "agents", circs: || wide_and_narrow(Circ::default()), run: agents },
    // Run times tick at once (`subagents::run_time`); counts roll like an odometer (`odometer.rs`).
    Action { name: "run-time-9s", circs: || wide_and_narrow(Circ::default()), run: |r| super::odometers::time(r, 9) },
    Action { name: "run-time-59s", circs: || wide_and_narrow(Circ::default()), run: |r| super::odometers::time(r, 59) },
    Action { name: "odometer-usage", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }], run: super::odometers::usage },
    Action { name: "odometer-questions", circs: || vec![Circ::default()], run: super::odometers::questions },
    Action { name: "composer-grow", circs: || vec![Circ::default(), Circ { narrow: true, zoomed: true, ..Circ::default() }], run: super::composer_grow::grow },
    Action { name: "odometer-queue", circs: || vec![Circ::default()], run: super::odometers::queue },
    Action { name: "agents-one", circs: || wide_and_narrow(Circ::default()), run: one_agent },
    Action { name: "agent-wakes", circs: || wide_and_narrow(Circ::default()), run: agent_wakes },
    Action { name: "agent-steered", circs: || wide_and_narrow(Circ::default()), run: super::steered::steered },
    // Dictation on the fake Hex: the toolbar's recording row comes and goes on the key, the
    // meter follows the voice, and the words land in the composer, which grows to hold them.
    Action { name: "dictate", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(1.0);
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(1.6);
    } },
    // Send while it listens: the words land, then the prompt goes as any send does.
    Action { name: "dictate-send", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| rt.key(dictation_key()));
        r.play(0.6);
        r.act("Send", |rt| Run::tap(rt, "submit"));
        r.play(1.6);
    } },
    // A slow Hex: "Preparing dictation…" opens a line under the toolbar and closes it.
    Action { name: "dictate-slow", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("⇧⌘M", |rt| {
            rt.update(|s, _| s.dictation.slow_fake());
            rt.key(dictation_key());
        });
        r.play(1.8);
        r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.6);
    } },
    Action { name: "dictate-cancel", circs: dictation_circs, run: |r| {
        r.type_prompt();
        r.act("tap the mic", |rt| Run::tap(rt, "mic"));
        r.play(0.6);
        r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.6);
    } },
    // A call transcript pasted whole: the composer grows to its cap and the text scrolls inside
    // it, typing at the end keeps the caret in view, and the wheel over it scrolls the prompt.
    Action { name: "long-paste", circs: || wide_and_narrow(Circ::default()), run: |r| {
        r.act("tap the composer", |rt| Run::tap(rt, "prompt"));
        r.act("paste", |rt| rt.paste(crate::stories::long_paste()));
        r.play(1.0);
        r.act("type", |rt| rt.key(KeyEvent { key: Key::Text(" and one more line".into()), mods: Mods::default() }));
        r.play(0.6);
        r.act("wheel over it", |rt| {
            let (x, y) = rt.find("editor").expect("the editor");
            rt.wheel(x, y, 120.0);
        });
        r.play_out(0.6);
    } },
    Action { name: "jump-latest", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }], run: |r| {
        r.swipe_up(1400.0, 1.0);
        if r.rt.find("jump-latest").is_none() { return; }
        r.act("tap jump-latest", |rt| Run::tap(rt, "jump-latest"));
        r.play(1.5);
    } },
    // A file dragged over the page: the frosted overlay fades in over 200 ms, out as the drag
    // leaves, in again, and out once the file lands in the composer (dropzone.rs).
    Action { name: "dropzone", circs: || wide_and_narrow(Circ::default()), run: |r| {
        // Its own file: the cells run in parallel, and each removes its file when it ends.
        let file = std::env::temp_dir().join(format!("oc-desktop-gate-drop-{}-{}.txt", std::process::id(), r.circ.name().replace('/', "-")));
        std::fs::write(&file, "notes").unwrap();
        let at = r.rt.find("panel").expect("the page");
        let drag = |rt: &mut Runtime<App>| rt.drag_files(vec![file.clone()], Some(at));
        r.act("drag a file over", &drag);
        r.play(0.5);
        r.act("drag it away", |rt| rt.drag_files_leave());
        r.play(0.5);
        r.act("drag it back", &drag);
        r.play(0.5);
        r.act("drop it", |rt| rt.drop_files(vec![file.clone()], Some(at)));
        r.play_out(0.6);
        std::fs::remove_file(&file).ok();
    } },
    // A drag selects a reply (its highlight is instant), then a prompt sends under it.
    Action { name: "select", circs: || wide_and_narrow(Circ::default()), run: |r| {
        let els = r.rt.elements();
        let words: Vec<_> = els.iter().filter(|e| !e.exiting && e.text.is_some() && e.path.contains("/t-msg_") && e.visible[3] >= 16.0 && e.visible[2] >= 40.0).collect();
        let (Some(first), Some(last)) = (words.first(), words.last()) else { panic!("no reply text on screen") };
        let (x0, y0) = (first.visible[0] + 1.0, first.visible[1] + 8.0);
        let (x1, y1) = (last.visible[0] + last.visible[2] - 1.0, last.visible[1] + last.visible[3] - 6.0);
        r.act("press on a reply", |rt| {
            rt.pointer_move(x0, y0);
            rt.pointer_down(x0, y0);
        });
        for i in 1..=6 {
            let t = i as f32 / 6.0;
            r.act("drag", |rt| rt.pointer_move(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        }
        r.act("release", |rt| rt.pointer_up(x1, y1));
        assert!(r.rt.selected_text().is_some_and(|t| !t.is_empty()), "the drag selects the reply");
        r.play(0.4);
        r.type_prompt();
        r.enter(false);
        r.play_out(1.0);
    } },
    Action { name: "scrollbar-scroll", circs: scroll_bar_circs, run: super::scrollbar::scroll_fade },
    Action { name: "scrollbar-drag", circs: scroll_bar_circs, run: super::scrollbar::drag },
    Action { name: "older-history", circs: || vec![Circ { scrolled: true, ..Circ::default() }, Circ { scrolled: true, narrow: true, zoomed: true, ..Circ::default() }], run: super::history::older },
    Action { name: "side-region", circs: || vec![Circ::default()], run: super::panels::side_region },
    Action { name: "review", circs: || vec![Circ::default()], run: super::panels::review },
    Action { name: "files", circs: || vec![Circ::default()], run: super::panels::files },
    Action { name: "tool-steps", circs: || vec![Circ::default()], run: super::panels::tool_steps },
    Action { name: "terminal-dock", circs: || vec![Circ::default()], run: super::panels::terminal_dock },
    Action { name: "model-efforts", circs: || vec![Circ::default(), Circ { zoomed: true, ..Circ::default() }], run: super::model_picker::run },
    Action { name: "layout-switch", circs: || vec![Circ::default()], run: super::layout_switch::run },
    Action { name: "find", circs: || vec![Circ::default(), Circ { zoomed: true, ..Circ::default() }], run: |r| {
        let mut cmd = Mods::default();
        cmd.cmd = true;
        r.act("⌘F", |rt| rt.key(KeyEvent { key: Key::Command('f'), mods: cmd }));
        r.play(0.2);
        for ch in "stream".chars() {
            r.act("type find", |rt| rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() }));
            r.play(0.08);
        }
        r.play(0.5);
        r.act("next match", |rt| rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() }));
        r.play(0.6);
        r.act("next match", |rt| rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() }));
        r.play(0.6);
        r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.4);
    } },
    // The pitch demo end to end (`demo.rs`): typing, streaming, tool groups, a steer, two
    // subagents, a question and the finish, at Kit's zoom too.
    Action { name: "demo", circs: || vec![Circ::default(), Circ { narrow: true, ..Circ::default() }, Circ { zoomed: true, ..Circ::default() }], run: |r| {
        crate::demo::start(&mut r.rt.state);
        let secs = crate::demo_script::events(&crate::demo_script::Ids::new(1)).last().map_or(0.0, |e| e.0);
        r.play(secs + 1.5);
    } },
];

/// The run starts a step whose shell call blocks it, and the hint shows.
fn background_shell(r: &mut Run) {
    let m = json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground" });
    r.event("session.execution.started", m.clone());
    r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground", "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }));
    r.play(0.3);
    r.event("session.tool.called", json!({ "sessionID": SID, "assistantMessageID": "msg_zzzzbackground", "id": "call_gate_shell", "name": "shell", "input": { "command": "cargo test" } }));
    r.play(1.0);
}

/// Background agents: two start a beat apart (the second joins the first's card) and step
/// along, then one finishes and the other fails.
fn agents(r: &mut Run) {
    use crate::agent_stories as st;
    r.act("an agent starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("it steps", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("another starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("both step", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("the first finishes", |rt| rt.update(st::finish_oldest));
    r.play(1.0);
    r.act("the second fails", |rt| rt.update(st::fail_newest));
    r.play(1.0);
}

/// One background agent on its own: a card with one row that starts, steps and finishes.
fn one_agent(r: &mut Run) {
    use crate::agent_stories as st;
    r.act("an agent starts", |rt| rt.update(st::spawn));
    r.play(1.0);
    r.act("it steps", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("it steps again", |rt| rt.update(st::advance));
    r.play(0.6);
    r.act("it finishes", |rt| rt.update(st::finish_oldest));
    r.play(1.0);
}

/// "Delegating agent…" wakes into a running row three times: the first call opens the card,
/// the next two join it while being written and wake in place there.
fn agent_wakes(r: &mut Run) {
    use crate::agent_stories as st;
    for _ in 0..3 {
        r.act("a call is written", |rt| rt.update(st::delegate));
        r.play(1.0);
        r.act("its agent starts", |rt| rt.update(st::spawn));
        r.play(0.8);
        r.act("it steps", |rt| rt.update(st::advance));
        r.play(0.6);
    }
}

fn dictation_circs() -> Vec<Circ> {
    let mut v = wide_and_narrow(Circ::default());
    v.push(Circ { prompt: Prompt::Lines, ..Circ::default() });
    v.push(Circ { length: Length::Empty, ..Circ::default() });
    v
}

fn dictation_key() -> KeyEvent {
    KeyEvent { key: Key::Command('m'), mods: Mods { cmd: true, shift: true, ..Mods::default() } }
}

/// The Question answers story's variant `v`, asked and then answered.
fn compaction(r: &mut Run, reason: &str, runs: f64) {
    r.event("session.compaction.started", json!({ "sessionID": SID, "eventID": "evt_gatecompaction", "reason": reason }));
    r.play(runs / 2.0);
    r.event("session.compaction.delta", json!({ "sessionID": SID, "text": "## Goal\nKeep reconnects alive." }));
    r.play(runs / 2.0);
    let tokens = json!({ "input": 10_412, "output": 1_034, "reasoning": 96, "cache": { "read": 1_880, "write": 0 } });
    r.event("session.compaction.ended", json!({ "sessionID": SID, "reason": reason, "text": "## Goal\nKeep reconnects alive.", "tokens": tokens }));
    r.quiet();
    r.play(1.5);
}

fn answer_question(r: &mut Run, v: usize) {
    use crate::question_stories as st;
    let send = |r: &mut Run, events: Vec<(&str, serde_json::Value)>| {
        for (ty, mut d) in events {
            d["sessionID"] = SID.into();
            r.event(ty, d);
        }
    };
    send(r, st::asked(v));
    r.play(0.8);
    send(r, st::answered(v));
    // Classic's taller dock closing settles in about a second, as the long question's does.
    r.play_out(1.5);
}

/// Opening a session through the real gate: cold (ready at 100 ms), prefetched, slow.
fn open_runs() -> Vec<(String, Run)> {
    let mut v = Vec::new();
    for (at, name) in [(0, "cold"), (1, "prefetched"), (2, "slow")] {
        for narrow in [false, true] {
            let mut app = App::empty();
            crate::open_story::setup(&mut app, at);
            super::load_builtins(&mut app);
            let size = if narrow { (720.0, 820.0) } else { (1280.0, 820.0) };
            let mut rt = Runtime::new(app, crate::view::app, size, Env::new(true, 1.0));
            rt.poll = Some(crate::poll);
            rt.on_key = Some(crate::on_key);
            let gate = super::sampler(&mut rt);
            rt.frame(0.0);
            let circ = Circ { narrow, ..Circ::default() };
            let mut run = Run { rt, gate, circ };
            run.idle(0.5);
            run.step();
            v.push((format!("open-session/{name}/{}", if narrow { "narrow" } else { "wide" }), run));
        }
    }
    v
}

fn open(r: &mut Run) {
    let state = |s: &App| (s.route.clone(), s.opening.is_some(), s.transcripts.get("ses_open_story").map(|t| t.messages.len()));
    let mut seen = state(&r.rt.state);
    // A prefetched session swaps in on the frame of the keypress itself.
    r.act("open", |rt| rt.key(KeyEvent { key: Key::Text("o".into()), mods: Mods::default() }));
    // Input remains live while the transcript is held, loading, and mounting.
    if let Some((x, y)) = r.rt.find("prompt") {
        r.act("focus during load", |rt| { rt.pointer_down(x, y); rt.pointer_up(x, y); });
        r.act("type during load", |rt| rt.key(KeyEvent { key: Key::Text("Draft".into()), mods: Mods::default() }));
        assert_eq!(r.rt.state.composer.text, "Draft");
    }
    for i in 0..=(1.5 / DT) as usize {
        if i > 0 {
            r.step_n(1);
        }
        let now = state(&r.rt.state);
        if now != seen {
            r.loaded();
            seen = now;
        }
    }
}

/// A visible tool group's disclosure in the timeline (`path`'s, or the first): its path and
/// a point on it.
fn trigger(r: &Run, path: Option<&str>) -> Option<(String, f32, f32)> {
    let els = r.rt.elements();
    let t = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("trigger") && e.path.contains("/c-") && e.visible[3] >= 12.0
        && path.is_none_or(|p| e.path == p))?;
    Some((t.path.clone(), t.rect[0] + 12.0, t.visible[1] + t.visible[3] / 2.0))
}

/// Taps [`trigger`]; returns the path tapped.
fn tap_trigger(r: &mut Run, path: Option<&str>) -> Option<String> {
    let (path, x, y) = trigger(r, path)?;
    r.act("tap a tool group", |rt| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    });
    Some(path)
}

impl Run {
    pub fn step_n(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }
}

/// Every cell: each action in each of its circumstances, then opening a session.
fn cells() -> Vec<(String, Job)> {
    let mut jobs: Vec<(String, Job)> = Vec::new();
    for a in ACTIONS {
        for c in (a.circs)() {
            let run = a.run;
            jobs.push((format!("{}/{}", a.name, c.name()), super::job(move || {
                let mut r = Run::new(c);
                run(&mut r);
                r.gate.rec
            })));
        }
    }
    for i in 0..6 {
        let (name, narrow) = (["cold", "prefetched", "slow"][i / 2], i % 2 == 1);
        jobs.push((format!("open-session/{name}/{}", if narrow { "narrow" } else { "wide" }), super::job(move || {
            let (_, mut r) = open_runs().swap_remove(i);
            open(&mut r);
            r.gate.rec
        })));
    }
    jobs
}

#[test]
fn every_message_animation_passes_the_motion_gate() {
    motion_gate::gate(cells(), ALLOW);
}
