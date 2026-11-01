use rune::{Key, KeyEvent, Mods, Runtime};

use super::*;
use crate::fixture_runtime as runtime;
use crate::pickers::Kind;

/// The popover keyed `key`, just opened: it starts smaller and faint, everything in it shows
/// as much as it does all the way (nothing fades in on its own after it), and it lands whole
/// within 0.2 s.
pub(crate) fn grows_whole(rt: &mut Runtime<App>, key: &str) {
    let host = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    let first = host(rt);
    assert!(first.scale < 0.99 && first.opacity < 0.9, "{key} grows in: scale {} opacity {}", first.scale, first.opacity);
    let inside = format!("{}/", first.path);
    let mut frames = Vec::new();
    for _ in 0..24 {
        let h = host(rt).opacity;
        // A spinner's dots pulse on their own loop.
        frames.push((h, rt.elements().into_iter().filter(|e| !e.exiting && e.loop_phase.is_none() && e.path.starts_with(&inside)).map(|e| (e.path, e.opacity)).collect::<Vec<_>>()));
        rt.frame(1.0 / 120.0);
    }
    let last = host(rt);
    assert!(last.scale > 0.999 && last.opacity > 0.999, "{key} lands whole: scale {} opacity {}", last.scale, last.opacity);
    let settled: std::collections::HashMap<String, f32> = rt.elements().into_iter().filter(|e| e.path.starts_with(&inside)).map(|e| (e.path, e.opacity)).collect();
    for (f, (h, els)) in frames.iter().enumerate() {
        for (path, o) in els {
            let full = settled.get(path).copied().unwrap_or(1.0);
            assert!(*o >= full * h - 0.03, "frame {f}: {path} fades in on its own ({o} while {key} is at {h})");
        }
    }
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}")).target
}

fn left_of_text(rt: &Runtime<App>, inside: &str) -> f32 {
    // The first text element under `inside`, by its key path.
    let els = rt.elements();
    let host = els.iter().position(|e| e.key.as_deref() == Some(inside)).unwrap_or_else(|| panic!("no {inside}"));
    els[host..].iter().find(|e| e.text.is_some()).unwrap().target[0]
}

#[test]
fn search_headers_and_labels_share_one_edge() {
    let mut rt = runtime();
    rt.state.open_picker(Kind::Model);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let menu = rect(&rt, "picker-model");
    let query = rect(&rt, "picker-search")[0] - menu[0];
    let header = left_of_text(&rt, "group-Anthropic") - menu[0];
    let label = left_of_text(&rt, "option-anthropic:claude-opus-5-5") - menu[0];
    assert!((query - LIST.inset - LIST.label_x()).abs() < 0.5, "query at {query}");
    // The model list's rail sits under the glass, so its headers and labels start one rail
    // width and its hairline after the query (p169); the glass is over the rail's marks.
    let rail = crate::model_picker::rail::W + 0.5;
    assert!((header - query - rail).abs() < 0.5, "header at {header}, query at {query}");
    assert!((label - query - rail).abs() < 0.5, "label at {label}, query at {query}");
    // The query is one line tall, on the glass's centre line.
    let field = rect(&rt, "picker-search");
    let glass = rect(&rt, "picker-search-row")[1] + LIST.inset + SEARCH_H / 2.0;
    assert!((field[3] - UI.line_height).abs() < 0.5, "field {} tall", field[3]);
    assert!((field[1] + field[3] / 2.0 - glass).abs() < 0.5, "field {field:?}, glass centre {glass}");
    // The highlight stays 6 pt inside the rail's edge.
    let row = rect(&rt, "option-anthropic:claude-opus-5-5");
    assert!((row[0] - menu[0] - rail - LIST.inset).abs() < 0.5);
}

#[test]
fn keys_mean_the_same_in_every_popover() {
    let ev = |key: Key, mods: Mods| KeyEvent { key, mods };
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    assert_eq!(nav(&ev(Key::Down, Mods::default())), Some(Nav::Step(1)));
    assert_eq!(nav(&ev(Key::Text("p".into()), ctrl)), Some(Nav::Step(-1)));
    assert_eq!(nav(&ev(Key::Enter, Mods::default())), Some(Nav::Choose));
    assert_eq!(nav(&ev(Key::Escape, Mods::default())), Some(Nav::Close));
    assert_eq!(nav(&ev(Key::Tab, Mods::default())), Some(Nav::Leave));
    assert_eq!(nav(&ev(Key::Text("x".into()), Mods::default())), None);
    assert_eq!(cycle(3, Some(0), -1), 2);
}

fn surface_el(rt: &Runtime<App>) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("picker-model"))
}

#[test]
fn popovers_grow_in_and_close_at_once_by_default() {
    let k = rune_knobs::Knobs::default();
    assert!(motion::opens(&k) && !motion::closes(&k));
    let t = motion::transition(&k, Side::Top, Align::Start);
    assert!(t.insertion.scale < 1.0 && t.insertion.opacity == 0.0, "it grows in");
    assert_eq!(t.removal, Transition::identity().removal, "it goes at once");
}

#[test]
fn grow_and_shrink_pivot_on_the_trigger() {
    let mut k = rune_knobs::Knobs::default();
    k.set(&motion::OPEN, 2.0);
    k.set(&motion::CLOSE, 2.0);
    let t = motion::transition(&k, Side::Top, Align::Start);
    assert_eq!((t.insertion.anchor.x, t.insertion.anchor.y), (0.0, 1.0), "above its trigger, it grows from its bottom-left");
    assert_eq!(t.removal.anchor, t.insertion.anchor);
    assert!(t.insertion.scale < 1.0 && t.insertion.opacity == 0.0);
    let t = motion::transition(&k, Side::Bottom, Align::End);
    assert_eq!((t.insertion.anchor.x, t.insertion.anchor.y), (1.0, 0.0));
}

