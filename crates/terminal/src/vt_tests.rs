use std::sync::Arc;

use crate::{
    CellFlags, CursorShape, GhosttyKey, GridPoint, KeyInput, MouseBtn, MouseInput, MouseKind, Rgb,
    ScrollOp, SelectionOp, TerminalTheme, UnderlineKind, VtCore, VtEvent,
};

#[test]
fn vttest_cursor_wrap_scroll_regions_and_alt_screen() {
    let mut vt = VtCore::new(10, 5, 100, TerminalTheme::opencode_dark()).unwrap();

    // Pending wrap at right margin (column 10): writing 10 chars leaves cursor at col 9 with
    // pending wrap, not wrapped onto row 1 until the 11th printable char arrives.
    vt.feed(b"0123456789");
    let snap = vt.snapshot();
    assert_eq!(snap.line_text(0), "0123456789");
    assert_eq!(snap.line_text(1), "");
    assert_eq!((snap.cursor.col, snap.cursor.row), (9, 0));

    vt.feed(b"X");
    let snap = vt.snapshot();
    assert_eq!(snap.line_text(0), "0123456789");
    assert!(snap.screen_rows[0].wrapped, "row 0 should record soft-wrap");
    assert_eq!(snap.line_text(1), "X");
    assert_eq!((snap.cursor.col, snap.cursor.row), (1, 1));

    // DECSTBM top/bottom scroll region (rows 2..=4 in 1-based CSI r) + LF scrolling inside region
    // while row 1 ("HEADER") and row 5 ("FOOTER") stay pinned.
    vt.feed(b"\x1bc"); // RIS full reset
    vt.feed(b"HEADER\r\nR1\r\nR2\r\nR3\r\nFOOTER");
    vt.feed(b"\x1b[2;4r\x1b[4;1H\nNEW");
    let snap = vt.snapshot();
    assert_eq!(snap.line_text(0), "HEADER");
    assert_eq!(snap.line_text(1), "R2");
    assert_eq!(snap.line_text(2), "R3");
    assert_eq!(snap.line_text(3), "NEW");
    assert_eq!(snap.line_text(4), "FOOTER");

    // Alternate screen (?1049h / ?1049l) isolates full-screen apps and restores primary screen.
    vt.feed(b"\x1b[?1049h\x1b[HALT-TUI");
    let alt = vt.snapshot();
    assert!(alt.alt_screen);
    assert_eq!(alt.line_text(0), "ALT-TUI");

    vt.feed(b"\x1b[?1049l");
    let back = vt.snapshot();
    assert!(!back.alt_screen);
    assert_eq!(back.line_text(0), "HEADER");
    assert_eq!(back.line_text(4), "FOOTER");
}

#[test]
fn sgr_ansi_256_truecolor_underlines_and_inverse() {
    let theme = TerminalTheme::opencode_dark();
    let mut vt = VtCore::new(40, 4, 100, theme.clone()).unwrap();

    // Bold ANSI red + 256-color bg (208) + 24-bit RGB fg + curly colored underline + inverse.
    vt.feed(
        b"\x1b[1;31mR\x1b[0m\
          \x1b[48;5;208mB\x1b[0m\
          \x1b[38;2;18;210;99;3;9mT\x1b[0m\
          \x1b[4:3;58:2::255:64:128mU\x1b[0m\
          \x1b[7mI\x1b[0m",
    );
    let snap = vt.snapshot();
    let row = &snap.screen_rows[0];

    // Cell 0: 'R', bold, ANSI red (palette[1])
    assert_eq!(row.cells[0].grapheme, "R");
    assert!(row.cells[0].flags.contains(CellFlags::BOLD));
    assert_eq!(row.cells[0].fg, theme.ansi_base[1]);
    assert_eq!(row.cells[0].bg, None);

    // Cell 1: 'B', 256-color bg
    assert_eq!(row.cells[1].grapheme, "B");
    assert!(row.cells[1].bg.is_some());

    // Cell 2: 'T', truecolor RGB + italic + strikethrough
    assert_eq!(row.cells[2].grapheme, "T");
    assert_eq!(row.cells[2].fg, Rgb::new(18, 210, 99));
    assert!(row.cells[2].flags.contains(CellFlags::ITALIC));
    assert!(row.cells[2].flags.contains(CellFlags::STRIKETHROUGH));

    // Cell 3: 'U', curly underline + RGB underline color
    assert_eq!(row.cells[3].grapheme, "U");
    assert_eq!(row.cells[3].underline, UnderlineKind::Curly);
    assert_eq!(row.cells[3].underline_color, Some(Rgb::new(255, 64, 128)));

    // Cell 4: 'I', inverse swaps fg and default bg
    assert_eq!(row.cells[4].grapheme, "I");
    assert!(row.cells[4].flags.contains(CellFlags::INVERSE));
    assert_eq!(row.cells[4].fg, theme.bg);
    assert_eq!(row.cells[4].bg, Some(theme.fg));
}

