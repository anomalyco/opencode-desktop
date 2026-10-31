//! An outline as plain HTML, for share pages and Workers: one element per line, classed by part
//! (`rp-box`, `rp-item`…) with `data-tone`, so the page's stylesheet draws it.

use crate::catalog::Tone;
use crate::outline::Node;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn to_html(nodes: &[Node]) -> String {
    let mut out = String::from("<div class=\"rp-ui\">");
    for n in nodes {
        node(&mut out, n);
    }
    out.push_str("</div>");
    out
}

fn node(out: &mut String, n: &Node) {
    let p = &n.part;
    let class = if n.spec().is_some() { esc(&p.name) } else if n.kids.is_empty() { "unknown".into() } else { "box".into() };
    out.push_str(&format!("<div class=\"rp-{class}\""));
    if let Some(t) = &p.tone {
        let tone = format!("{:?}", Tone::of(t)).to_lowercase();
        out.push_str(&format!(" data-tone=\"{tone}\" data-word=\"{}\"", esc(t)));
    }
    out.push('>');
    if !p.key.is_empty() {
        out.push_str(&format!("<b>{}</b> ", esc(&p.key)));
    }
    if n.spec().is_none() && n.kids.is_empty() {
        out.push_str(&esc(&format!("{} {}", p.name, n.words())));
    } else if !p.text.is_empty() {
        out.push_str(&format!("<span>{}</span>", esc(&p.text)));
    }
    if !p.detail.is_empty() {
        out.push_str(&format!(" <small>{}</small>", esc(&p.detail)));
    }
    for k in &n.kids {
        node(out, k);
    }
    out.push_str("</div>");
}
