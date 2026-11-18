//! Recorded ANSI/VT byte streams for Storybook stages, tests, and the 100k-line scroll benchmark.

use std::fmt::Write;

pub const BUILD_STREAM: &[u8] = b"\
\x1b]2;zsh \xe2\x80\x94 seer\x07\
\x1b[1;34m~/code/seer\x1b[0m \x1b[38;5;244mon\x1b[0m \x1b[1;35m\xe2\x8e\x87 main\x1b[0m \x1b[1;32m\xe2\x9d\xaf\x1b[0m cargo test --locked -p terminal\r\n\
\x1b[1;32m   Compiling\x1b[0m terminal v0.0.1 (crates/terminal)\r\n\
\x1b[1;32m    Finished\x1b[0m `test` profile [optimized] target(s) in 0.42s\r\n\
\x1b[1;32m     Running\x1b[0m unittests src/lib.rs\r\n\r\n\
running 11 tests\r\n\
test abi_tests::rust_repr_c_structs_match_linked ... \x1b[32mok\x1b[0m\r\n\
test vt_tests::vttest_cursor_wrap_scroll_regions ... \x1b[32mok\x1b[0m\r\n\
test vt_tests::sgr_ansi_256_truecolor_underlines ... \x1b[32mok\x1b[0m\r\n\
test vt_tests::wide_cjk_and_zwj_emoji_graphemes ... \x1b[32mok\x1b[0m\r\n\
test vt_tests::row_ids_persist_across_scrollback ... \x1b[32mok\x1b[0m\r\n\
test pty_tests::server_pty_speaks_connect_token ... \x1b[32mok\x1b[0m\r\n\r\n\
test result: \x1b[1;32mok\x1b[0m. 11 passed; 0 failed; finished in 0.18s\r\n\
\x1b[2mDocs:\x1b[0m \x1b]8;;https://ghostty.org/docs\x07\x1b[4;38;2;97;175;239mhttps://ghostty.org/docs\x1b[0m\x1b]8;;\x07\r\n\r\n\
\x1b[1;34m~/code/seer\x1b[0m \x1b[38;5;244mon\x1b[0m \x1b[1;35m\xe2\x8e\x87 main\x1b[0m \x1b[1;32m\xe2\x9d\xaf\x1b[0m ";

pub const BUILD_BURST: &[u8] = b"git status -sb\r\n\
\x1b[1;32m##\x1b[0m main...origin/main\r\n\
\x1b[32m M\x1b[0m apps/desktop/src/terminal/grid.rs\r\n\
\x1b[1;34m~/code/seer\x1b[0m \x1b[38;5;244mon\x1b[0m \x1b[1;35m\xe2\x8e\x87 main\x1b[0m \x1b[1;32m\xe2\x9d\xaf\x1b[0m ";

pub const HTOP_STREAM: &[u8] = b"\
\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J\
\x1b[1;36mCPU1\x1b[0m [\x1b[32m|||||||||||\x1b[33m|||\x1b[0m    \x1b[1m62.4%\x1b[0m] \x1b[1;36mTasks:\x1b[0m 148, 3 run\r\n\
\x1b[1;36mCPU2\x1b[0m [\x1b[32m||||||||\x1b[33m||\x1b[0m        \x1b[1m41.0%\x1b[0m] \x1b[1;36mLoad:\x1b[0m  3.42 2.91\r\n\
\x1b[1;36mCPU3\x1b[0m [\x1b[32m|||||||||||||\x1b[31m||\x1b[0m   \x1b[1m74.8%\x1b[0m] \x1b[1;36mFrame:\x1b[0m \x1b[32m120 fps\x1b[0m\r\n\
\x1b[1;36mMem \x1b[0m [\x1b[32m||||||||||\x1b[34m||||\x1b[0m \x1b[1m18.4/36G\x1b[0m] \x1b[1;36mSwp:\x1b[0m   0.4/8.0G\r\n\
\x1b[38;5;240m\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\x1b[0m\r\n\
\x1b[30;42m  PID USER     RES S  CPU%   TIME+  COMMAND     \x1b[0m\r\n\
\x1b[7m48210 dev     184M R  38.2  2:14.08 opencode    \x1b[0m\r\n\
48214 dev      92M R  21.4  0:48.91 rust-analyz \r\n\
48102 dev      64M S   8.6  1:09.44 opencode-cli\r\n\
47990 dev      41M S   2.1  0:19.02 ghostty-vt  \r\n\
47812 dev      28M S   0.4  0:04.55 zsh -l      \r\n";

