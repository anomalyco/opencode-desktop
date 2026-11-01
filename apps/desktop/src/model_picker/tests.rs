//! The efforts flyout opens on hover as Desktop's does (p169): after 100 ms the first time and at
//! once after that, kept by the safe triangle on the way in, following a wheel scroll and closed
//! by the arrow keys.

use rune::{Key, KeyEvent, Mods, Runtime};

use super::hover::in_triangle;
use crate::App;
use crate::pickers::Kind;

const DT: f64 = 1.0 / 60.0;

/// Ten providers with efforts on every model, at `zoom`, the list open.
fn runtime(zoom: f32) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    let efforts = serde_json::json!([{"id": "high"}, {"id": "xhigh"}]);
    let model = |id: &str, provider: &str| serde_json::json!({"id": id, "providerID": provider, "name": id, "variants": efforts});
    let mut models = vec![model("opus", "opencode"), model("sonnet", "opencode")];
    for p in ["github-copilot", "openai", "console-openai-test", "moonshot", "zai", "xai", "mistral", "groq", "deepseek"] {
        models.push(model(&format!("m-{p}"), p));
    }
    app.pickers.load_live(&serde_json::Value::Array(models), &serde_json::json!([{"id": "build"}]));
    app.pickers.prefs = Default::default();
    let mut rt = app.into_runtime();
    rt.set_viewport((2000.0, 1000.0), 2.0);
    rt.frame(DT);
    rt.update(|s, _| s.open_picker(Kind::Model));
    play(&mut rt, 0.5);
    rt
}

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap_or_else(|| panic!("no {key}")).rect
}

/// The model rows wholly inside the list's view, top to bottom, by model key.
fn visible(rt: &Runtime<App>) -> Vec<(String, [f32; 4])> {
    let view = rect(rt, "picker-scroll");
    let mut rows: Vec<_> = rt.elements().into_iter()
        .filter(|e| !e.exiting && e.rect[1] >= view[1] && e.rect[1] + e.rect[3] <= view[1] + view[3])
        .filter_map(|e| Some((e.key.as_deref()?.strip_prefix("option-")?.to_owned(), e.rect)))
        .collect();
    rows.sort_by(|a, b| a.1[1].total_cmp(&b.1[1]));
    rows
}

fn on(r: [f32; 4]) -> [f32; 2] {
    [r[0] + 40.0, r[1] + r[3] / 2.0]
}

fn point(rt: &mut Runtime<App>, [x, y]: [f32; 2]) {
    rt.pointer_move(x, y);
    rt.frame(DT);
}

fn flyout(rt: &Runtime<App>) -> Option<String> {
    rt.state.pickers.flyout.as_ref().map(|f| f.model.clone())
}

fn lit(rt: &Runtime<App>) -> String {
    rt.state.picker_active().unwrap()
}

/// The pointer on `row` until its efforts are open beside it.
fn open_on(rt: &mut Runtime<App>, row: &(String, [f32; 4])) {
    point(rt, on(row.1));
    play(rt, 0.2);
    assert_eq!(flyout(rt).as_deref(), Some(row.0.as_str()));
}

#[test]
fn the_first_flyout_waits_and_the_next_opens_at_once() {
    let mut rt = runtime(1.0);
    let rows = visible(&rt);
    point(&mut rt, on(rows[1].1));
    assert_eq!(lit(&rt), rows[1].0, "the row lights at once");
    play(&mut rt, 0.05);
    assert_eq!(flyout(&rt), None, "the first flyout waits out the pointer's intent");
    play(&mut rt, 0.1);
    assert_eq!(flyout(&rt).as_deref(), Some(rows[1].0.as_str()), "and opens after 100 ms");
    assert!(rt.find("effort-flyout").is_some());
    point(&mut rt, on(rows[2].1));
    assert_eq!(flyout(&rt).as_deref(), Some(rows[2].0.as_str()), "the next opens in the frame the pointer gets there");
}

