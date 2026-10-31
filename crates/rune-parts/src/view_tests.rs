use rune::{El, Env, Runtime, col};

use crate::parse_outline;
use crate::view::{Pane, PartsResolver, PartsTheme, Plain, outline, plan_row};

/// Hex's flow from t694, the one Kit called "super ugly" (t1197).
const STEPS: &str = "row
  box You say
    text um cooper netties just shipped
  arrow
  box Hex hears it
    text on your Mac, no AI rewrite yet
  arrow
  box The steps run
    item fix words
    item {proposal} AI rewrite, in Slack
    item lowercase, in chat apps
  arrow
  box Hex pastes
    text kubernetes just shipped";

const CHAT: &str = "say Kit {voice}: clean up my Slack messages, and lowercase everything in chat apps
say agent: Added two steps and tested them.
  tools hex output add, hex output test";

/// Hex's failure flow from t694: box 3's item drew its detail over its words (t1197).
const FAILS: &str = "row
  box 1 · You dictate in Slack
    text the rewrite times out
    item {done} your normal dictation pastes
  arrow
  box 2 · A notice beside the HUD
    item {risk} Slack rewrite didn't run
    button Fix
  arrow
  box 3 · Output opens at that step
    item {risk} slack-cleanup · timed out after 4 s
    text hex.config.ts, line 7
    button Copy for agent";

fn theme() -> PartsTheme {
    let c = rune::oklch(0.6, 0.0, 0.0);
    PartsTheme { text: c, muted: c, faint: c, ring: c, bg: c, bubble: c, done: c, risk: c, proposal: c, working: c, accent: c }
}

/// Words that fill the width they're given, as Seer's rich text does.
struct Filling;
impl PartsResolver for Filling {
    fn inline(&self, src: &str, font: rune::TextStyle, color: rune::Color) -> El {
        rune::text(src.to_owned()).font(font).color(color).wrap().w_full()
    }
}

struct Page {
    src: &'static str,
    width: f32,
    /// The width the outline was told it has; a column can land narrower (a padded stage).
    plan: f32,
    filling: bool,
}

fn view(p: &Page, _: &mut rune::Cx<Page>) -> El {
    let nodes = parse_outline(p.src);
    let drawn = if p.filling { outline(&nodes, p.plan, &theme(), &Filling) } else { outline(&nodes, p.plan, &theme(), &Plain) };
    col().w_full().child(col().w(p.width).child(drawn))
}

fn laid_out(src: &'static str, width: f32) -> Runtime<Page> {
    laid_out_with(src, width, false)
}

fn laid_out_with(src: &'static str, width: f32, filling: bool) -> Runtime<Page> {
    planned_for(src, width, width, filling)
}

fn planned_for(src: &'static str, plan: f32, width: f32, filling: bool) -> Runtime<Page> {
    let mut rt = Runtime::new(Page { src, width, plan, filling }, view, (width + 40.0, 1200.0), Env::new(true, 1.0));
    for i in 0..3 {
        rt.frame(i as f64 / 60.0);
    }
    rt
}

fn rect(rt: &Runtime<Page>, key: &str) -> [f32; 4] {
    rt.presented(key).unwrap_or_else(|| panic!("{key} draws")).0
}

const BOXES: [&str; 4] = ["L2", "L5", "L8", "L13"];
const ARROWS: [&str; 3] = ["L4", "L7", "L12"];

/// A pane of one sentence `want` wide that wraps to two lines at half that (and three below).
fn sentence(want: f32) -> Pane {
    let floor = (want / 2.0).ceil();
    let heights = (0..).map(|i| floor + 4.0 * i as f32).take_while(|&w| w < want).map(|_| 42.0).chain([21.0]).collect();
    Pane { want, floor, max: f32::INFINITY, heights }
}

