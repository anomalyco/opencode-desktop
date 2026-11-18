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
    s.language.0 = Some(opencode_i18n::I18N.locale());
    rune::set_labels(label);
    sync_runtime_locale();
}

/// Settings' Language row: saves `id` and switches to it, the macOS menu bar included.
pub fn set(s: &mut App, id: &str) {
    if opencode_i18n::locales().iter().all(|l| l.id() != id) {
        return;
    }
    let id = id.to_owned();
    if opencode_i18n::set_locale(&id) {
        sync_runtime_locale();
        crate::menubar::refresh();
    }
    s.change_settings(|x| x.language = Some(id));
}

fn sync_runtime_locale() {
    text_language();
    rune::labels::set_locale(opencode_i18n::t!("intl.names.locale"));
    // Rune's native menus (text fields', Windows' window menu, macOS's) read the language's way.
    rune::labels::set_direction(crate::view::ui_direction());
}

/// How searches fold in the interface's language: Turkish and Azerbaijani fold `I` to `ı`.
///
/// The interface language is the text's, as Desktop's `<html lang>` (`language.tsx:396`): Han
/// characters take its forms and fonts (Yu Gothic UI's for Japanese, Microsoft JhengHei's for
/// Traditional Chinese) where the text itself doesn't say (kana, Hangul and Bopomofo do).
pub fn search_fold() -> rune_kit::search::Fold {
    rune_kit::search::Fold::for_language(opencode_i18n::locale().tag())
}

fn text_language() {
    rune::text::set_language(Some(opencode_i18n::locale().tag()));
}

/// Rune's own words in the chosen language, as Desktop translates them: the text menu's Cut,
/// Copy, Paste and Select All (`desktop.menu.*`, environment.ts), its Spellcheck items
/// (`desktop.contextMenu.*`, spellcheck.ts) and the macOS menu bar's Edit items and menu titles
/// (native/menu.ts), and the dictionaries by Chromium's names for them (`nativeLanguageOptions`),
/// which Rune sorts in `intl.names.locale`. Learn Spelling, No Guesses Found and the app and
/// Window menus' role items stay in English, as Electron's are.
fn label(label: rune::Label<'_>) -> Option<String> {
    let key = match label {
        rune::Label::Language(tag) => return Some(opencode_i18n::names::language_name(tag)),
        rune::Label::Cut => opencode_i18n::key!("desktop.menu.cut"),
        rune::Label::Copy => opencode_i18n::key!("desktop.menu.copy"),
        rune::Label::Paste => opencode_i18n::key!("desktop.menu.paste"),
        rune::Label::SelectAll => opencode_i18n::key!("desktop.menu.selectAll"),
        rune::Label::Undo => opencode_i18n::key!("desktop.menu.undo"),
        rune::Label::Redo => opencode_i18n::key!("desktop.menu.redo"),
        rune::Label::Spellcheck => opencode_i18n::key!("desktop.contextMenu.spellcheck"),
        rune::Label::SpellcheckLanguages => opencode_i18n::key!("desktop.contextMenu.spellcheckLanguages"),
        rune::Label::Menu("File") => opencode_i18n::key!("desktop.menu.file"),
        rune::Label::Menu("Edit") => opencode_i18n::key!("desktop.menu.edit"),
        rune::Label::Menu("View") => opencode_i18n::key!("desktop.menu.view"),
        rune::Label::Menu("Go") => opencode_i18n::key!("desktop.menu.go"),
        rune::Label::Menu("Window") => opencode_i18n::key!("desktop.menu.window"),
        rune::Label::Menu("Help") => opencode_i18n::key!("desktop.menu.help"),
        _ => return None,
    };
    Some(opencode_i18n::I18N.text(key).to_owned())
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
    sync_runtime_locale();
    crate::menubar::refresh();
    tx.invalidate_memos_by_prefix("");
    crate::plugin_host::services::locale_changed(s);
    crate::plugin_host::dormant::locale_changed(s);
    true
}
