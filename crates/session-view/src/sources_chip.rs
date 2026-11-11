//! Desktop's Sources experiment in the timeline (`session-ui/src/sources/sources-chip.tsx`,
//! `sources.css`): under a finished turn that searched the web or opened pages, "N sources"
//! with the first three sites' tiles, opening the turn in the Sources plugin's tab through a
//! link only that plugin takes. Off unless Knobs › Experiments › Sources is on.

use opencode_kit::model::{Message, Part, ToolStatus, Transcript, tool_output};
use rune::{El, Shadow, Tx, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::Pal;

/// The link the chip opens: the turn's user message id after it (the Sources plugin's `LINK`).
pub const LINK: &str = "opencode-sources:";

const COLORS: [&str; 8] = ["orange", "yellow", "cyan", "green", "red", "pink", "blue", "purple"];

/// What the chip shows of a turn: its prompt's id, how many pages it met, their first three
/// sites (each with the host it was met on, whose favicon a site icon shows).
pub struct Chip {
    pub prompt: String,
    pub count: usize,
    pub sites: Vec<(String, String)>,
}

/// The turn that reply `message` ends: its web searches' results and the pages it opened,
/// deduped by page, when it ran any (`turnSources`, `steps.length > 0`).
pub fn of(t: &Transcript, message: usize) -> Option<Chip> {
    let start = t.messages[..=message].iter().rposition(|m| matches!(m, Message::User { .. }))?;
    let Message::User { id: prompt, .. } = &t.messages[start] else { return None };
    let end = t.messages[start + 1..].iter().position(|m| matches!(m, Message::User { .. })).map_or(t.messages.len(), |n| start + 1 + n);
    let mut steps = 0;
    let mut seen: Vec<String> = Vec::new();
    let mut sites: Vec<(String, String)> = Vec::new();
    let mut count = 0;
    let mut meet = |url: &str| {
        let Some((site, key, host)) = page(url) else { return };
        if seen.contains(&key) {
            return;
        }
        seen.push(key);
        count += 1;
        if !sites.iter().any(|(s, _)| *s == site) {
            sites.push((site, host));
        }
    };
    for m in &t.messages[start + 1..end] {
        let Message::Assistant(a) = m else { continue };
        for part in &a.parts {
            let Part::Tool { name, status, input, metadata, .. } = part else { continue };
            if matches!(status, ToolStatus::Pending) {
                continue;
            }
            match name.as_str() {
                "websearch" => {
                    steps += 1;
                    if matches!(status, ToolStatus::Completed) {
                        for url in results(tool_output(status, metadata).unwrap_or_default()) {
                            meet(&url);
                        }
                    }
                }
                "webfetch" => {
                    let Some(url) = input["url"].as_str().filter(|u| page(u).is_some()) else { continue };
                    steps += 1;
                    if !matches!(status, ToolStatus::Error) {
                        meet(url);
                    }
                }
                _ => {}
            }
        }
    }
    sites.truncate(3);
    (steps > 0).then(|| Chip { prompt: prompt.clone(), count, sites })
}

/// The URLs of a search's `## [title](url)` result headings.
fn results(text: &str) -> Vec<String> {
    text.split('\n')
        .filter_map(|line| {
            if !line.starts_with("## [") || !line.ends_with(')') {
                return None;
            }
            let split = line.rfind("](").filter(|&i| i >= 4)?;
            Some(line[split + 2..line.len() - 1].to_owned())
        })
        .collect()
}

/// A web page's site (its host without `www.`), its identity (the site, path without a
/// trailing slash, and query) and its host.
fn page(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].rsplit('@').next()?.split(':').next()?.to_lowercase();
    if host.is_empty() {
        return None;
    }
    let site = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let tail = rest[end..].split('#').next().unwrap_or_default();
    let (path, query) = tail.split_once('?').map_or((tail, String::new()), |(p, q)| (p, format!("?{q}")));
    Some((site.clone(), format!("{site}{}{query}", path.trim_end_matches('/')), host))
}

