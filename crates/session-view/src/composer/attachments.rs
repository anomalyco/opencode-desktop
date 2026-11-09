//! Composer attachment encoding, image paste, file helpers, and thumbnail/file chip row (`p254`).

use std::path::Path;

use opencode_kit::client::{FileInput, PathAttachment, data_url};
use opencode_kit::model::{Attachment, Message, Transcript};
use rune::{ClipboardContent, El, Live, ScrollPos, SurfaceFit, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::ComposerHost;
use crate::images::{identity, opens};
use crate::palette::Pal;
use crate::user_parts::{CARD_SUBTITLE, CARD_TITLE};

/// The server's cap on one inline attachment (20 MB).
pub const MAX_BYTES: usize = 20 << 20;

/// The MIME type by extension of media the model reads natively, else `None`.
pub fn native_mime(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

/// `attachmentMime`: native media by extension; otherwise text when the first 4 KB has no NUL
/// and few control bytes, else binary.
pub fn mime_for(path: &Path, head: &[u8]) -> &'static str {
    if let Some(m) = native_mime(path) {
        return m;
    }
    let control = head.iter().filter(|&&b| b < 9 || (b > 13 && b < 32)).count();
    if head.contains(&0) || (!head.is_empty() && control as f32 / head.len() as f32 > 0.3) {
        return "application/octet-stream";
    }
    "text/plain"
}

pub fn name_of(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_owned()
}

/// Reads up to 4 KB from `path` for MIME sniffing.
pub fn head(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(4096);
    std::fs::File::open(path)
        .and_then(|f| f.take(4096).read_to_end(&mut buf))
        .map_err(|e| format!("{}: {e}", name_of(path)))?;
    Ok(buf)
}

/// Loads a native image/PDF file small enough to travel inline.
pub fn from_path(path: &Path) -> Result<Attachment, String> {
    let name = name_of(path);
    let mime = native_mime(path).ok_or_else(|| format!("{name} doesn't go inline"))?;
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("{name} is over 20 MB"));
    }
    Ok(Attachment { mime: mime.into(), name, bytes: bytes.into(), media: None })
}