#[test]
fn a_row_fits_side_by_side_until_its_words_would_crush() {
    let four = [sentence(226.0), sentence(210.0), sentence(176.0), sentence(184.0)];
    let w = plan_row(&four, 3, 640.0).expect("four boxes fit a note column");
    assert!((w.iter().sum::<f32>() + 3.0 * 12.0 + 6.0 * 6.0 - 640.0).abs() < 4.0, "the row fills the column: {w:?}");
    assert!(w.iter().zip(&four).all(|(&x, p)| x >= p.floor), "no pane below its floor: {w:?}");
    assert_eq!(plan_row(&four, 3, 380.0), None, "a side panel stacks them");
    let short = [sentence(90.0), sentence(90.0)];
    assert_eq!(plan_row(&short, 1, 640.0).unwrap(), [308.0, 308.0], "short panes share the spare room");
    let mut tall = sentence(160.0);
    tall.heights = tall.heights.iter().map(|h| h + 60.0).collect();
    let w = plan_row(&[tall, sentence(300.0)], 1, 400.0).unwrap();
    assert_eq!(w[0], 160.0, "the tallest pane gets its room first: {w:?}");
}

#[test]
fn the_flow_draws_equal_boxes_with_arrows_on_their_middle() {
    let rt = laid_out(STEPS, 640.0);
    let boxes: Vec<[f32; 4]> = BOXES.iter().map(|k| rect(&rt, k)).collect();
    for b in &boxes {
        assert_eq!((b[1], b[3]), (boxes[0][1], boxes[0][3]), "every box shares one top and height: {boxes:?}");
    }
    for (i, a) in ARROWS.iter().enumerate() {
        let a = rect(&rt, a);
        assert!(a[0] >= boxes[i][0] + boxes[i][2] && a[0] + a[2] <= boxes[i + 1][0], "arrow {i} sits between its boxes");
        let (mid, box_mid) = (a[1] + a[3] / 2.0, boxes[0][1] + boxes[0][3] / 2.0);
        assert!((mid - box_mid).abs() < 1.0, "arrow {i} on the boxes' middle: {mid} vs {box_mid}");
    }
    // No sentence wraps past two lines.
    for k in ["L3", "L6", "L9", "L10", "L11", "L14"] {
        assert!(rect(&rt, k)[3] <= 42.0, "{k} wraps to at most two lines: {:?}", rect(&rt, k));
    }
}

#[test]
fn a_narrow_column_stacks_the_flow_with_full_width_boxes() {
    let rt = laid_out(STEPS, 380.0);
    let boxes: Vec<[f32; 4]> = BOXES.iter().map(|k| rect(&rt, k)).collect();
    for w in boxes.windows(2) {
        assert!(w[1][1] > w[0][1] + w[0][3], "stacked, one under another: {boxes:?}");
        assert_eq!(w[0][2], 380.0, "full width");
    }
}

#[test]
fn loose_chat_lines_share_one_panel_and_frames_get_none() {
    let rt = laid_out(CHAT, 640.0);
    let panel = rect(&rt, "panelL1");
    for k in ["L1", "L2"] {
        let r = rect(&rt, k);
        assert!(r[0] > panel[0] && r[1] >= panel[1] && r[1] + r[3] <= panel[1] + panel[3], "{k} inside the panel");
    }
    let rt = laid_out(STEPS, 640.0);
    assert!(rt.presented("panelL1").is_none(), "a row of frames isn't wrapped again");
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

#[test]
fn an_items_words_and_detail_never_overlap() {
    let widths = [(640.0, 640.0), (520.0, 520.0), (380.0, 380.0), (300.0, 300.0), (640.0, 580.0)];
    for ((plan, width), filling) in widths.into_iter().flat_map(|w| [(w, false), (w, true)]) {
        let rt = planned_for(FAILS, plan, width, filling);
        let (words, detail) = (rect(&rt, "L11.words"), rect(&rt, "L11.detail"));
        let at = format!("at {width} (planned {plan}){}", if filling { ", filling" } else { "" });
        assert!(!overlap(words, detail), "{at}: words {words:?} and detail {detail:?} overlap");
        let box3 = rect(&rt, "L10");
        for r in [words, detail] {
            assert!(r[0] >= box3[0] && r[0] + r[2] <= box3[0] + box3[2] + 0.5, "{at}: {r:?} stays inside box 3 {box3:?}");
        }
        assert!(detail[2] >= 90.0, "{at}: the detail shows whole, not cut to \"…\": {detail:?}");
    }
    for filling in [false, true] {
        let rt = laid_out_with("item fix words · 2 ms", 640.0, filling);
        let (words, detail) = (rect(&rt, "L1.words"), rect(&rt, "L1.detail"));
        assert!((words[1] - detail[1]).abs() < 3.0 && detail[0] >= words[0] + words[2], "a short detail follows on the line");
    }
}