#[test]
fn a_fade_plays_in_and_a_shrink_plays_out() {
    let mut rt = runtime();
    rt.state.knobs.set(&motion::OPEN, 1.0);
    rt.state.knobs.set(&motion::CLOSE, 2.0);
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.open_picker(Kind::Model);
        motion::opening(&s.knobs, tx);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let mid = surface_el(&rt).expect("open").opacity;
    assert!(mid > 0.0 && mid < 1.0, "fading in, opacity {mid}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(surface_el(&rt).unwrap().opacity, 1.0);
    rt.update(|s, tx| {
        s.close_picker(true);
        motion::closing(&s.knobs, tx);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let out = surface_el(&rt).expect("still leaving");
    assert!(out.exiting && out.scale < 1.0, "shrinking, scale {}", out.scale);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(surface_el(&rt).is_none());
}

fn fill(rt: &Runtime<App>) -> f32 {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("lit-fill")).map(|e| e.rect[1]).expect("a lit row")
}

fn options(rt: &Runtime<App>) -> Vec<[f32; 4]> {
    rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("option-"))).map(|e| e.rect).collect()
}

fn open_model_picker(rt: &mut Runtime<App>) {
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.open_picker(Kind::Model);
        motion::opening(&s.knobs, tx);
    });
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

/// The list Kit saw (p169): one model under several providers, a heading over each.
fn crowded(app: &mut App) {
    let model = |id: &str, provider: &str, name: &str| serde_json::json!({"id": id, "providerID": provider, "name": name, "variants": [{"id": "high"}, {"id": "xhigh"}]});
    let mut models = vec![model("claude-opus-5-5", "opencode", "Claude Opus 5.5")];
    for p in ["github-copilot", "openai", "console-openai-test"] {
        models.push(model("gpt-6.1-terra", p, "GPT-6.1 Terra"));
    }
    for p in ["console-moonshot-ai", "zai", "xai", "mistral", "groq", "deepseek"] {
        models.push(model(&format!("m-{p}"), p, &format!("Model {p}")));
    }
    let agents = serde_json::json!([{"id": "build", "model": {"id": "claude-opus-5-5", "providerID": "opencode", "variant": "xhigh"}}]);
    app.pickers.load_live(&serde_json::Value::Array(models), &agents);
    app.pickers.prefs = Default::default();
}

/// The pointer lights the row under it in the frame it gets there, at Kit's zoom and with the
/// list scrolled: a highlight that slid after the pointer sat a group or two above it (p169).
#[test]
fn the_pointer_lights_its_row_at_once() {
    for zoom in [1.0, 1.6] {
        let mut app = App::fixture_session();
        app.zoom = zoom;
        crowded(&mut app);
        let mut rt = app.into_runtime();
        rt.set_viewport((1400.0, 1000.0), 2.0);
        open_model_picker(&mut rt);
        let view = rect(&rt, "picker-scroll");
        let before = options(&rt)[0][1];
        rt.wheel(view[0] + view[2] / 2.0, view[1] + view[3] / 2.0, -120.0 * zoom);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        assert!(options(&rt)[0][1] < before - 10.0, "zoom {zoom}: the list scrolled");
        let shown: Vec<_> = options(&rt).into_iter().filter(|r| r[1] >= view[1] && r[1] + r[3] <= view[1] + view[3]).collect();
        assert!(shown.len() >= 3, "zoom {zoom}: rows in view {shown:?}");
        for row in [shown[shown.len() - 1], shown[0], shown[1]] {
            rt.pointer_move(row[0] + 40.0, row[1] + row[3] / 2.0);
            rt.frame(1.0 / 60.0);
            assert!((fill(&rt) - row[1]).abs() < 0.5, "zoom {zoom}: lit at {} under the pointer's row at {}", fill(&rt), row[1]);
        }
    }
}

/// The arrow keys slide the lit row's fill to the next row, and it lands there; a filter puts it
/// in place at once (t1150).
#[test]
fn arrow_keys_slide_the_highlight() {
    let mut rt = runtime();
    open_model_picker(&mut rt);
    let rows = options(&rt);
    let (a, b) = (rows[0], rows[1]);
    assert!((fill(&rt) - a[1]).abs() < 0.5, "under the first row: {} vs {}", fill(&rt), a[1]);
    rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let mid = fill(&rt);
    assert!(mid > a[1] + 1.0 && mid < b[1] - 1.0, "sliding: {mid} between {} and {}", a[1], b[1]);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!((fill(&rt) - b[1]).abs() < 0.5, "lands on the second row: {} vs {}", fill(&rt), b[1]);
    rt.update(|s, tx| {
        tx.instant();
        s.pickers.search = TextEdit::new("o");
        s.pickers.search.focused = false;
    });
    rt.frame(1.0 / 60.0);
    let lit = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("option-")) && e.fill.a > 0.0);
    assert!(lit.is_none(), "rows keep no fill of their own");
    let first = options(&rt)[0];
    assert!((fill(&rt) - first[1]).abs() < 0.5, "a filter lights its first row at once: {} vs {}", fill(&rt), first[1]);
}
