//! 120 fps scroll benchmark over a 100,000-line `cat` buffer at Kit's zoom `1.6` (`2.0` backing scale).
//!
//! Run with:
//! `cargo test --locked --profile perf -p opencode-desktop scroll_100k_cat_benchmark -- --ignored --nocapture --test-threads=1`

use rune::{Env, Runtime};
use terminal::{RestoreState, ScrollOp, TerminalConfig};

use super::grid::{cell_metrics, terminal_font_style};
use super::story::view_card;
use super::streams::cat_lines_stream;
use super::theme_from_pal;
use crate::palette::pal;
use crate::{App, view};

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn report_metric(name: &str, values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
    println!(
        "METRIC name={name} p50={:.4} p95={:.4} p99={:.4} max={:.4} n={}",
        percentile(values, 0.50),
        percentile(values, 0.95),
        percentile(values, 0.99),
        percentile(values, 1.00),
        values.len(),
    );
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn scroll_100k_cat_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");

    let cat_bytes = cat_lines_stream(100_000);
    println!("INFO 100k_cat_stream_bytes={}", cat_bytes.len());

    let mut app = App::empty();
    app.zoom = 1.6;
    crate::open_scene(&mut app);
    app.finish_stream();

    let (choice, style) = terminal_font_style(&app.knobs);
    let (cw, ch) = cell_metrics(choice, style);
    let cols = (((700.0 - 32.0) / cw).floor() as u16).clamp(60, 160);
    let rows = (((420.0 - 54.0) / ch).floor() as u16).clamp(15, 60);

    app.insert_stream_terminal(
        "story\0/workspace/seer",
        "term_bench",
        "cat 100k.log",
        1,
        TerminalConfig {
            cols,
            rows,
            cell_w_px: (cw * 1.6 * 2.0).round() as u32,
            cell_h_px: (ch * 1.6 * 2.0).round() as u32,
            scrollback_lines: 100_000,
            theme: theme_from_pal(pal(true)),
        },
        Some(RestoreState {
            buffer: cat_bytes,
            cols: Some(cols),
            rows: Some(rows),
            scroll_y: None,
            cursor: None,
        }),
        b"",
    );
    crate::stories::stage(&mut app, view_card);

    let spec = crate::spec(|| App::empty());
    let mut rt = Runtime::with_text(
        app,
        view::app,
        (1280.0, 820.0),
        Env::new(true, 2.0),
        rune::text::TextSystem::new(spec.fonts, spec.families),
    );
    rt.poll = Some(crate::poll);

    // Warmup initial atlas + first scroll frames.
    rt.frame(0.0);
    for _ in 0..30 {
        rt.update(|s, _| {
            let ws = s.terminal.workspaces.get("story\0/workspace/seer").unwrap();
            ws.tabs[0].session.scroll(ScrollOp::Delta(-3));
        });
        std::thread::sleep(std::time::Duration::from_micros(600));
        rt.frame(1.0 / 120.0);
    }

    let total_frames = 300usize;
    let mut cpu_ms = Vec::with_capacity(total_frames);
    let mut wall_ms = Vec::with_capacity(total_frames);
    let mut view_ms = Vec::with_capacity(total_frames);
    let mut commit_ms = Vec::with_capacity(total_frames);
    let mut layout_ms = Vec::with_capacity(total_frames);
    let mut text_ms = Vec::with_capacity(total_frames);
    let mut allocs = Vec::with_capacity(total_frames);

    for i in 0..total_frames {
        // Scroll 4 lines per frame at 120 Hz (= 480 lines/sec through the 100k-line buffer).
        let delta = if (i / 150) % 2 == 0 { -4 } else { 4 };
        rt.update(|s, _| {
            let ws = s.terminal.workspaces.get("story\0/workspace/seer").unwrap();
            ws.tabs[0].session.scroll(ScrollOp::Delta(delta));
        });
        // Wait for the off-thread worker to publish the scrolled snapshot before timing the UI frame.
        let wait_start = std::time::Instant::now();
        while !rt
            .state
            .terminal
            .dirty_flag
            .load(std::sync::atomic::Ordering::Acquire)
            && wait_start.elapsed() < std::time::Duration::from_millis(20)
        {
            std::hint::spin_loop();
        }

        rune::take_frame_phases();
        let alloc_before = procwatch::alloc::thread_allocations();
        let wall_t0 = std::time::Instant::now();
        let cpu_t0 = rune::thread_cpu_ms();

        rt.frame(1.0 / 120.0);

        let frame_cpu = rune::thread_cpu_ms() - cpu_t0;
        let frame_wall = wall_t0.elapsed().as_secs_f64() * 1000.0;
        let frame_alloc = (procwatch::alloc::thread_allocations() - alloc_before) as f64;
        let phases = rune::take_frame_phases();
        let p = rune::PHASES;

        cpu_ms.push(frame_cpu);
        wall_ms.push(frame_wall);
        view_ms.push(phases[p]);
        commit_ms.push(phases[p + 1]);
        layout_ms.push(phases[p + 2]);
        text_ms.push(phases[p + 3]);
        allocs.push(frame_alloc);
    }

    report_metric("terminal_100k_scroll_cpu_ms", &mut cpu_ms);
    report_metric("terminal_100k_scroll_wall_ms", &mut wall_ms);
    report_metric("terminal_100k_scroll_view_ms", &mut view_ms);
    report_metric("terminal_100k_scroll_commit_ms", &mut commit_ms);
    report_metric("terminal_100k_scroll_layout_ms", &mut layout_ms);
    report_metric("terminal_100k_scroll_text_ms", &mut text_ms);
    report_metric("terminal_100k_scroll_allocations", &mut allocs);

    assert!(
        percentile(&cpu_ms, 0.99) < 8.33,
        "p99 CPU frame time {:.3} ms must stay within 120 fps budget (8.33 ms)",
        percentile(&cpu_ms, 0.99)
    );
}
