//! A static HTML document, not a browser tab. Only decoded text runs and bounded layout
//! values reach Rune. The document has no network, filesystem, script or app bridge.
//! Leave files requiring unsupported layout in the source viewer rather than pretending
//! that their markup or hidden text was displayed. Find consumes the exact painted runs.
//! The web build compiles this crate too, so its parser (`scraper`) is a dependency of
//! `features/files`.

use std::sync::Arc;

use opencode_search::{SlabFind, SlabMatch};
use rune::{Color, El, Family, ScrollSpec, Span, TextHighlight, TextStyle, Tx, col, text};
use scraper::{ElementRef, Html, Selector};

use super::label_find::LabelDoc;

pub struct Page {
    runs: Vec<Run>,
    slab: SlabFind,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HtmlPage")
            .field("runs", &self.runs.len())
            .finish()
    }
}

struct Run {
    words: String,
    spans: Vec<Span>,
    top: f32,
    font: TextStyle,
}

const BODY: TextStyle =
    TextStyle::new(16.0, 400.0).family(Family::Named("Times New Roman, Times, serif"));

#[derive(Clone, Copy, Default)]
struct Inline {
    bold: bool,
    italic: bool,
    mono: bool,
    pre: bool,
}

struct Flow {
    runs: Vec<Run>,
    current: String,
    spans: Vec<Span>,
    font: TextStyle,
    gap: f32,
    space: bool,
    visited: usize,
}

impl Page {
    pub(crate) fn parse(path: &str, source: &str) -> Option<Arc<Self>> {
        let ext = std::path::Path::new(path).extension()?.to_str()?;
        if !["html", "htm"].iter().any(|s| ext.eq_ignore_ascii_case(s))
            || source.len() > super::markdown::RENDER_LIMIT
        {
            return None;
        }
        let dom = Html::parse_document(source);
        // A stylesheet can hide, reorder or generate content. Don't ignore its cascade.
        if dom
            .select(&Selector::parse("style, link[rel~=stylesheet]").ok()?)
            .next()
            .is_some()
        {
            return None;
        }
        let body = dom.select(&Selector::parse("body").ok()?).next()?;
        let mut flow = Flow {
            runs: vec![],
            current: String::new(),
            spans: vec![],
            font: BODY,
            gap: 0.0,
            space: false,
            visited: 0,
        };
        flow.visit(body, Inline::default(), 0)?;
        flow.flush();
        let mut slab = SlabFind::default();
        for run in &flow.runs {
            slab.push(&run.words);
        }
        Some(Arc::new(Page {
            runs: flow.runs,
            slab,
        }))
    }
}

impl Flow {
    fn flush(&mut self) {
        if !self.current.is_empty() {
            self.runs.push(Run {
                words: std::mem::take(&mut self.current),
                spans: std::mem::take(&mut self.spans),
                top: self.gap,
                font: self.font,
            });
            self.gap = 0.0;
        }
        self.space = false;
    }

