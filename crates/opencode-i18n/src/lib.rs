//! OpenCode's UI strings in every language Desktop ships.
//!
//! `locales/official/<id>.json` are Desktop's own catalogs, its app and UI messages merged per
//! locale, copied by `sync.ts` from the opencode repository; keys keep Desktop's names
//! (`settings.general.row.language.title`, `ui.sessionReview.title`), with the catalogs of the GUI
//! plugins this app builds in under `plugin.<id>.`. `locales/intl/<id>.json` holds the number
//! and date patterns ICU gives each locale (`intl.percent`, `intl.date.*`, `intl.relative.*`),
//! also written by `sync.ts`.
//! `locales/rune/<id>.json` holds this app's strings Desktop doesn't have, under `rune.` keys,
//! and wins over the other layers. The build turns both into one static catalog (`rune_i18n::codegen`), so lookups are a
//! slice of an embedded blob and a key Desktop renames fails the build where it's used.
//!
//! - [`t!`]: a message, `t!("key")` as `&'static str` or `t!("key", name = value)` filled in.
//! - [`plural!`]: the message for a count's CLDR category, `{{count}}` filled in.
//! - [`date`]: dates, times and relative times as Desktop's `Intl` calls write them.
//! - [`set_locale`] switches every later lookup; [`detect`] picks the locale for the system's
//!   languages, as Desktop does when no language is chosen.

include!(concat!(env!("OUT_DIR"), "/i18n.rs"));

pub mod date;
mod intl;

use std::borrow::Cow;

pub use intl::plural_category;

pub use rune_i18n::Key;
pub use rune_i18n::LocaleInfo;
pub use rune_i18n::Part;
pub use rune_i18n::PluralCategory;
pub use rune_i18n::PluralKey;
pub use rune_i18n::parts;
pub use rune_i18n::render;

/// The translator every view reads.
pub static I18N: rune_i18n::I18n = rune_i18n::I18n::new(&CATALOG);

/// The message for a key, checked at compile time: `t!("key")` is the current locale's
/// `&'static str`, `t!("key", name = value, ...)` a `String` with its `{{name}}`s filled.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::I18N.text($crate::key!($key))
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::I18N.format($crate::key!($key), &[$((stringify!($name), &$value as &dyn ::core::fmt::Display)),+])
    };
}

/// A key as a constant, for tables of keys.
#[macro_export]
macro_rules! key {
    ($key:literal) => {{
        const KEY: $crate::Key = $crate::Key::named($crate::KEYS, $key);
        KEY
    }};
}

/// The message of a plural base for a count, with `{{count}}` and any `name = value`s filled.
#[macro_export]
macro_rules! plural {
    ($key:literal, $count:expr) => {{
        let count = ($count) as u64;
        $crate::render($crate::plural_text($crate::plural_key!($key), count), &[("count", &count)])
    }};
    ($key:literal, $count:expr, $($name:ident = $value:expr),+ $(,)?) => {{
        let count = ($count) as u64;
        $crate::render(
            $crate::plural_text($crate::plural_key!($key), count),
            &[$((stringify!($name), &$value as &dyn ::core::fmt::Display),)+ ("count", &count)],
        )
    }};
}

/// The message of plural base `key` for `count`'s category in the current locale (its
/// placeholders left), as Desktop picks `key.<category>` by `Intl.PluralRules`.
pub fn plural_text(key: PluralKey, count: u64) -> &'static str {
    I18N.plural_form(key, plural_category(count))
}

/// A plural base as a constant.
#[macro_export]
macro_rules! plural_key {
    ($key:literal) => {{
        const KEY: $crate::PluralKey = $crate::PluralKey::named($crate::PLURALS, $key);
        KEY
    }};
}

/// The current locale.
pub fn locale() -> LocaleInfo {
    I18N.info()
}

/// Every locale, in the order Desktop's Language setting lists them.
pub fn locales() -> &'static [LocaleInfo] {
    I18N.locales()
}

/// Makes the locale with id `id` (`de`, `zht`) current. Returns whether the locale changed; an
/// unknown id changes nothing.
pub fn set_locale(id: &str) -> bool {
    I18N.find_locale(id).is_some_and(|index| I18N.set_locale(index))
}

/// The id of the locale that best matches the system's preferred languages, else `en`.
pub fn detect() -> &'static str {
    let languages = rune_i18n::system_languages();
    locales()[I18N.negotiate(languages.iter().map(String::as_str))].id()
}

/// Whether the current locale is an English one (Desktop compares the tag's language).
pub fn is_english() -> bool {
    locale().tag().split('-').next().is_some_and(|language| language.eq_ignore_ascii_case("en"))
}

/// `source` in English, else `key`'s message: Desktop's `tDynamic`, which keeps copy generated at
/// runtime in English and uses the catalog for other locales.
pub fn t_dynamic(key: Key, source: &str) -> Cow<'_, str> {
    if is_english() {
        return Cow::Borrowed(source);
    }
    Cow::Borrowed(I18N.text(key))
}

/// `items` as the locale's long conjunction (`a, b, and c`, `a, b und c`), as Desktop's `list`.
pub fn list<S: AsRef<str>>(items: &[S]) -> String {
    intl::list(items)
}

/// `items` joined for the UI package's labels: commas only in English, the locale's conjunction
/// otherwise, as `@opencode/ui`'s `list` does.
pub fn ui_list<S: AsRef<str>>(items: &[S]) -> String {
    if is_english() {
        return items.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", ");
    }
    intl::list(items)
}

/// `n` with the locale's digits and grouping, as `Intl.NumberFormat(locale).format(n)`.
pub fn integer(n: i64) -> String {
    intl::integer(n)
}

/// `n` as a whole percentage in the locale's format (`150%`, `150 %`, `%150`), as
/// `Intl.NumberFormat(locale, { style: "percent" }).format(n)`.
pub fn percent(n: f64) -> String {
    t!("intl.percent", value = integer((n * 100.0).round() as i64))
}

/// `n` with at most `max_fraction` fraction digits in the locale's format.
pub fn decimal(n: f64, max_fraction: u8) -> String {
    intl::decimal(n, max_fraction)
}
