//! Settings › About (`settings/about/about.tsx`): the version (pressing it copies it), the
//! licence, the wordmark, the credits with the count of other contributors (read from GitHub
//! once, 935 until then), where it was first published and illustrated, the site, the
//! trademark, the typefaces, the third-party notices, the tagline, the MIT and globe marks and
//! the copyright over Anomaly's brush mark, centred in a 560 pt column.

use std::sync::atomic::{AtomicU32, Ordering};

use rune::{El, Icon, TextStyle, Tx, col, el, icon, row, text};

use crate::App;
use crate::palette::Pal;

/// `.settings-about-content`: 13 on 20 at 440, -0.04 tracking.
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

/// `FALLBACK_OTHER_CONTRIBUTORS`, until GitHub answers.
const FALLBACK_OTHERS: u32 = 935;
static OTHERS: AtomicU32 = AtomicU32::new(0);
static ASKED: std::sync::Once = std::sync::Once::new();

const WRITERS: [&str; 16] = ["thdxr", "adamdotdev", "rekram1-node", "kitlangton", "iamdavidhill", "jayair", "fwang", "brendonovich", "nexxeln", "hona", "kommander", "jlongster", "vimtor", "r44vcorp", "simonklee", "arvsrn"];
const ILLUSTRATORS: [&str; 4] = ["usrnk1", "ludvigrask_", "arvsrn", "iamdavidhill"];

static LEGAL: Icon = Icon { name: "about-legal", view: 91.0, body: include_str!("../../../assets/about/legal.svg.body") };
static BRUSH: Icon = Icon { name: "about-anomaly-brush", view: 110.0, body: include_str!("../../../assets/about/anomaly-brush.svg.body") };

/// The contributors past the 16 credited, as GitHub's contributor list's last page says.
fn others(s: &App) -> u32 {
    if !crate::headless() && !cfg!(test) && s.live() {
        ASKED.call_once(|| {
            let waker = s.waker.clone();
            std::thread::spawn(move || {
                let link = ureq::get("https://api.github.com/repos/anomalyco/opencode/contributors?anon=1&per_page=1")
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28")
                    .call()
                    .ok()
                    .and_then(|r| r.headers().get("link").and_then(|v| v.to_str().ok()).map(str::to_owned));
                let total = link.as_deref().and_then(last_page).filter(|n| *n > WRITERS.len() as u32);
                OTHERS.store(total.map_or(FALLBACK_OTHERS, |n| n - WRITERS.len() as u32), Ordering::Relaxed);
                if let Some(w) = waker {
                    w();
                }
            });
        });
    }
    match OTHERS.load(Ordering::Relaxed) {
        0 => FALLBACK_OTHERS,
        n => n,
    }
}

/// The `page=` of the `rel="last"` link in a Link header.
fn last_page(link: &str) -> Option<u32> {
    let last = link.split(',').find(|l| l.contains("rel=\"last\""))?;
    let at = last.find("page=")? + 5;
    last[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

/// "a, b, and c", as `Intl.ListFormat` reads a list.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

pub fn view(s: &App, p: &Pal) -> El {
    let version = if s.updater.version.is_empty() { env!("CARGO_PKG_VERSION").to_owned() } else { s.updater.version.clone() };
    let label = format!("Version {version}");
    let copied = session_view::copied::is_copied(s, "settings-about-version");
    let line = |key: &str, words: &str, faint: bool| text(words.to_owned()).key(key.to_owned()).font(BODY).color(if faint { p.faint } else { p.muted }).wrap().align(rune::Align::Center);
    let mut writers: Vec<String> = WRITERS.iter().map(|w| (*w).to_owned()).collect();
    let others = others(s);
    writers.push(format!("{others} other{}", if others == 1 { "" } else { "s" }));
    let copy = label.clone();
    let version_button = row().key("settings-about-version").role("button").label(if copied { "Copied" } else { "Copy" }).help(if copied { "Copied" } else { "Copy" })
        .h(24.0).my(-2.0).px(6.0).mx(-6.0).radius(4.0).center_y()
        .hover_group().hover(p.layer2)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.copy(copy.clone());
            session_view::copied::mark_copied(s, "settings-about-version", tx);
        })
        .child(text(label).font(BODY).color(p.muted).group_color(p.text));
    let ink = p.muted;
    let (stem, weak) = (ink.alpha(ink.a * 0.2), ink.alpha(ink.a * 0.2 * 0.3));
    let wordmark = crate::view::logo_at(178.0 / 234.0, stem, weak).key("settings-about-wordmark");
    let site = text("www.opencode.ai").key("settings-about-site").role("link").font(BODY).color(p.faint)
        .on_press_tx(|_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url("https://opencode.ai");
        });
    let notices = text("Third-party notices").key("settings-about-notices").role("button").font(BODY).color(p.muted)
        .on_press_tx(|_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url("https://github.com/anomalyco/opencode/blob/dev/LICENSE");
        });
    let clipped = |key: &str, glyph: &'static Icon, w: f32, h: f32| el().key(key.to_owned()).size(w, h).clip_soft(0.0).child(icon(glyph, w));
    let content = col().key("settings-about-content").w_full().max_w(560.0).pt(80.0).pb(80.0).gap(24.0).items_center()
        .child(col().key("settings-about-intro").items_center().gap(14.0).child(version_button).child(line("license", "Released under the MIT License", false)))
        .child(wordmark)
        .child(col().key("settings-about-credits").w_full().items_center().gap(16.0)
            .child(line("writers", &format!("Written by {}", list(&writers)), true))
            .child(line("illustrators", &format!("Illustrated by {}", list(&ILLUSTRATORS.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>())), true)))
        .child(col().w_full().items_center().gap(16.0)
            .child(line("published", "First published in Missouri, USA", false))
            .child(line("illustrated", "First illustrated in London, England", false)))
        .child(site)
        .child(col().key("settings-about-details").w_full().items_center().gap(16.0)
            .child(line("description", "OpenCode, the open source coding agent", false))
            .child(line("trademark", "OpenCode is a registered trademark of Anomaly Innovations, Inc.", false))
            .child(line("typeset", "Typeset in Inter and IBM Plex Mono", false))
            .child(notices))
        .child(line("tagline", "AI can\u{2019}t build great software, without you", false))
        .child(clipped("settings-about-legal", &LEGAL, 109.2, 19.2))
        .child(col().key("settings-about-copyright").w_full().items_center().gap(12.0)
            .child(line("copyright", "\u{a9} Anomaly Innovations, Inc.", false))
            .child(clipped("settings-about-anomaly-brush", &BRUSH, 126.72, 48.384)));
    col().key("settings-about").w_full().px(24.0).items_center().child(content)
}
