//! Settings search's reveal (`settings/search.rs`): a picked result shows its page, glides its
//! row to the middle and lights it, the light fading out over 1.8 s. The glide and the fade run
//! once each, with no snap, pop or restart.

use super::Run;

/// Opens Settings, then reveals Preferences' Spellcheck row from the search, as a picked result does.
pub fn reveal(r: &mut Run) {
    r.rt.update(|s, tx| {
        tx.instant();
        s.open_settings();
    });
    r.idle(0.5);
    r.restart();
    r.act("pick a search result", |rt| {
        rt.update(|s, tx| {
            let e = crate::settings::search::index(s).into_iter().find(|e| e.target.as_deref() == Some("settings-spellcheck")).expect("Spellcheck is in the search index");
            crate::settings::search::reveal(s, &e, tx);
        })
    });
    r.play(1.0);
    assert!(crate::settings::search::reveal_of(&r.rt.state, "settings-spellcheck").is_some(), "the light fades over 1.8 s");
    r.play(1.5);
    assert_eq!(crate::settings::search::reveal_of(&r.rt.state, "settings-spellcheck"), None, "faded after 1.8 s");
}
