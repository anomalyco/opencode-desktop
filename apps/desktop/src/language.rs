//! The display language, as Desktop's language provider has it (`runtime/i18n/language.tsx`):
//! Settings › Preferences › Language picks one of Desktop's locales (`opencode_i18n::locales`),
//! and until one is picked the system's preferred languages choose (`detectLocale`). A pick
//! switches every window at once: each rebuilds its memoized rows on its next poll.

use rune::Tx;

use crate::App;

/// The locale this window's view last built in.
#[derive(Debug, Default)]
pub struct Applied(Option<usize>);

/// Makes the saved language current, else `--language <id>`, else the system's; a headless run
/// with neither stays in English, so captures don't follow the machine they run on.
pub fn apply(s: &mut App) {
    if let Some(id) = crate::arg("--language") {
        s.settings.language = Some(id);
    }
    let id = match &s.settings.language {
        Some(id) => id.as_str(),
        None if crate::headless() => "en",
        None => opencode_i18n::detect(),
    };
    opencode_i18n::set_locale(id);
}

/// Settings' Language row: saves `id` and switches to it.
pub fn set(s: &mut App, id: &str) {
    if opencode_i18n::locales().iter().all(|l| l.id() != id) {
        return;
    }
    let id = id.to_owned();
    opencode_i18n::set_locale(&id);
    s.change_settings(|x| x.language = Some(id));
}

/// A switch made in any window: rebuilds this window's memoized subtrees, which keep the strings
/// they were built with.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = opencode_i18n::I18N.locale();
    if s.language.0 == Some(now) {
        return false;
    }
    let first = s.language.0.is_none();
    s.language.0 = Some(now);
    if first {
        return false;
    }
    tx.invalidate_memos_by_prefix("");
    true
}
