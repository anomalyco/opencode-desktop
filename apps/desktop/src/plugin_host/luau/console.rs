//! The plugin console, as Roblox Studio's Output: every plugin's `print`, `warn`, errors and
//! reloads, newest last. An error is a card with its message, the source location as a link that
//! opens the file at that line, and its traceback; repeats fold into a count. It opens as a tab
//! in the side region (Show plugin console, Ctrl+Shift+J).

use std::cell::RefCell;
use std::collections::VecDeque;

use rune::{El, Tx, col, row, text};
use rune_luau::{LogLevel, Reload};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_SMALL};

/// The console's tab id under the host's `plugin` panel.
pub const TAB: &str = "luau-console";

pub struct Line {
    pub plugin: String,
    pub level: LogLevel,
    pub message: String,
    /// `file.luau:line` from the message, if it names one.
    pub at: Option<(String, u32)>,
    pub count: u32,
}

thread_local! {
    static LINES: RefCell<VecDeque<Line>> = const { RefCell::new(VecDeque::new()) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Adds a line from plugin `plugin`; a repeat of the last line counts up.
pub fn push(plugin: &str, level: LogLevel, message: &str) {
    if level != LogLevel::Info {
        crate::plugin_host::tasks::log(format!("plugin {plugin}: {message}"));
    }
    LINES.with_borrow_mut(|lines| {
        if let Some(last) = lines.back_mut()
            && last.plugin == plugin
            && last.level == level
            && last.message == message
        {
            last.count += 1;
            return;
        }
        if lines.len() == 500 {
            lines.pop_front();
        }
        lines.push_back(Line { plugin: plugin.to_owned(), level, message: message.to_owned(), at: location(message), count: 1 });
    });
    VERSION.set(VERSION.get() + 1);
}

pub fn reloaded(plugin: &str, r: Reload) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    crate::plugin_host::tasks::log(format!("plugin {plugin}: reloaded: compile {:.3} ms off the UI thread, swap {:.3} ms on it", ms(r.compile), ms(r.swap)));
    push(plugin, LogLevel::Info, &format!("Reloaded in {:.2} ms (compile {:.2} ms off the UI thread, swap {:.2} ms)", ms(r.compile) + ms(r.swap), ms(r.compile), ms(r.swap)));
}

/// The first `name.luau:line` in a message.
fn location(message: &str) -> Option<(String, u32)> {
    let at = message.find(".luau:")?;
    let start = message[..at].rfind(|c: char| c.is_whitespace() || c == '[' || c == '"' || c == '@').map_or(0, |i| i + 1);
    let digits: String = message[at + 6..].chars().take_while(char::is_ascii_digit).collect();
    Some((message[start..at + 5].to_owned(), digits.parse().ok()?))
}

/// The console tab.
pub fn view(app: &App, p: &Pal) -> El {
    rune::memo::memo_with_hash("luau-console", VERSION.get() ^ u64::from(p.dark) ^ (p.generation << 1), || {
        LINES.with_borrow(|lines| {
            let cards: Vec<El> = lines.iter().enumerate().map(|(n, l)| card(app, p, n, l)).collect();
            let head = row()
                .w_full()
                .center_y()
                .child(text(summary(lines.len(), lines.iter().filter(|l| l.level == LogLevel::Error).count())).font(UI).color(p.faint))
                .child(rune::spacer())
                .child(
                    row().key("luau-console-clear").role("button").label("Clear").h(24.0).px(8.0).radius(4.0).center().hover(p.hover).child(text("Clear").font(UI).color(p.muted)).on_press_tx(|_: &mut App, tx: &mut Tx| {
                        tx.instant();
                        LINES.with_borrow_mut(VecDeque::clear);
                        VERSION.set(VERSION.get() + 1);
                    }),
                );
            col().key("luau-console").w_full().h_full().pt(8.0).child(
                col().key("luau-console-scroll").w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: true }).child(col().w_full().px(16.0).py(12.0).gap(8.0).child(head).children(cards)),
            )
        })
    })
}

fn card(app: &App, p: &Pal, n: usize, l: &Line) -> El {
    let (ink, fill, border) = match l.level {
        LogLevel::Error => (p.danger, p.danger_bg, p.danger_border),
        LogLevel::Warn => (p.warning, p.layer1, p.border_muted),
        LogLevel::Info => (p.muted, p.layer1, p.border_muted),
    };
    let mut lines = l.message.lines();
    let first = lines.next().unwrap_or_default().to_owned();
    let rest: Vec<&str> = lines.filter(|s| !s.trim().is_empty()).take(12).collect();
    let mut head = row().w_full().gap(8.0).center_y().child(text(l.plugin.clone()).font(UI_SMALL).color(ink));
    if let Some((file, line)) = &l.at {
        let href = source_href(app, &l.plugin, file, *line);
        let label = format!("{file}:{line}");
        let link = text(label.clone()).font(UI_SMALL).color(p.accent).hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT);
        head = head.child(match href {
            Some(href) => row().key(format!("luau-console-at-{n}")).role("link").label(label).child(link).on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_reply_link(&href, tx)),
            None => row().child(link),
        });
    }
    if l.count > 1 {
        head = head.child(text(format!("×{}", l.count)).font(UI_SMALL).color(p.faint));
    }
    let mut c = col().key(format!("luau-console-{n}")).w_full().px(12.0).py(8.0).gap(4.0).radius(6.0).fill(fill).border(1.0, border).child(head).child(text(first).font(UI).color(p.text).wrap().selectable(true));
    if !rest.is_empty() {
        c = c.child(text(rest.join("\n")).font(rune::TextStyle::new(11.0, 440.0).lh(16.0).mono()).color(p.faint).wrap().selectable(true));
    }
    c
}

/// `file://` link to `file` of plugin `plugin` at `line`, when the plugin runs from a folder.
fn source_href(app: &App, plugin: &str, file: &str, line: u32) -> Option<String> {
    let p = app.plugins.luau.plugins.iter().find(|p| p.manifest.id == plugin)?;
    let path = p.origin.dir()?.join(file);
    Some(format!("file://{}#L{line}", path.display()))
}

fn summary(lines: usize, errors: usize) -> String {
    let word = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!("{}, {}", word(lines, "line", "lines"), word(errors, "error", "errors"))
}