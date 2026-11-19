//! Desktop host adapter for `opencode-feature-office` (`features/office`).

pub use opencode_feature_office::{
    OfficeCompositor, OfficeEnv, configure, pages, poll, sheet,
};

use opencode_kernel::{Feature, KernelBuilder};

use crate::App;

/// Desktop adapter around `opencode_feature_office::registry` that bridges `open` to
/// `rune_luau::{Completer, Data}` so `opencode-feature-office` stays free of `rune-luau`.
pub mod registry {
    use office_preview::Kind;
    pub use opencode_feature_office::registry::*;
    use rune_luau::{Completer, Data};

    fn map(fields: impl IntoIterator<Item = (&'static str, Data)>) -> Data {
        Data::Map(
            fields
                .into_iter()
                .filter(|(_, v)| *v != Data::Nil)
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    fn doc_data(doc: OpenedDocument) -> Data {
        let kind = match doc.kind {
            Kind::Document => "docx",
            Kind::Spreadsheet => "xlsx",
            Kind::Presentation => "pptx",
        };
        map([
            ("id", Data::Integer(doc.id as i64)),
            ("kind", Data::String(kind.into())),
            ("pages", Data::Integer(doc.pages as i64)),
            ("sheets", Data::List(doc.sheets.into_iter().map(Data::String).collect())),
        ])
    }

    pub fn open(kind: Kind, bytes: Vec<u8>, size: usize, waker: Option<rune::Waker>, done: Completer) {
        opencode_feature_office::registry::open(kind, bytes, size, waker, move |ans| {
            done.complete(map([
                ("size", Data::Integer(ans.size as i64)),
                ("signature", Data::String("zip".into())),
                ("document", ans.document.map_or(Data::Nil, doc_data)),
                ("error", ans.error.map_or(Data::Nil, Data::String)),
            ]));
        });
    }
}

impl opencode_feature_office::OfficeHost for App {}

/// Wires desktop's cache directory and wall clock into the Office feature crate once at startup.
pub fn init() {
    configure(OfficeEnv {
        cache_dir: crate::dirs::cache,
        wall_ms: Some(crate::wall_ms),
    });
}

/// Kernel feature registration for Office document previews.
pub struct OfficeFeature;

impl Feature<App> for OfficeFeature {
    fn id(&self) -> &'static str {
        "feature.office"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // Precaution: both `updater.poll` and `office.poll` can push toasts via `App::toast`.
        builder
            .schedule(crate::tick::stage::INGEST_LOCAL, "office.poll", |s, _, tx| poll(s, tx))
            .after("updater.poll");
    }
}