    fn words(&mut self, value: &str, i: Inline) {
        let start = self.current.len();
        if i.pre {
            self.current.push_str(value)
        } else {
            for c in value.chars() {
                if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000c}') {
                    self.space = true;
                    continue;
                }
                if self.space && !self.current.is_empty() && !self.current.ends_with('\n') {
                    self.current.push(' ')
                }
                self.space = false;
                self.current.push(c);
            }
        }
        if start < self.current.len() && (i.bold || i.italic || i.mono) {
            let mut span = Span::new(start, self.current.len());
            if i.bold {
                span = span.bold()
            }
            if i.italic {
                span = span.italic()
            }
            if i.mono {
                span = span.mono()
            }
            self.spans.push(span);
        }
    }

    fn visit(&mut self, e: ElementRef<'_>, mut i: Inline, depth: usize) -> Option<()> {
        self.visited += 1;
        if depth > 96 || self.visited > 8192 || self.runs.len() > 2048 {
            return None;
        }
        let v = e.value();
        let tag = v.name();
        if v.attr("hidden").is_some()
            || matches!(
                tag,
                "script" | "template" | "head" | "title" | "meta" | "link" | "base"
            )
        {
            return Some(());
        }
        let (mut top, mut bottom) = (0.0_f32, 0.0_f32);
        let block = match tag {
            "body" | "div" | "section" | "article" | "main" | "header" | "footer" | "nav"
            | "aside" | "address" | "figure" | "figcaption" => true,
            "p" | "pre" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                top = 16.0;
                bottom = 16.0;
                true
            }
            "span" | "em" | "strong" | "b" | "i" | "code" | "samp" | "kbd" | "br" => false,
            _ => return None,
        };
        for declaration in v
            .attr("style")
            .unwrap_or("")
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let (key, value) = declaration.split_once(':')?;
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_ascii_lowercase();
            match (key.as_str(), value.as_str()) {
                ("display", "none") => return Some(()),
                ("margin-top" | "margin-bottom", val) if block => {
                    let px: f32 = if val == "0" {
                        0.0
                    } else {
                        val.strip_suffix("px")?.trim().parse().ok()?
                    };
                    if !(0.0..=32768.0).contains(&px) {
                        return None;
                    }
                    if key == "margin-top" {
                        top = px
                    } else {
                        bottom = px
                    }
                }
                _ => return None,
            }
        }
        let previous = self.font;
        if block {
            self.flush();
            self.gap = self.gap.max(top)
        }
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.font.size = match tag {
                    "h1" => 32.0,
                    "h2" => 24.0,
                    "h3" => 18.72,
                    "h4" => 16.0,
                    "h5" => 13.28,
                    _ => 10.72,
                };
                self.font.weight = 700.0
            }
            "pre" => {
                i.pre = true;
                i.mono = true
            }
            "samp" | "code" | "kbd" => i.mono = true,
            "em" | "i" | "address" => i.italic = true,
            "strong" | "b" => i.bold = true,
            "br" => {
                self.space = false;
                self.current.push('\n')
            }
            _ => {}
        }
        for child in e.children() {
            if let Some(el) = ElementRef::wrap(child) {
                self.visit(el, i, depth + 1)?
            } else if let Some(t) = child.value().as_text() {
                self.words(t, i)
            }
        }
        if block {
            self.flush();
            self.gap = self.gap.max(bottom);
            self.font = previous
        }
        Some(())
    }
}

fn scope(sid: &str, path: &str) -> String {
    format!("file-html-{sid}-{path}-scroll")
}

fn key(n: usize) -> String {
    format!("html-run-{n}")
}

impl LabelDoc for Page {
    type Key = usize;
    fn scan(&self, query: &str) -> Vec<SlabMatch> {
        let mut hits = Vec::new();
        self.slab.find(query, |hit| hits.push(hit));
        hits
    }
    fn index(&self, n: usize) -> Option<usize> {
        (n < self.runs.len()).then_some(n)
    }
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        tx.scroll_center_in_instant(scope(sid, path), key(hit.row))
    }
}

pub(crate) fn view(
    sid: &str,
    path: &str,
    doc: &Arc<Page>,
    marks: impl Fn(&Arc<Page>, usize) -> Vec<TextHighlight>,
) -> El {
    let body = doc.runs.iter().enumerate().fold(
        col()
            .key("html-document")
            .role("document")
            .label(path.to_owned())
            .w_full()
            .px(8.0)
            .pb(8.0),
        |body, (n, run)| {
            body.child(
                text(run.words.clone())
                    .key(key(n))
                    .mt(run.top)
                    .w_full()
                    .font(run.font)
                    .color(Color::BLACK)
                    .spans(run.spans.clone())
                    .wrap()
                    .selectable(true)
                    .text_highlights(marks(doc, n)),
            )
        },
    );
    col()
        .key(scope(sid, path))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(Color::WHITE)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(body)
}
