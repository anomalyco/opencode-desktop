//! Settings › About (`settings/about/about.tsx`): the version (pressing it copies it), the
//! licence, the wordmark, the credits with the count of other contributors (read from GitHub
//! once, 935 until then), where it was first published and illustrated, the site, the
//! trademark, the typefaces, the third-party notices, the tagline, the MIT and globe marks and
//! the copyright over Anomaly's brush mark, centred in a 560 pt column.

use std::sync::atomic::{AtomicU32, Ordering};

use rune::{El, Icon, TextStyle, Tx, col, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use opencode_i18n::t;

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

/// The contributors past the 16 credited, as GitHub's contributor list's last page says. A
/// window asks the first time About shows, with or without a server, as Desktop does
/// (`loadOtherContributorCount`); headless runs and tests stay offline and show the fallback.
fn others(s: &App) -> u32 {
    if !crate::headless() {
        ASKED.call_once(|| {
            let waker = (*s.waker).clone();
            let _ = rune::task::spawn("about-contributors", move || {
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


pub fn view(s: &App, p: &Pal) -> El {
    let version = if s.updater.version.is_empty() { crate::updater::build_version().to_owned() } else { s.updater.version.clone() };
    let label = t!("settings.about.version", version = version);
    let copied = session_view::copied::is_copied(s, "settings-about-version");
    let line = |key: &str, words: &str, faint: bool| text(words.to_owned()).key(key.to_owned()).font(BODY).color(if faint { p.faint } else { p.muted }).wrap().align(rune::Align::Center);
    let mut writers: Vec<String> = WRITERS.iter().map(|w| (*w).to_owned()).collect();
    let others = others(s);
    writers.push(opencode_i18n::plural!("settings.about.otherContributor", others));
    let copy = label.clone();
    let version_button = row().key("settings-about-version").role("button").label(if copied { t!("common.copied") } else { t!("ui.message.copy") }).help(if copied { t!("common.copied") } else { t!("ui.message.copy") })
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
    let site = text(t!("settings.about.website")).key("settings-about-site").role("link").font(BODY).color(p.faint)
        .on_press_tx(|_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url("https://opencode.ai");
        });
    let notices = text(t!("settings.about.notices.title")).key("settings-about-notices").role("button").font(BODY).color(p.muted)
        .on_press_tx(|_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url("https://github.com/anomalyco/opencode/blob/dev/LICENSE");
        });
    let clipped = |key: &str, glyph: &'static Icon, w: f32, h: f32| el().key(key.to_owned()).size(w, h).clip_soft(0.0).child(icon(glyph, w));
    let content = col().key("settings-about-content").w_full().max_w(560.0).pt(80.0).pb(80.0).gap(24.0).items_center()
        .child(col().key("settings-about-intro").items_center().gap(14.0).child(version_button).child(line("license", t!("settings.about.license"), false)))
        .child(wordmark)
        .child(col().key("settings-about-credits").w_full().items_center().gap(16.0)
            .child(line("writers", &t!("settings.about.writtenByNames", names = opencode_i18n::list(&writers)), true))
            .child(line("illustrators", &t!("settings.about.illustratedByNames", names = opencode_i18n::list(&ILLUSTRATORS)), true)))
        .child(col().w_full().items_center().gap(16.0)
            .child(line("published", t!("settings.about.firstPublished"), false))
            .child(line("illustrated", t!("settings.about.firstIllustrated"), false)))
        .child(site)
        .child(col().key("settings-about-details").w_full().items_center().gap(16.0)
            .child(line("description", t!("settings.about.description"), false))
            .child(line("trademark", t!("settings.about.trademark"), false))
            .child(line("typeset", t!("settings.about.typeset"), false))
            .child(notices))
        .child(line("tagline", t!("settings.about.tagline"), false))
        .child(clipped("settings-about-legal", &LEGAL, 109.2, 19.2))
        .child(col().key("settings-about-copyright").w_full().items_center().gap(12.0)
            .child(line("copyright", t!("settings.about.copyright"), false))
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
    scramble: Scramble,
    /// The last poll's clock: a page opens in an input handler, which runs after its frame's
    /// poll, so this is when it opened (Desktop's effect starts its timers there and then).
    clock: f64,
}

/// The scramble's phase: spelled, opened at `since` and waiting for its first poll, or running
/// from `start` (the app's clock) with each letter's settle tick and the last tick drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scramble {
    Still,
    Pending { since: f64 },
    Running { start: f64, settles: [u8; 8], drawn: u8 },
}

enum Event {
    /// The page shows (`true`) or leaves.
    Active(bool),
    Poll { now: f64, reduced: bool },
}

/// What a step asks of the poll: draw the letters at once (an SVG `use` swaps, nothing glides),
/// and wake at the next tick.
#[derive(Default)]
struct Effects {
    drew: bool,
    wake: Option<f64>,
}

impl Default for Wordmark {
    fn default() -> Self {
        Wordmark { letters: TARGET, scramble: Scramble::Still, clock: 0.0 }
    }
}

impl Wordmark {
    pub fn activate(&mut self, active: bool) {
        self.step(Event::Active(active));
    }

    fn step(&mut self, event: Event) -> Effects {
        match (self.scramble, event) {
            (_, Event::Active(active)) => {
                self.scramble = if active { Scramble::Pending { since: self.clock } } else { Scramble::Still };
                self.letters = TARGET;
                Effects::default()
            }
            (Scramble::Still, Event::Poll { .. }) => Effects::default(),
            (Scramble::Pending { .. }, Event::Poll { reduced: true, .. }) => {
                self.scramble = Scramble::Still;
                Effects::default()
            }
            (Scramble::Pending { since }, Event::Poll { now, .. }) => {
                // Desktop's order: every letter's start, then every letter's settle tick.
                let mut random = random(now);
                let starts: [u8; 8] = std::array::from_fn(|_| pick(&mut random, CHOICES));
                let settles: [u8; 8] = std::array::from_fn(|_| 6 + pick(&mut random, 8));
                self.letters = starts;
                self.scramble = Scramble::Running { start: since, settles, drawn: 0 };
                // A first poll that comes late catches up on the ticks already due.
                let fx = self.step(Event::Poll { now, reduced: false });
                Effects { drew: true, wake: fx.wake }
            }
            (Scramble::Running { start, settles, drawn }, Event::Poll { now, .. }) => {
                // Desktop's timer for a tick fires on the frame its time is due, exactly on it too.
                let tick = ((now - start + 1e-9) / TICK).floor().clamp(0.0, 255.0) as u8;
                let last = settles.iter().copied().max().unwrap_or(0);
                let tick = tick.min(last);
                // Each tick is a timer of Desktop's that draws for the letters still moving, so a
                // frame that spans two runs both.
                let mut random = random(now);
                for at in drawn + 1..=tick {
                    for i in 0..8 {
                        self.letters[i] = if at >= settles[i] { TARGET[i] } else { pick(&mut random, CHOICES) };
                    }
                }
                let drew = tick > drawn;
                if tick >= last {
                    self.scramble = Scramble::Still;
                    return Effects { drew, wake: None };
                }
                self.scramble = Scramble::Running { start, settles, drawn: tick.max(drawn) };
                Effects { drew, wake: Some(start + f64::from(tick + 1) * TICK) }
            }
        }
    }
}

/// `Math.random`: the egg's stream, Desktop's page having one.
fn random(now: f64) -> crate::egg::rng::Rng {
    crate::egg::rng::Rng::new(now.to_bits())
}

/// `Math.floor(Math.random() * below)`.
fn pick(random: &mut crate::egg::rng::Rng, below: u8) -> u8 {
    (random.next() * f64::from(below)).floor() as u8
}

/// Runs the scramble on the app's clock: an opening's first letters, then each tick's.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let reduced = *s.reduced_motion;
    let w = &mut s.settings_pages.wordmark;
    if w.scramble == Scramble::Still {
        w.clock = now;
        return false;
    }
    let fx = w.step(Event::Poll { now, reduced });
    w.clock = now;
    if tracing() {
        trace(w, now);
    }
    if fx.drew {
        tx.instant();
    }
    if let Some(due) = fx.wake {
        tx.wake_after(std::time::Duration::from_secs_f64((due - now).max(0.001)));
    }
    fx.drew
}

/// `--about-trace <file>` (proof runs): the letters at each poll while they move.
fn tracing() -> bool {
    thread_local! {
        static ON: bool = crate::arg("--about-trace").is_some();
    }
    ON.with(|o| *o)
}

fn trace(w: &Wordmark, now: f64) {
    use std::io::Write;
    thread_local! {
        static OUT: std::cell::RefCell<Option<std::fs::File>> =
            std::cell::RefCell::new(crate::arg("--about-trace").and_then(|p| std::fs::File::create(p).ok()));
    }
    const NAMES: [char; 6] = ['o', 'p', 'e', 'n', 'c', 'd'];
    let letters: String = w.letters.iter().map(|&l| NAMES[usize::from(l)]).collect();
    OUT.with_borrow_mut(|f| {
        if let Some(f) = f {
            let _ = writeln!(f, "{{\"t\":{now:.6},\"letters\":\"{letters}\"}}");
        }
    });
}
/// The wordmark spelling `letters`, `k` points a unit of its 234 by 42 grid, each letter 30 on.
fn wordmark(letters: &[u8; 8], k: f32, stem: rune::Color, weak: rune::Color) -> El {
    // A picture: right to left doesn't mirror it. Desktop's is an SVG, whose edges fall between
    // pixels at this scale (178/234) and are anti-aliased there, not snapped.
    let mut logo = el().size(234.0 * k, 42.0 * k).dir(rune::Direction::Ltr).pixel_snap(false);
    for (i, &letter) in letters.iter().enumerate() {
        for &(x, y, w, h, shadow) in LETTERS[usize::from(letter) % LETTERS.len()] {
            logo = logo.child(el().key(format!("l{i}-{x}-{y}-{w}-{h}")).absolute((x + i as f32 * 30.0) * k, y * k).size(w * k, h * k).fill(if shadow { weak } else { stem }));
        }
    }
    logo
}
