//! `ui.officePages` and `ui.officeSheet`: the host's Office views (`crate::office`) for an
//! `OfficeDocument` the plugin opened with `Office:OpenAsync`.

use rune::{El, TextStyle, col, text};
use rune_luau::{Data, HostNode, NodeArgs};

use crate::office;
use crate::palette::{Pal, pal};
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::sources::builtin_message;

pub fn all() -> Vec<HostNode> {
    vec![HostNode::new("officePages", pages), HostNode::new("officeSheet", sheet)]
}

fn pages(a: &mut NodeArgs) -> Result<El, String> {
    let id = document(a)?;
    cx::read(Dep::Office(id));
    if let Some(reason) = office::registry::snapshot(id).and_then(|s| s.render_error) {
        return Ok(render_failed(pal(a.dark()), &reason));
    }
    let labels = a.data("labels").unwrap_or(Data::Nil);
    let field = |name: &str| match &labels {
        Data::Map(m) => m.get(name).and_then(|v| if let Data::String(s) = v { Some(s.clone()) } else { None }).unwrap_or_default(),
        _ => String::new(),
    };
    let labels = office::pages::Labels {
        page: field("page"),
        hidden: field("hiddenSlide"),
        failed: field("failed"),
        notes: field("speakerNotes"),
        zoom_in: opencode_i18n::t!("ui.preview.zoomIn").to_owned(),
        zoom_out: opencode_i18n::t!("ui.preview.zoomOut").to_owned(),
        fit: opencode_i18n::t!("ui.preview.fit").to_owned(),
        controls: opencode_i18n::t!("ui.preview.controls").to_owned(),
    };
    Ok(office::pages::view::<crate::App>(pal(a.dark()), id, &labels))
}

fn sheet(a: &mut NodeArgs) -> Result<El, String> {
    let id = document(a)?;
    cx::read(Dep::Office(id));
    if let Some(reason) = office::registry::snapshot(id).and_then(|s| s.render_error) {
        return Ok(render_failed(pal(a.dark()), &reason));
    }
    let labels = a.data("labels").unwrap_or(Data::Nil);
    let field = |name: &str| match &labels {
        Data::Map(m) => m.get(name).and_then(|v| if let Data::String(s) = v { Some(s.clone()) } else { None }).unwrap_or_default(),
        _ => String::new(),
    };
    let labels = office::sheet::Labels { name: field("name"), too_large: field("copyTooLarge"), failed: field("copyFailed") };
    Ok(office::sheet::view::<crate::App>(id, labels, pal(a.dark()).v2.border_focus))
}

fn render_failed(p: &Pal, reason: &str) -> El {
    let lang = opencode_i18n::locale().id();
    let title = builtin_message("microsoft-office", lang, "problem.openFailed").unwrap_or("This file could not be shown.");
    col()
        .key("office-render-failed")
        .role("status")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(24.0)
        .gap(8.0)
        .center()
        .fill(p.layer1)
        .child(text(title).font(TextStyle::new(13.0, 440.0).lh(24.0)).color(p.muted))
        .child(text(reason.to_owned()).font(TextStyle::new(12.0, 400.0).lh(18.0)).color(p.muted).wrap().wrap_anywhere().selectable(true).max_w(520.0))
}

/// The `document` prop's id.
fn document(a: &NodeArgs) -> Result<u64, String> {
    match a.data("document") {
        Some(Data::Map(m)) => match m.get("id") {
            Some(Data::Integer(id)) => u64::try_from(*id).map_err(|_| "document id".to_owned()),
            Some(Data::Number(id)) => Ok(*id as u64),
            _ => Err("document has no id".into()),
        },
        _ => Err("document is not an OfficeDocument".into()),
    }
}