#[test]
fn the_safe_triangle_keeps_the_flyout_on_the_way_in() {
    for zoom in [1.0, 1.6] {
        let mut rt = runtime(zoom);
        let rows = visible(&rt);
        open_on(&mut rt, &rows[1]);
        let fly = rect(&rt, "effort-flyout");
        let apex = on(rows[1].1);
        // On the next row, at its edge nearest the flyout: inside the triangle toward it.
        let row = rows[2].1;
        let x = if fly[0] > apex[0] { row[0] + row[2] - 4.0 } else { row[0] + 4.0 };
        let crossing = [x, row[1] + row[3] / 2.0];
        assert!(in_triangle(apex, fly, crossing), "zoom {zoom}: {crossing:?} heads for {fly:?} from {apex:?}");
        point(&mut rt, crossing);
        assert_eq!(lit(&rt), rows[1].0, "zoom {zoom}: crossing a row on the way leaves the highlight");
        assert_eq!(flyout(&rt).as_deref(), Some(rows[1].0.as_str()), "zoom {zoom}: and the flyout");
        point(&mut rt, [fly[0] + 20.0, fly[1] + 20.0]);
        play(&mut rt, 0.5);
        assert_eq!(flyout(&rt).as_deref(), Some(rows[1].0.as_str()), "zoom {zoom}: reaching it keeps it");

        // Resting on a row inside the triangle hands over to it, as on macOS.
        point(&mut rt, apex);
        point(&mut rt, crossing);
        play(&mut rt, 0.1);
        assert_eq!(flyout(&rt).as_deref(), Some(rows[1].0.as_str()), "zoom {zoom}: a short rest keeps it");
        play(&mut rt, 0.4);
        assert_eq!(lit(&rt), rows[2].0, "zoom {zoom}: a longer one lights the row");
        assert_eq!(flyout(&rt).as_deref(), Some(rows[2].0.as_str()), "zoom {zoom}: and opens its efforts");

        // Off the rows and outside the triangle, it closes.
        let search = rect(&rt, "picker-search-row");
        point(&mut rt, [search[0] + 40.0, search[1] + 10.0]);
        assert_eq!(flyout(&rt), None, "zoom {zoom}: leaving closes it");
    }
}

#[test]
fn a_wheel_scroll_moves_the_flyout_to_the_row_under_the_pointer() {
    let mut rt = runtime(1.6);
    let rows = visible(&rt);
    open_on(&mut rt, &rows[1]);
    let pointer = on(rows[1].1);
    let under_pointer = |rt: &Runtime<App>| visible(rt).into_iter().find(|(_, r)| pointer[1] >= r[1] && pointer[1] <= r[1] + r[3]);
    // Small steps until another row is under the pointer, not a provider's title.
    let mut under = None;
    for _ in 0..30 {
        rt.wheel_xy(pointer[0], pointer[1], 0.0, -8.0 * 1.6);
        play(&mut rt, 0.2);
        under = under_pointer(&rt).filter(|(k, _)| *k != rows[1].0);
        if under.is_some() { break; }
    }
    let under = under.expect("the list scrolled another row under the pointer");
    assert_eq!(lit(&rt), under.0, "the row now under the pointer is lit");
    assert_eq!(flyout(&rt).as_deref(), Some(under.0.as_str()), "and its efforts are open");
    let fly = rect(&rt, "effort-flyout");
    assert!(fly[1] <= under.1[1] + under.1[3] && fly[1] + fly[3] >= under.1[1], "beside it: {fly:?} by {:?}", under.1);
}

#[test]
fn a_keyboard_scroll_closes_the_flyout_and_keeps_the_keys_row() {
    let mut rt = runtime(1.6);
    let rows = visible(&rt);
    open_on(&mut rt, &rows[1]);
    let down = |rt: &mut Runtime<App>| {
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(DT);
    };
    down(&mut rt);
    assert_eq!(flyout(&rt), None, "the keys close the flyout");
    let before = rect(&rt, "picker-scroll");
    let first = rows[0].1[1];
    for _ in 0..12 {
        down(&mut rt);
    }
    play(&mut rt, 0.6);
    assert!(visible(&rt)[0].1[1] != first || rect(&rt, "picker-scroll") != before, "the keys scrolled the list");
    let keyed = lit(&rt);
    assert!(visible(&rt).iter().any(|(k, _)| *k == keyed), "the lit row is in view");
    let under = visible(&rt).into_iter().find(|(_, r)| on(rows[1].1)[1] >= r[1] && on(rows[1].1)[1] <= r[1] + r[3]).map(|r| r.0);
    assert_ne!(under.as_deref(), Some(keyed.as_str()), "the test's pointer rests on another row");
    assert_eq!(flyout(&rt), None, "and nothing reopens");
    play(&mut rt, 0.5);
    assert_eq!(lit(&rt), keyed, "rows scrolled under the still pointer leave the keys' highlight");
}

