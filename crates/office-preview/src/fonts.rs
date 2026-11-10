//! The faces documents draw with, as Desktop's `fonts.ts` picks them: BetterOffice's bundled
//! faces, metric-compatible ones first (Carlito for Calibri, Caladea for Cambria, Liberation for
//! Arial, Times New Roman and Courier New), then the Office application's own fallbacks. The Latin
//! and international packs are compiled in. The Chinese, Japanese and Korean pack (27 MB) is not:
//! it is installed on first need (`cjk.rs`), and until then those scripts fall back to the Latin
//! faces, as Desktop's do (it ships no CJK faces at all).

use std::path::Path;
use std::sync::Arc;

use betteroffice_fonts::{BundledFontOptions, BundledFonts, FontFaceHandle, FontLoading, FontManager, FontPack, FontProvider, FontSource, LastResortOffice};

/// The bundled faces, the CJK pack's when it is installed.
#[derive(Clone)]
pub struct FontPacks {
    provider: Arc<Packs>,
}

struct Packs {
    compiled: BundledFonts,
    cjk: Option<BundledFonts>,
}

impl FontPacks {
    /// The compiled-in packs, with the CJK pack installed in `cjk` when it is complete there.
    pub fn new(cjk: Option<&Path>) -> FontPacks {
        let compiled = BundledFonts::new(BundledFontOptions { packs: vec![FontPack::Latin, FontPack::Intl], base_url: None }).unwrap_or_default();
        let cjk = cjk
            .and_then(|dir| BundledFonts::from_pack_directories(vec![(FontPack::Cjk, dir.to_path_buf())]).ok())
            .filter(|b| b.pack_diagnostics().is_empty() && b.bundled_packs().is_some_and(|p| p.contains(&FontPack::Cjk)));
        FontPacks { provider: Arc::new(Packs { compiled, cjk }) }
    }

    pub fn has_cjk(&self) -> bool {
        self.provider.cjk.is_some()
    }

    /// The source BetterOffice's engines open files with.
    pub fn source(&self) -> FontSource {
        FontSource::Provider(self.provider.clone())
    }

    /// The faces a run in `family` draws with, in fallback order: the family's own or its
    /// substitute, a face for each of `scripts` (`cjk-sc`, `arabic`...), the glyph fallback, and
    /// the application's last resort.
    pub fn resolve(&self, family: &str, bold: bool, italic: bool, scripts: &[String], office: LastResortOffice) -> Vec<FontFaceHandle> {
        FontManager::new(&self.source(), FontLoading::OnDemand).map(|m| m.resolve(family, bold, italic, scripts, office)).unwrap_or_default()
    }

    /// The face that covers `script` (`arabic`, `hebrew`, `cjk-sc`...), when a pack has one.
    pub fn script_face(&self, script: &str, bold: bool) -> Option<FontFaceHandle> {
        self.provider.resolve_script_fallback(script, bold, false)
    }

    /// The first face `resolve` gives, loaded.
    pub fn face(&self, family: &str, bold: bool, italic: bool, office: LastResortOffice) -> Option<(FontFaceHandle, Vec<u8>)> {
        self.resolve(family, bold, italic, &[], office).into_iter().find_map(|h| h.load().ok().map(|bytes| (h, bytes)))
    }
}

impl FontProvider for Packs {
    fn resolve_family(&self, family: &str, bold: bool, italic: bool) -> Option<FontFaceHandle> {
        self.compiled.resolve_family(family, bold, italic).or_else(|| self.cjk.as_ref()?.resolve_family(family, bold, italic))
    }

    fn resolve_script_fallback(&self, script: &str, bold: bool, italic: bool) -> Option<FontFaceHandle> {
        match &self.cjk {
            Some(cjk) if script.starts_with("cjk") => cjk.resolve_script_fallback(script, bold, italic),
            _ => self.compiled.resolve_script_fallback(script, bold, italic),
        }
    }

    fn resolve_family_script_fallback(&self, family: &str, script: &str, bold: bool, italic: bool) -> Option<FontFaceHandle> {
        match &self.cjk {
            Some(cjk) if script.starts_with("cjk") => cjk.resolve_family_script_fallback(family, script, bold, italic),
            _ => self.compiled.resolve_family_script_fallback(family, script, bold, italic),
        }
    }

    fn resolve_glyph_fallback(&self, family: &str, bold: bool, italic: bool, office: LastResortOffice) -> Option<FontFaceHandle> {
        self.compiled.resolve_glyph_fallback(family, bold, italic, office)
    }

    fn resolve_last_resort(&self, family: &str, bold: bool, italic: bool, office: LastResortOffice) -> FontFaceHandle {
        self.compiled.resolve_last_resort(family, bold, italic, office)
    }

    fn bundled_packs(&self) -> Option<Vec<FontPack>> {
        let mut packs = vec![FontPack::Latin, FontPack::Intl];
        if self.cjk.is_some() {
            packs.push(FontPack::Cjk);
        }
        Some(packs)
    }
}

/// Whether `text` holds Chinese, Japanese or Korean characters, which only the CJK pack draws.
pub fn cjk_text(text: &str) -> bool {
    text.chars().any(|c| matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F))
}
