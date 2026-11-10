//! `ui.officePages` and `ui.officeSheet`: the host's Office views (`crate::office`) for an
//! `OfficeDocument` the plugin opened with `Office:OpenAsync`.

use rune::El;
use rune_luau::{Data, HostNode, NodeArgs};

use super::cx::{self, Dep};
use crate::office;
use crate::palette::pal;

pub fn all() -> Vec<HostNode> {
    vec![HostNode::new("officePages", pages), HostNode::new("officeSheet", sheet)]
}

fn pages(a: &mut NodeArgs) -> Result<El, String> {
    let id = document(a)?;
    cx::read(Dep::Office(id));
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
    Ok(office::pages::view(pal(a.dark()), id, &labels))
}

fn sheet(a: &mut NodeArgs) -> Result<El, String> {
    let id = document(a)?;
    cx::read(Dep::Office(id));
    let labels = a.data("labels").unwrap_or(Data::Nil);
    let field = |name: &str| match &labels {
        Data::Map(m) => m.get(name).and_then(|v| if let Data::String(s) = v { Some(s.clone()) } else { None }).unwrap_or_default(),
        _ => String::new(),
    };
    let labels = office::sheet::Labels { name: field("name"), too_large: field("copyTooLarge"), failed: field("copyFailed") };
    Ok(office::sheet::view(id, labels, pal(a.dark()).v2.border_focus))
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
