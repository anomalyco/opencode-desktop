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
    let version = if s.updater.version.is_empty() { crate::updater::build_version().to_owned() } else { s.updater.version.clone() };
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
    let wordmark = wordmark(&s.settings_pages.wordmark.letters, 178.0 / 234.0, stem, weak).key("settings-about-wordmark");
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

/// The wordmark's letters, as `AnimatedWordmark` spells it: o p e n c o d e.
const TARGET: [u8; 8] = [0, 1, 2, 3, 4, 0, 5, 2];
/// The letters a scramble draws from: o, p, e, n, c, d.
const CHOICES: u8 = 6;
/// Each letter's rects in its 24 by 42 cell (`settings-about-letter-*`), `true` for its shadow.
const LETTERS: [&[(f32, f32, f32, f32, bool)]; 6] = [
    &[(0., 6., 24., 6., false), (0., 12., 6., 24., false), (18., 12., 6., 24., false), (6., 30., 12., 6., false), (6., 18., 12., 12., true)],
    &[(0., 6., 24., 6., false), (0., 12., 6., 30., false), (18., 12., 6., 24., false), (6., 30., 12., 6., false), (6., 18., 12., 12., true)],
    &[(0., 6., 24., 6., false), (0., 12., 6., 24., false), (18., 12., 6., 12., false), (6., 18., 12., 6., false), (6., 30., 18., 6., false), (6., 24., 18., 6., true)],
    &[(0., 6., 18., 6., false), (0., 12., 6., 24., false), (18., 12., 6., 24., false), (6., 18., 12., 18., true)],
    &[(0., 6., 24., 6., false), (0., 12., 6., 18., false), (0., 30., 24., 6., false), (6., 18., 18., 12., true)],
    &[(0., 6., 18., 6., false), (18., 0., 6., 36., false), (0., 12., 6., 24., false), (6., 30., 12., 6., false), (6., 18., 12., 12., true)],
];
/// A scramble's step (`tick * 75` ms).
const TICK: f64 = 0.075;

/// The wordmark's letters while the page shows (`AnimatedWordmark`'s `active`): each opening
/// starts every letter on a random one, which changes each 75 ms tick until its own settle tick,
/// 6 to 13, then stays right. Under Reduce Motion it is spelled at once.
pub struct Wordmark {
    pub letters: [u8; 8],
    /// The page opened and its scramble hasn't started.
    pending: bool,
    /// Each letter's settle tick.
    settles: [u8; 8],
    /// When the running scramble started (the app's clock) and the last tick it drew.
    started: Option<(f64, u8)>,
    seed: u64,
}

impl Default for Wordmark {
    fn default() -> Self {
        Wordmark { letters: TARGET, pending: false, settles: [0; 8], started: None, seed: 0x9E37_79B9_7F4A_7C15 }
    }
}

impl Wordmark {
    /// The page shows (`true`) or leaves: a scramble starts on the next poll, or the letters
    /// spell the word again.
    pub fn activate(&mut self, active: bool) {
        self.started = None;
        self.pending = active;
        self.letters = TARGET;
    }

    fn random(&mut self, below: u8) -> u8 {
        // xorshift64*: a scramble needs no better.
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        (self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u8 % below
    }
}

/// Runs the scramble on the app's clock: an opening's first letters, then each tick's.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let reduced = s.reduced_motion;
    let w = &mut s.settings_pages.wordmark;
    if std::mem::take(&mut w.pending) && !reduced {
        w.seed ^= now.to_bits() | 1;
        for i in 0..8 {
            w.letters[i] = w.random(CHOICES);
            w.settles[i] = 6 + w.random(8);
        }
        w.started = Some((now, 0));
        // A letter swaps for the next at once, as an SVG `use` does: nothing glides between them.
        tx.instant();
        tx.wake_after(std::time::Duration::from_secs_f64(TICK));
        return true;
    }
    let Some((start, drawn)) = w.started else { return false };
    let tick = ((now - start) / TICK).floor().clamp(0.0, 255.0) as u8;
    let last = w.settles.iter().copied().max().unwrap_or(0);
    if tick > drawn {
        tx.instant();
        for i in 0..8 {
            w.letters[i] = if tick >= w.settles[i] { TARGET[i] } else { w.random(CHOICES) };
        }
    }
    if tick >= last {
        w.started = None;
    } else {
        w.started = Some((start, tick.max(drawn)));
        tx.wake_after(std::time::Duration::from_secs_f64((start + f64::from(tick + 1) * TICK - now).max(0.001)));
    }
    tick > drawn
}

/// The wordmark spelling `letters`, `k` points a unit of its 234 by 42 grid, each letter 30 on.
fn wordmark(letters: &[u8; 8], k: f32, stem: rune::Color, weak: rune::Color) -> El {
    let mut logo = el().size(234.0 * k, 42.0 * k);
    for (i, &letter) in letters.iter().enumerate() {
        for &(x, y, w, h, shadow) in LETTERS[usize::from(letter) % LETTERS.len()] {
            logo = logo.child(el().key(format!("l{i}-{x}-{y}-{w}-{h}")).absolute((x + i as f32 * 30.0) * k, y * k).size(w * k, h * k).fill(if shadow { weak } else { stem }));
        }
    }
    logo
}
