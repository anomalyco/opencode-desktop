//! A Mermaid diagram in a reply fits the reply column however narrow the window (p164).
use rune::{ElementInfo, Runtime};

use crate::App;
use crate::fixture_ids::MAIN;
use opencode_kit::model::{Assistant, Message, Part};

const WIDE_DIAGRAM: &str = "Here's the model:\n\n```mermaid\nflowchart TD\n  Note[(Note: lasts, has versions)] -.linked.- Chat\n  subgraph Chat[The chat: has a history]\n    A[your point] --> B[result 1] --> C[your reply] --> D[result 2]\n  end\n  D -->|newest turn is a question| N[Needs you]\n  D -->|newest turn is a result you haven't read| R[Ready]\n```\n\nThe note lives beside the chat.\n";

fn runtime(width: f32, zoom: f32) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    app.transcripts.get_mut(MAIN).expect("main").messages.push(Message::Assistant(Assistant {
        id: "msg_zzdiagram".into(),
        completed: Some(1),
        parts: vec![Part::Text { text: WIDE_DIAGRAM.into() }],
        ..Default::default()
    }));
    let mut rt = Runtime::new(app, super::app, (width, 760.0), rune::Env::new(true, 1.0));
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

fn el(rt: &Runtime<App>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5 && inner[1] >= outer[1] - 0.5 && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5 && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

/// Rects are in screen points; the zoomed shell scales them all alike, so containment holds at any zoom.
fn check(width: f32, zoom: f32) {
    let rt = runtime(width, zoom);
    let column = el(&rt, "t-msg_zzdiagram-0").rect;
    let card = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("b1") && e.path.contains("t-msg_zzdiagram-0")).expect("the diagram's card").rect;
    let diagram = el(&rt, "diagram").rect;
    let scroll = el(&rt, &format!("scroll-{MAIN}")).rect;
    let what = format!("window {width} zoom {zoom}: scroll {scroll:?} column {column:?} card {card:?} diagram {diagram:?}");
    assert!(inside(diagram, card), "diagram leaves its card: {what}");
    assert!(inside(card, column), "card leaves the column: {what}");
    assert!(column[0] >= scroll[0] - 0.5 && column[0] + column[2] <= scroll[0] + scroll[2] + 0.5, "column leaves the timeline: {what}");
    let drawn = |e: &ElementInfo| !e.exiting && e.path.contains("t-msg_zzdiagram-0") && e.rect[1] >= card[1] && e.rect[1] < card[1] + card[3];
    let labels: Vec<_> = rt.elements().into_iter().filter(|e| drawn(e) && (e.text.is_some() || e.key.as_deref().is_some_and(|k| k.starts_with("node")))).collect();
    assert!(labels.len() >= 8, "the diagram's labels and nodes draw: {what}");
    for e in labels {
        assert!(inside(e.rect, card), "{:?} {:?} leaves the card: {what}", e.key, e.text);
    }
}

#[test]
fn a_wide_diagram_fits_a_narrow_column() {
    for zoom in [1.0, 1.6] {
        check(560.0 * zoom, zoom);
    }
}

#[test]
fn a_wide_diagram_still_fits_a_wide_window() {
    for zoom in [1.0, 1.6] {
        check(1800.0 * zoom, zoom);
    }
}