/// Partial screen update on `htop` rows 1 and 7 only (verifies per-row damage leaves rows 2..6 clean).
pub const HTOP_TICK_STREAM: &[u8] = b"\
\x1b[1;1H\x1b[1;36mCPU1\x1b[0m [\x1b[32m|||||||||||||\x1b[33m|||\x1b[0m  \x1b[1m71.9%\x1b[0m] \x1b[1;36mTasks:\x1b[0m 149, 4 run\
\x1b[7;1H\x1b[7m48210 dev     185M R  44.7  2:15.12 opencode    \x1b[0m";

pub const COLORS_UNICODE_STREAM: &[u8] = b"\
\x1b[1mGhostty VT 1.4 \xc2\xb7 SGR, Truecolor & Mode 2027\x1b[0m\r\n\r\n\
ANSI 16   \x1b[40m   \x1b[41m   \x1b[42m   \x1b[43m   \x1b[44m   \x1b[45m   \x1b[46m   \x1b[47m   \x1b[0m\r\n\
Bright    \x1b[100m   \x1b[101m   \x1b[102m   \x1b[103m   \x1b[104m   \x1b[105m   \x1b[106m   \x1b[107m   \x1b[0m\r\n\
Truecolor \x1b[48;2;255;95;109m  \x1b[48;2;255;145;110m  \x1b[48;2;255;195;113m  \x1b[48;2;140;215;144m  \x1b[48;2;86;182;194m  \x1b[48;2;97;175;239m  \x1b[48;2;198;120;221m  \x1b[0m\r\n\r\n\
Styles    \x1b[1mBold\x1b[0m \xc2\xb7 \x1b[3mItalic\x1b[0m \xc2\xb7 \x1b[2mFaint\x1b[0m \xc2\xb7 \x1b[9mStrike\x1b[0m \xc2\xb7 \x1b[4:3;58:2::240:113:120mCurly\x1b[0m\r\n\
CJK/Emoji \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e \xc2\xb7 \xe4\xb8\xad\xe6\x96\x87 \xc2\xb7 \xf0\x9f\x9a\x80 \xc2\xb7 \xf0\x9f\x91\xa8\xe2\x80\x8d\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x91\xa7\xe2\x80\x8d\xf0\x9f\x91\xa6 \xc2\xb7 \xf0\x9f\x91\x8b\xf0\x9f\x8f\xbd\r\n\
Box grid  \xe2\x94\x8c\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\xac\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x90\r\n\
\x20         \xe2\x94\x82 ok \xe2\x94\x82 0ms\xe2\x94\x82\r\n\
\x20         \xe2\x94\x94\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\xb4\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x98\r\n";

/// Generates `lines` of realistic colored compiler/test log output (`100_000` for the benchmark).
pub fn cat_lines_stream(lines: usize) -> Vec<u8> {
    let mut out = String::with_capacity(lines * 96);
    for i in 0..lines {
        let module = match i % 5 {
            0 => "crates/terminal/src/render_extract.rs",
            1 => "apps/desktop/src/terminal/grid.rs",
            2 => "crates/opencode-kit/src/pty.rs",
            3 => "apps/desktop/src/panels/dock.rs",
            _ => "crates/rune/src/text/glyph_cache.rs",
        };
        let ms = 1 + (i * 7) % 48;
        let _ = writeln!(
            &mut out,
            "\x1b[2m[{i:06}]\x1b[0m \x1b[32mPASS\x1b[0m \x1b[38;2;97;175;239m{module}\x1b[0m:\x1b[33m{}\x1b[0m ({ms}ms)\r",
            10 + (i % 420)
        );
    }
    out.into_bytes()
}
