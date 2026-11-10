use crate::Route;

/// The header title's present (not leaving) and leaving copies, as (title, opacity).
fn titles(rt: &rune::Runtime<crate::App>) -> (Vec<(String, f32)>, Vec<(String, f32)>) {
    let all: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("title-ses_"))).collect();
    let pick = |leaving: bool| all.iter().filter(|e| e.exiting == leaving).map(|e| (e.text.clone().unwrap_or_default(), e.opacity)).collect();
    (pick(false), pick(true))
}

fn rename(rt: &mut rune::Runtime<crate::App>, to: &str) {
    rt.update(|s, _| {
        let Route::Session { id, .. } = s.route.clone() else { panic!("a session is open") };
        s.sessions.iter_mut().find(|x| x.id == id).unwrap().title = to.into();
    });
}

/// A rename crossfades: for a moment the old title is leaving while the new one fades in, and
/// then only the new one is left, at full strength.
#[test]
fn a_renamed_title_crossfades() {
    let mut rt = crate::fixture_runtime();
    rename(&mut rt, "Add dark mode to the settings page");
    rt.frame(0.08);
    let (now, leaving) = titles(&rt);
    assert_eq!(now.len(), 1, "one new title: {now:?}");
    assert!(now[0].1 < 0.95, "the new title is still fading in: {now:?}");
    assert_eq!(leaving.len(), 1, "the old title is leaving: {leaving:?}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let (now, leaving) = titles(&rt);
    assert!(leaving.is_empty() && (now[0].1 - 1.0).abs() < 1e-3, "settled on the new title: {now:?} {leaving:?}");
}

/// Opening another session shows its title at once: only a rename fades.
#[test]
fn another_session_shows_its_title_at_once() {
    let mut rt = crate::fixture_runtime();
    let other = rt.state.sessions.iter().map(|x| x.id.clone()).find(|id| Route::here(id.clone()) != rt.state.route).unwrap();
    rt.update(|s, tx| s.open_now(&other, tx));
    rt.frame(1.0 / 60.0);
    let (now, leaving) = titles(&rt);
    assert!(leaving.is_empty(), "the old session's title doesn't linger: {leaving:?}");
    assert!(now.iter().all(|t| (t.1 - 1.0).abs() < 1e-3), "the new title is at full strength: {now:?}");
}