/// Appends `Attached file:` lines for path-referenced attachments.
pub fn path_lines(text: &str, refs: &[PathAttachment]) -> String {
    let lines = refs.iter().map(|r| format!("Attached file: `{}`", r.path));
    (!text.trim().is_empty())
        .then(|| text.to_owned())
        .into_iter()
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Encodes inline `Attachment`s into `PromptInput.FileAttachment` values (`data:` URL or `media:` ref).
pub fn inputs(files: &[Attachment]) -> Vec<FileInput> {
    files
        .iter()
        .map(|f| match &f.media {
            Some(hash) => FileInput {
                uri: format!("media:{hash}"),
                name: f.name.clone(),
                mime: Some(f.mime.clone()),
                mention: None,
            },
            None => FileInput {
                uri: data_url(&f.mime, &f.bytes),
                name: f.name.clone(),
                ..Default::default()
            },
        })
        .collect()
}

/// Pastes files or an image from the clipboard into the host's composer attachments.
pub fn paste<S: ComposerHost>(s: &mut S, content: &ClipboardContent, _: &mut Tx) -> bool {
    if !content.files().is_empty() && s.composer_paste_files(content.files()) {
        return true;
    }
    let Some(img) = content.image() else { return false };
    let n = s.composer_attachments().len() + 1;
    s.composer_paste_attachment(Attachment {
        mime: "image/png".into(),
        name: format!("pasted-image-{n}.png"),
        bytes: img.png().into(),
        media: None,
    })
}

/// Deduplicating push helper for a host's `Vec<Attachment>`.
pub fn push_unique(list: &mut Vec<Attachment>, a: Attachment) {
    if !list.iter().any(|f| f.bytes == a.bytes && f.media == a.media) {
        list.push(a);
    }
}

/// Settles local prompt echoes against freshly fetched server messages.
pub fn reconcile(echoes: &mut Vec<(String, String, String)>, t: &mut Transcript, fresh: &[Message]) {
    echoes.retain(|(session, id, body)| {
        if *session != t.session {
            return true;
        }
        if fresh.iter().any(|m| m.id() == id) {
            return false;
        }
        let known = |mid: &str| t.messages.iter().any(|m| m.id() == mid);
        let twin = fresh.iter().any(|m| {
            matches!(m, Message::User { id: mid, text } if mid != id && text.trim() == body && !known(mid))
        });
        if twin {
            t.messages.retain(|m| m.id() != id);
        }
        !twin
    });
}

/// Renders the composer's attachment chips: quote cards, `58×46` image thumbnails, `160px` file
/// cards, and in-progress uploads, each with a round hover remove button.
pub fn chips<S: ComposerHost>(s: &S, p: &Pal) -> Option<El> {
    let quotes = s.composer_quote_chips(p);
    let files = s.composer_attachments();
    let paths = s.composer_path_attachments();
    let uploading = s.composer_uploading();
    if quotes.is_empty() && files.is_empty() && paths.is_empty() && uploading.is_empty() {
        return None;
    }
    let mut r = row().key("attachments-strip").px(8.0).pt(8.0).pb(4.0).gap(8.0).children(quotes);
    for f in files {
        let id = identity(f);
        let img = s.media_store().image(f);
        let w = if img.is_some() { 58.0 } else { 160.0 };
        let body = match img {
            Some(img) => {
                let thumb = el()
                    .key("attachment-thumb")
                    .size(58.0, 46.0)
                    .radius(6.0)
                    .fill(p.layer2)
                    .pixels_fit(img.pixels(), SurfaceFit::Fill)
                    .child(
                        el()
                            .key("thumb-ring")
                            .absolute(0.0, 0.0)
                            .size(58.0, 46.0)
                            .radius(6.0)
                            .box_shadow(Pal::ring_in(p.border)),
                    );
                opens(s, thumb, el().key("held").size(58.0, 46.0), format!("composer:{id}"), f, img, None)
            }
            None => card(p, &f.name, type_label(&f.name)),
        };
        let gone = id.clone();
        let remove = remove_button(p, &f.name, w, move |s: &mut S| s.composer_remove_attachment(&gone));
        r = r.child(el().key(format!("chip-{id}")).hover_group().child(body).child(remove));
    }
    for f in paths {
        let gone = f.path.clone();
        let remove = remove_button(p, &f.name, 160.0, move |s: &mut S| s.composer_remove_path_attachment(&gone));
        r = r.child(
            el()
                .key(format!("path-{}", f.path))
                .hover_group()
                .child(card(p, &f.name, type_label(&f.name)))
                .child(remove),
        );
    }
    // An upload's card and its cancel (`editor.tsx:880-899`).
    for (id, name) in uploading {
        let gone = id.clone();
        let cancel = remove_button(p, &name, 160.0, move |s: &mut S| s.composer_cancel_upload(&gone)).label(opencode_i18n::t!("ui.promptInput.cancelUpload"));
        r = r.child(el().key(format!("uploading-{id}")).hover_group().child(card(p, &name, opencode_i18n::t!("rune.composer.uploading").into())).child(cancel));
    }
    // One row that scrolls sideways with no bar (`editor.tsx:758-910`), and a 24 pt bg-base
    // fade on each side that shows while chips hide under it (`attachments.css`).
    let fade = |key: &str, side: &'static std::thread::LocalKey<Live<f32>>, angle: f32| {
        let (left, right) = if angle > 0.0 { (Some(0.0), None) } else { (None, Some(0.0)) };
        side.with(|o| {
            el().key(key.to_owned())
                .pin(left, Some(0.0), right, Some(0.0))
                .w(24.0)
                .linear_gradient(angle, [(0.0, p.bg_base), (1.0, p.bg_base.alpha(0.0))])
                .live_opacity(o)
                .inert()
        })
    };
    let strip = el()
        .key("attachments-scroll")
        .w_full()
        .scroll_x()
        .on_scroll_offset(|at: ScrollPos| {
            FADE_LEFT.with(|o| o.set(if at.offset[0] > 0.1 { 1.0 } else { 0.0 }));
            FADE_RIGHT.with(|o| o.set(if at.max[0] > 0.0 && at.offset[0] < at.max[0] - 1.0 { 1.0 } else { 0.0 }));
        })
        .child(r);
    Some(
        el().key("attachments")
            .w_full()
            // The fades keep to the composer's rounded top corners.
            .clip_corners(if s.composer_docked_top() { [0.0; 4] } else { [12.0, 12.0, 0.0, 0.0] })
            .child(strip)
            .child(fade("attachments-fade-left", &FADE_LEFT, std::f32::consts::FRAC_PI_2))
            .child(fade("attachments-fade-right", &FADE_RIGHT, -std::f32::consts::FRAC_PI_2)),
    )
}

thread_local! {
    /// Whether chips hide under the strip's left and right edges: one composer shows at a time.
    static FADE_LEFT: Live<f32> = Live::new(0.0);
    static FADE_RIGHT: Live<f32> = Live::new(0.0);
}

/// The round button on a chip's corner that shows on hover, for a chip `w` wide.
pub fn remove_button<S: 'static>(
    p: &Pal,
    _name: &str,
    w: f32,
    on: impl Fn(&mut S) + 'static,
) -> El {
    el()
        .key("remove")
        .role("button")
        .label(opencode_i18n::t!("ui.promptInput.removeAttachment"))
        .absolute(w - 12.0, -4.0)
        .size(16.0, 16.0)
        .radius(8.0)
        .center()
        .fill(p.icon_muted)
        .outline(1.0, 0.0, p.icon_contrast)
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            on(s);
        })
        .child(icon(&ic::XMARK_SMALL, 12.0).color(p.icon_contrast))
}

/// `AttachmentCard` (`attachment-card.css`): 160 wide, 8 in, its title over its subtitle 6
/// apart (the subtitle's line at least 16 tall), on the hover overlay with a 0.5 hairline in
/// `border-base`.
pub fn card(p: &Pal, title: &str, subtitle: String) -> El {
    col()
        .key("card")
        .w(160.0)
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(p.hover)
        .box_shadow(Pal::ring_in(p.border))
        .child(text(title.to_owned()).font(CARD_TITLE).color(p.text).truncate_fade().w_full())
        .child(row().key("subtitle").w_full().min_h(16.0).center_y().child(text(subtitle).font(CARD_SUBTITLE).color(p.muted).truncate_fade()))
}