/// The chip (`sources-chip`): 20 tall, 2 then 8 in, fully round on the hover overlay, its tiles
/// tucked 3 under each other with a gap cut round the one before, and "N sources" at 12/440 in
/// muted ink; hovered, the pressed overlay and base ink.
pub fn chip<S: TimelineHost>(s: &S, p: &Pal, chip: &Chip) -> El {
    let label = match chip.count {
        0 => opencode_i18n::t!("ui.sources.none").to_owned(),
        n => opencode_i18n::plural!("ui.sources.count", n),
    };
    let token = |name: &str| crate::palette::token(p.dark, name);
    // The chip's own surface: the hover overlay over the page.
    let a = p.hover.a;
    let behind = rune::Color { r: p.bg_base.r * (1.0 - a) + p.hover.r * a, g: p.bg_base.g * (1.0 - a) + p.hover.g * a, b: p.bg_base.b * (1.0 - a) + p.hover.b * a, a: 1.0 };
    let icons = if chip.sites.is_empty() {
        row().child(icon(&ic::GLOBE, 14.0).color(token("text-text-faint").unwrap_or(p.faint)).ml(4.0))
    } else {
        // The first tile on top, ringed in the chip's surface: the gap each later tile's mask
        // cuts round the one before it.
        let n = chip.sites.len();
        let site_icons = s.knobs().get(&crate::knobs::SOURCES_ICONS) > 0.5;
        let mut stack = el().w(16.0 + 13.0 * (n - 1) as f32).h(16.0);
        for (i, (site, host)) in chip.sites.iter().enumerate().rev() {
            let shown = if site_icons { site_icon(s, p, site, host) } else { tile(p, site) };
            let tile = shown.key(format!("site-{i}")).absolute(13.0 * i as f32, 0.0);
            stack = stack.child(if i + 1 < n { tile.outline(1.5, 0.0, behind) } else { tile });
        }
        row().child(stack)
    };
    let href = format!("{LINK}{}", chip.prompt);
    row()
        .key("sources-chip")
        .role("button")
        .label(label.clone())
        .h(20.0)
        .pl(2.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(10.0)
        .fill(p.hover)
        .hover(p.pressed)
        .hover_group()
        .child(icons)
        .child(text(label).font(rune::TextStyle::new(12.0, 440.0).lh(16.0).tracking(0.05)).color(p.muted).group_color(p.text).single_line())
        .on_press_tx(move |s: &mut S, tx: &mut Tx| s.open_link(&href, tx))
}

/// A site's own icon (`site-icon`, site): its favicon covering a 16 round tile on layer-02
/// (`[data-state]`), fading in over 150 ms once it loaded (`transition: opacity 150ms ease-out`),
/// and the letter tile in its place once it failed (`site-icon.tsx:13-44`).
/// `site-icon-image`'s `opacity` from 0 to 1 over 150 ms ease-out.
static FADE_IN: [rune::Beat; 2] = [rune::Beat::new(0.0).opacity(0.0), rune::Beat::new(0.15).opacity(1.0).easing(rune::Easing::EASE_OUT)];

fn site_icon<S: TimelineHost>(s: &S, p: &Pal, site: &str, host: &str) -> El {
    let pixels = match s.media_store().markdown.site_icon(host) {
        crate::images::markdown::SiteIcon::Failed => return tile(p, site),
        crate::images::markdown::SiteIcon::Loading => None,
        crate::images::markdown::SiteIcon::Loaded(pixels) => Some(pixels),
    };
    let token = |name: &str| crate::palette::token(p.dark, name);
    let ring = token("alpha-dark-12").unwrap_or(p.border);
    // It mounts as it shows and plays `opacity` 0 to 1 over 150 ms ease-out from that frame
    // (`sources.css`), cached or not: Chromium's `load` event lands after the `<img>` mounts.
    let tile = el().size(16.0, 16.0).radius(8.0).fill(p.layer2).box_shadow(Shadow { spread: 0.5, color: ring, inset: true, ..Shadow::default() }).clip_rounded(8.0);
    match pixels {
        Some(pixels) => tile.child(el().key("favicon").size(16.0, 16.0).pixels_fit(&pixels, rune::SurfaceFit::Fill).animated_changes().transition(rune::Transition::none()).sequence(rune::Sequence::new(&FADE_IN))),
        None => tile,
    }
}

/// A site's letter tile (`site-icon`, letters): 16 round on its avatar colour.
fn tile(p: &Pal, site: &str) -> El {
    let hash = site.chars().fold(2_166_136_261u32, |h, c| (h ^ c as u32).wrapping_mul(16_777_619));
    let color = COLORS[(hash % COLORS.len() as u32) as usize];
    let token = |name: &str| crate::palette::token(p.dark, name);
    let bg = token(&format!("avatar-bg-{color}")).unwrap_or(p.layer2);
    let ink = token("avatar-fg").unwrap_or(p.text);
    let ring = token("alpha-dark-12").unwrap_or(p.border);
    let letter = site.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    row()
        .size(16.0, 16.0)
        .center()
        .radius(8.0)
        .fill(bg)
        .box_shadow(Shadow { spread: 0.5, color: ring, inset: true, ..Shadow::default() })
        .child(text(letter).font(rune::TextStyle::new(9.0, 530.0).lh(9.0)).color(ink))
}
