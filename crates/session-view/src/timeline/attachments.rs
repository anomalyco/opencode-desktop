//! A sent prompt's attachments, over its bubble, as Desktop's user message draws them
//! (`session-ui/src/message/message-content.tsx:505-648`, `message-part.css:26-125`,
//! `attachment-card.css`): first the files it named by path (`metadata.attachments`) as cards,
//! then its own files: an image as a 132×88 thumbnail that opens the viewer, anything else as a
//! card that shows the file. 8 apart, wrapping, flush right in the bubble's `min(82%, 64ch)`.

use std::path::Path;

use opencode_kit::model::{self, Attachment};
use rune::{El, FileDialogOptions, Icon, SurfaceFit, Tx, el, icon, row};

use crate::TimelineHost;
use crate::composer::attachments::{card, type_label};
use crate::images::{Fetched, gallery_id, opens};
use crate::palette::Pal;
use crate::style::BUBBLE_MAX;

/// All image states share one footprint (OpenCode 26f2522e60, `message-part.css`, `message-content.tsx`).
const THUMB: (f32, f32) = (132.0, 88.0);

/// `outline-photo` (`icons/catalog.ts`), which a failed image shows.
static OUTLINE_PHOTO: Icon = Icon {
    name: "outline-photo",
    view: 14.0,
    body: r#"<path d="M11.6666 11.6666L8.16663 8.16663L6.99997 9.33332L4.66663 6.99998L2.1583 9.50832M2.04163 2.04165H11.9583V11.9583H2.04163V2.04165ZM9.33332 5.24998C9.33332 5.81377 8.87629 6.27081 8.31244 6.27081C7.74866 6.27081 7.29163 5.81377 7.29163 5.24998C7.29163 4.68619 7.74866 4.22915 8.31244 4.22915C8.87629 4.22915 9.33332 4.68619 9.33332 5.24998Z" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// Whether prompt `id` has anything [`view`] draws.
pub fn any(id: &str) -> bool {
    !model::user_files(id).is_empty() || !opencode_kit::mention::user_path_attachments(id).is_empty()
}

pub fn view<S: TimelineHost>(s: &S, p: &Pal, id: &str) -> Option<El> {
    let files = model::user_files(id);
    let refs = opencode_kit::mention::user_path_attachments(id);
    if files.is_empty() && refs.is_empty() {
        return None;
    }
    let mut items = row().key("attachments").w_frac(0.82).max_w(BUBBLE_MAX).gap(8.0).flex_wrap().justify_end().items_start();
    // A file named by path shows as its card, its path on hover; the server reads it.
    for (i, r) in refs.iter().enumerate() {
        items = items.child(card(p, &r.name, type_label(&r.name)).key(format!("ref-{i}")).help(r.path.clone()));
    }
    for (i, f) in files.iter().enumerate() {
        items = items.child(if f.is_image() { thumb(s, p, id, i, f) } else { file_card::<S>(p, i, f) });
    }
    Some(row().w_full().justify_end().child(items))
}

/// An image: its picture cropped to fill, with a 0.5 hairline over it, opening the viewer.
/// Still loading it holds the same space; failed, the photo glyph in it, and a press
/// asks again (`attachment-media-machine.ts`, `message-file.ts`).
fn thumb<S: TimelineHost>(s: &S, p: &Pal, message: &str, i: usize, f: &Attachment) -> El {
    let store = s.media_store();
    let key = format!("file-{i}");
    let image = store.image(f).filter(|img| img.error().is_none());
    let (w, h) = THUMB;
    let ring = el().key("ring").absolute(0.0, 0.0).size(w, h).radius(6.0).box_shadow(Pal::ring_in(p.border)).inert();
    let tile = el().key(key.clone()).label(f.name.clone()).size(w, h).radius(6.0).fill(p.layer2);
    match image {
        Some(img) => {
            let thumb = tile.pixels_fit(img.pixels(), SurfaceFit::Fill).child(ring);
            let held = el().key(format!("{key}-held")).size(w, h);
            opens(s, thumb, held, gallery_id(&format!("user-attachments:{message}"), i), f, img, Some(message))
        }
        None if store.fetched(f) == Some(Fetched::Loading) => tile.child(ring),
        None => {
            let retry = f.clone();
            tile.role("button")
                .label(opencode_i18n::t!("ui.fileMedia.state.error", kind = opencode_i18n::t!("ui.fileMedia.kind.image")))
                .cursor(rune::Cursor::Pointer)
                .center()
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    s.media_store().retry(&retry);
                })
                .child(icon(&OUTLINE_PHOTO, 20.0).color(p.icon_faint))
                .child(ring)
        }
    }
}

/// Anything else: its card, titled by its file name with its type under it, that shows the
/// file where it is, or saves it when it has no path here (`region.tsx` `openAttachment`).
fn file_card<S: TimelineHost>(p: &Pal, i: usize, f: &Attachment) -> El {
    let name = opencode_kit::path::basename(&f.name).to_owned();
    let label = if f.mime == "application/pdf" { "PDF".to_owned() } else { type_label(&f.name) };
    let file = f.clone();
    card(p, &name, label)
        .key(format!("file-{i}"))
        .role("button")
        .focusable(true)
        .cursor(rune::Cursor::Pointer)
        .help(f.name.clone())
        .on_press_tx(move |s: &mut S, tx: &mut Tx| open(s, &file, tx))
}

fn open<S: TimelineHost>(s: &mut S, f: &Attachment, tx: &mut Tx) {
    let path = Path::new(&f.name);
    if path.is_absolute() && s.reveal_file(path) {
        return;
    }
    let Some(bytes) = s.media_store().bytes(f) else { return };
    let name = opencode_kit::path::basename(&f.name);
    let options = FileDialogOptions { file_name: Some(if name.is_empty() { "attachment".into() } else { name.to_owned() }), ..Default::default() };
    tx.save_file(options, move |_: &mut S, answer, _| {
        if let Ok(Some(path)) = answer
            && let Err(e) = std::fs::write(&path, &bytes)
        {
            eprintln!("save attachment: {e}");
        }
    });
}