/// A file sent as context without a mention (`editor.tsx:802-830`): the card on the base
/// surface (the hover overlay in dark, `attachment-card.css`), its subtitle the file's 12 pt
/// icon, then its type and lines (`TypeScript:12-20`).
pub fn context_card(p: &Pal, title: &str, icon: El, label: String) -> El {
    col()
        .key("card")
        .w(160.0)
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(if p.dark { p.hover } else { p.bg_base })
        .box_shadow(Pal::ring_in(p.border))
        .child(text(title.to_owned()).font(CARD_TITLE).color(p.text).truncate_fade().w_full())
        .child(row().h(16.0).gap(4.0).center_y().child(icon).child(text(label).font(CARD_SUBTITLE).color(p.muted).truncate_fade().shrink().min_w(0.0)))
}

/// Desktop's `typeLabel` (`message-file.ts`): a source file's language by its extension, any
/// other extension in capitals, and File without one (a dotfile has none).
pub fn type_label(name: &str) -> String {
    let base = opencode_kit::path::basename(name);
    let ext = match base.rfind('.') {
        Some(i) if i > 0 => base[i + 1..].to_ascii_lowercase(),
        _ => return opencode_i18n::t!("ui.common.file").into(),
    };
    if ext == "pdf" {
        return "PDF".into();
    }
    LANGUAGES.iter().find(|(_, exts)| exts.contains(&ext.as_str())).map_or_else(|| ext.to_uppercase(), |(lang, _)| (*lang).to_owned())
}

const LANGUAGES: &[(&str, &[&str])] = &[
    ("Assembly", &["asm", "s"]),
    ("Astro", &["astro"]),
    ("Batch File", &["bat", "cmd"]),
    ("C", &["c", "h"]),
    ("C#", &["cs"]),
    ("C++", &["cpp", "cc", "cxx", "hpp", "hh", "hxx"]),
    ("Clojure", &["clj", "cljs", "cljc", "edn"]),
    ("CSS", &["css"]),
    ("CUDA", &["cu", "cuh"]),
    ("Dart", &["dart"]),
    ("Diff", &["diff", "patch"]),
    ("Elixir", &["ex", "exs"]),
    ("Elm", &["elm"]),
    ("Erlang", &["erl", "hrl"]),
    ("F#", &["fs", "fsi", "fsx"]),
    ("Fortran", &["f", "f77", "f90", "f95", "f03", "f08", "for"]),
    ("GDScript", &["gd"]),
    ("Gleam", &["gleam"]),
    ("GLSL", &["glsl", "vert", "frag", "geom", "comp"]),
    ("Go", &["go"]),
    ("GraphQL", &["graphql", "gql"]),
    ("Groovy", &["groovy", "gradle"]),
    ("Haskell", &["hs"]),
    ("HCL", &["hcl"]),
    ("HLSL", &["hlsl"]),
    ("HTML", &["html", "htm"]),
    ("Java", &["java"]),
    ("JavaScript", &["js", "cjs", "mjs"]),
    ("JSON", &["json"]),
    ("JSON Lines", &["jsonl"]),
    ("JSON with Comments", &["jsonc"]),
    ("JSON5", &["json5"]),
    ("JSX", &["jsx"]),
    ("Julia", &["jl"]),
    ("Kotlin", &["kt", "kts"]),
    ("LaTeX", &["tex", "latex"]),
    ("Less", &["less"]),
    ("Lua", &["lua"]),
    ("Markdown", &["md", "markdown"]),
    ("MDX", &["mdx"]),
    ("Nim", &["nim"]),
    ("Nix", &["nix"]),
    ("OCaml", &["ml", "mli"]),
    ("Perl", &["pl", "pm"]),
    ("PHP", &["php"]),
    ("PowerShell", &["ps1", "psm1"]),
    ("Protocol Buffer", &["proto"]),
    ("Python", &["py", "pyi"]),
    ("R", &["r"]),
    ("Ruby", &["rb"]),
    ("Rust", &["rs"]),
    ("Sass", &["sass"]),
    ("Scala", &["scala", "sbt"]),
    ("SCSS", &["scss"]),
    ("Shell", &["sh", "bash", "zsh"]),
    ("Solidity", &["sol"]),
    ("SQL", &["sql"]),
    ("Svelte", &["svelte"]),
    ("SVG", &["svg"]),
    ("Swift", &["swift"]),
    ("Terraform", &["tf", "tfvars"]),
    ("TOML", &["toml"]),
    ("TSX", &["tsx"]),
    ("TypeScript", &["ts", "cts", "mts"]),
    ("Vue", &["vue"]),
    ("WebAssembly", &["wat", "wasm"]),
    ("WGSL", &["wgsl"]),
    ("XML", &["xml"]),
    ("YAML", &["yaml", "yml"]),
    ("Zig", &["zig"]),
];