#[test]
fn wide_cjk_and_mode_2027_zwj_emoji_grapheme_clusters() {
    let mut vt = VtCore::new(20, 4, 100, TerminalTheme::opencode_dark()).unwrap();

    // Wide CJK characters occupy 2 columns (lead cell width=2 + spacer tail width=0).
    // Family ZWJ emoji + skin-tone emoji + combining mark stay intact as single graphemes.
    let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
    let wave = "👋🏽";
    let accent = "e\u{0301}";
    let stream = format!("日A{family}{wave}{accent}");
    vt.feed(stream.as_bytes());

    let snap = vt.snapshot();
    let c = &snap.screen_rows[0].cells;

    // Col 0..1: '日' (wide)
    assert_eq!(c[0].grapheme, "日");
    assert_eq!(c[0].width, 2);
    assert!(c[1].flags.contains(CellFlags::WIDE_SPACER_TAIL));
    assert_eq!(c[1].width, 0);

    // Col 2: 'A' (narrow)
    assert_eq!(c[2].grapheme, "A");
    assert_eq!(c[2].width, 1);

    // Col 3..4: ZWJ family emoji stays together in one multi-codepoint cell!
    assert_eq!(c[3].grapheme, family);
    assert_eq!(c[3].width, 2);
    assert!(c[4].flags.contains(CellFlags::WIDE_SPACER_TAIL));

    // Col 5..6: skin-tone wave emoji stays together in one cell!
    assert_eq!(c[5].grapheme, wave);
    assert_eq!(c[5].width, 2);
    assert!(c[6].flags.contains(CellFlags::WIDE_SPACER_TAIL));

    // Col 7: 'e' + combining acute accent stays together in one narrow cell!
    assert_eq!(c[7].grapheme, accent);
    assert_eq!(c[7].width, 1);
}

#[test]
fn row_ids_persist_across_scrollback_and_clean_rows_reuse_allocations() {
    let mut vt = VtCore::new(40, 4, 500, TerminalTheme::opencode_dark()).unwrap();
    for i in 0..20 {
        vt.feed(format!("line-{i:03}\r\n").as_bytes());
    }
    vt.feed(b"prompt$ ");
    let bottom = vt.snapshot();
    assert!(bottom.scrollbar.at_bottom);
    assert_eq!(bottom.line_text(0), "line-017");
    assert_eq!(bottom.line_text(1), "line-018");
    assert_eq!(bottom.line_text(2), "line-019");
    assert_eq!(bottom.line_text(3), "prompt$");

    let row_18_ptr = bottom.screen_rows[1].clone();
    let row_19_ptr = bottom.screen_rows[2].clone();

    // Partial screen update on row 3 only: rows 0..3 are clean and must reuse exact Arc pointers.
    vt.feed(b"ls");
    let partial = vt.snapshot();
    assert_eq!(partial.line_text(3), "prompt$ ls");
    assert!(Arc::ptr_eq(&partial.screen_rows[1], &row_18_ptr));
    assert!(Arc::ptr_eq(&partial.screen_rows[2], &row_19_ptr));

    // Scroll up by 1 line: `line-018` moves from viewport row 1 to viewport row 2 with the SAME
    // RowId and reuses its cached `Arc<TerminalRow>` without rebuilding!
    vt.scroll(ScrollOp::Delta(-1));
    let scrolled = vt.snapshot();
    assert!(!scrolled.scrollbar.at_bottom);
    assert_eq!(scrolled.line_text(2), "line-018");
    assert_eq!(scrolled.screen_rows[2].id, row_18_ptr.id);
    assert!(Arc::ptr_eq(&scrolled.screen_rows[2], &row_18_ptr));
}

#[test]
fn resize_reflows_wrapped_lines_and_selection_extracts_text() {
    let mut vt = VtCore::new(20, 5, 100, TerminalTheme::opencode_dark()).unwrap();
    vt.feed(b"abcdefghij0123456789\r\nhello rust_world_id!");
    let wide = vt.snapshot();
    assert_eq!(wide.line_text(0), "abcdefghij0123456789");

    // Shrink width to 10 cols: the 20-char line reflows into two 10-char rows.
    vt.resize(10, 5, 8, 18);
    let narrow = vt.snapshot();
    assert_eq!(narrow.line_text(0), "abcdefghij");
    assert_eq!(narrow.line_text(1), "0123456789");

    // Grow back to 25 cols and test word selection on "rust_world_id".
    vt.resize(25, 5, 8, 18);
    let grown = vt.snapshot();
    let target_row = grown
        .screen_rows
        .iter()
        .position(|r| r.text.contains("rust_world_id"))
        .expect("reflowed line present") as u16;

    vt.select(SelectionOp::Word(GridPoint::Viewport {
        col: 8,
        row: target_row,
    }));
    assert_eq!(vt.selected_text().as_deref(), Some("rust_world_id"));
    let with_sel = vt.snapshot();
    assert!(with_sel.screen_rows[usize::from(target_row)].cells[6].selected);

    vt.select(SelectionOp::Clear);
    assert_eq!(vt.selected_text(), None);
}

