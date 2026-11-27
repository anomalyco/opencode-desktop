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

/// `opencode-desktop --office-check [file]`: loads the office engines as a window would at its
/// first Word, Excel or PowerPoint file (their library, when the build ships them as one) and,
/// with a file, opens it; prints what came of it and how long it took, and exits (0: it worked).
/// No window. `smoke-app.sh` runs it on a packaged build, and it times a first open after an install.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)]
pub fn check_if_asked() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--office-check") {
        return;
    }
    let started = std::time::Instant::now();
    let ms = |since: std::time::Instant| since.elapsed().as_secs_f64() * 1000.0;
    let library = match opencode_feature_office::engines_library() {
        Ok(library) => library,
        Err(why) => {
            println!("office: {why}");
            std::process::exit(1);
        }
    };
    println!("office: the engines are {} ({:.1} ms)", if library { "in their library, loaded" } else { "in the exe" }, ms(started));
    let Some(path) = args.next() else { std::process::exit(0) };
    let (Some(kind), Ok(bytes)) = (office_preview::Kind::of(&path), std::fs::read(&path)) else {
        println!("office: {path} is not a readable .docx, .xlsx or .pptx file");
        std::process::exit(2);
    };
    let opening = std::time::Instant::now();
    let answer = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = answer.clone();
    let size = bytes.len();
    opencode_feature_office::registry::open(kind, bytes, size, None, move |a| *slot.borrow_mut() = Some(a));
    let answer = loop {
        opencode_feature_office::registry::poll();
        if let Some(answer) = answer.borrow_mut().take() {
            break answer;
        }
        if opening.elapsed() > std::time::Duration::from_secs(120) {
            println!("office: {path} gave no answer in 120 s");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    match (answer.document, answer.error) {
        (Some(document), _) => {
            println!("office: {path} opened: {} pages, {} sheets ({:.1} ms)", document.pages, document.sheets.len(), ms(opening));
            std::process::exit(0)
        }
        (None, error) => {
            println!("office: {path} did not open: {} ({:.1} ms)", error.unwrap_or_default(), ms(opening));
            std::process::exit(1)
        }
    }
}
/// Kernel feature registration for Office document previews.
pub struct OfficeFeature;

impl Feature<App> for OfficeFeature {
    fn id(&self) -> &'static str {
        "feature.office"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // Precaution: both `updater.poll` and `office.poll` can push toasts via `App::toast`.
        let office_poll = builder
            .schedule(crate::tick::stage::INGEST_LOCAL, "office.poll", |s, _, tx| poll(s, tx));
        #[cfg(not(target_arch = "wasm32"))]
        office_poll.after("updater.poll");
        #[cfg(target_arch = "wasm32")]
        let _ = office_poll;
    }
}
