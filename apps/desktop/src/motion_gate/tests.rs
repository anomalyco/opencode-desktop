use super::*;

#[test]
fn another_runtime_cannot_change_a_cells_measured_columns() {
    job(|| {
        let r = Run::new(Circ { narrow: true, ..Circ::default() });
        let style = rune::TextStyle::new(14.0, 400.0).mono();
        let column = || rune::text::measure("settings/SettingsPage.tsx", &[], style, None);
        let before = column();
        let epoch = rune::text::font_epoch();
        // Another Desktop test starts a window with the launch fonts. Joining fixes the
        // interleaving: the next column measurement happens after that window registers them.
        std::thread::spawn(|| {
            let spec = crate::spec(App::empty);
            rune::text::TextSystem::new(spec.fonts, spec.families);
        }).join().unwrap();
        assert_eq!(column(), before, "a concurrent window must not resize the table's columns");
        assert_eq!(rune::text::font_epoch(), epoch, "the cell owns its measured fonts");
        r.gate.rec
    })();
}