#[test]
fn kitty_keyboard_mouse_paste_effects_and_vt_serialize_roundtrip() {
    let mut vt = VtCore::new(60, 8, 200, TerminalTheme::opencode_dark()).unwrap();

    // Desktop override: ⌘Backspace -> \x15 (Ctrl+U kill-line).
    let mut cmd_bs = KeyInput::press(GhosttyKey::Backspace);
    cmd_bs.cmd = true;
    assert_eq!(vt.encode_key(&cmd_bs), b"\x15");

    // Legacy Escape vs Kitty disambiguate mode (`CSI > 1 u`).
    assert_eq!(vt.encode_key(&KeyInput::press(GhosttyKey::Escape)), b"\x1b");
    vt.feed(b"\x1b[>1u");
    assert_eq!(vt.encode_key(&KeyInput::press(GhosttyKey::Escape)), b"\x1b[27u");
    vt.feed(b"\x1b[<u");

    // Bracketed paste mode (?2004h).
    assert_eq!(vt.encode_paste("echo hi"), b"echo hi");
    vt.feed(b"\x1b[?2004h");
    assert_eq!(vt.encode_paste("echo hi"), b"\x1b[200~echo hi\x1b[201~");

    // SGR mouse encoding (?1000h?1006h).
    vt.resize(60, 8, 10, 20);
    vt.feed(b"\x1b[?1000h\x1b[?1006h");
    let m = vt.encode_mouse(MouseInput {
        kind: MouseKind::Press,
        button: MouseBtn::Left,
        x_px: 25.0,
        y_px: 45.0,
        shift: false,
        ctrl: false,
        alt: false,
        cmd: false,
    });
    assert_eq!(m, b"\x1b[<0;3;3M");

    // OSC 2 title, OSC 7 pwd, visual bell, DECSCUSR block cursor, OSC 8 hyperlink, CSI ?996n query.
    vt.feed(b"\x1b]2;cargo test\x07\x1b]7;file://localhost/workspace/demo\x07\x07\x1b[2 q");
    vt.feed(b"\x1b]8;;https://opencode.ai/docs\x07docs\x1b]8;;\x07");
    vt.feed(b"\x1b[?996n");

    let replies = vt.take_pty_writes();
    assert_eq!(replies, b"\x1b[?997;1n", "dark scheme reply to ?996n");

    let evs = vt.take_events();
    assert!(evs.contains(&VtEvent::Bell));
    assert!(evs.contains(&VtEvent::TitleChanged("cargo test".into())));
    assert_eq!(vt.hyperlink_at(0, 0).as_deref(), Some("https://opencode.ai/docs"));

    let snap = vt.snapshot();
    assert_eq!(snap.title, "cargo test");
    assert_eq!(snap.bell_count, 1);
    assert_eq!(snap.cursor.shape, CursorShape::Block);

    // Serialize VT state and restore into a fresh VtCore: text & colors survive!
    vt.feed(b"\r\n\x1b[32mPASS\x1b[0m all green");
    let saved = vt.serialize_vt();
    assert!(!saved.is_empty());

    let mut restored = VtCore::new(60, 8, 200, TerminalTheme::opencode_dark()).unwrap();
    restored.feed(&saved);
    let rsnap = restored.snapshot();
    assert!(rsnap.viewport_text().contains("PASS all green"));
    let pass_row = rsnap
        .screen_rows
        .iter()
        .find(|r| r.text.contains("PASS"))
        .unwrap();
    assert_eq!(pass_row.cells[0].fg, TerminalTheme::opencode_dark().ansi_base[2]);
}

#[test]
fn serialize_vt_capped_truncates_old_scrollback_at_row_limit() {
    let mut vt = VtCore::new(60, 10, 5_000, TerminalTheme::opencode_dark()).unwrap();
    let mut big = Vec::with_capacity(120_000);
    for i in 0..3_000 {
        big.extend_from_slice(format!("row-{i:04} \x1b[36mpayload\x1b[0m\r\n").as_bytes());
    }
    vt.feed(&big);
    let _ = vt.snapshot();

    let capped = vt.serialize_vt_capped(2_000);
    let text = String::from_utf8_lossy(&capped);
    assert!(
        !text.contains("row-0050"),
        "ancient scrollback rows before 2,000-row cap should be pruned"
    );
    assert!(
        text.contains("row-2999"),
        "recent rows and viewport must be preserved"
    );

    let mut restored = VtCore::new(60, 10, 5_000, TerminalTheme::opencode_dark()).unwrap();
    restored.feed(&capped);
    assert!(restored.snapshot().viewport_text().contains("row-2999"));
}
