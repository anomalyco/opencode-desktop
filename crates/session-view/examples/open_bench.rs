//! Offline newest-page replay. Pass a private, newest-first API response, kept outside Git.
//! `cargo run --release -p session-view --features open-bench --example open_bench -- PAGE.json [LIMIT]`
//! An optional second page after LIMIT alternates A/B on the same worker.
//! OPEN_BENCH_RTT_MS / OPEN_BENCH_MBPS shape the local replay's response.
//! Measures readiness for paint, not presentation; `read_only` + RUNE_FRAME_LOG times paint.
use std::{cell::RefCell, time::Duration, io::{Read, Write}};
use rune::time::Instant;
use opencode_kit::client::Server;
use opencode_kit::model::{Transcript, parse_messages};
use rune::{Env, Runtime};
use session_view::{fixture::SESSION_ID, story::{Host, view}, timeline};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let limit: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut paths = vec![args[1].clone()];
    paths.extend(args.get(3).cloned());
    for run in 0..9 {
      if run > 0 { paths.reverse(); }
      for path in &paths {
        let raw = std::fs::read(path).unwrap();
        let mut data: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        data["data"].as_array_mut().unwrap().truncate(limit);
        let raw = serde_json::to_vec(&data).unwrap();
        let bytes = raw.len();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = Server::new(&format!("http://{}", listener.local_addr().unwrap())).with_password("fixture");
        let number = |key| std::env::var(key).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let (rtt, mbps) = (number("OPEN_BENCH_RTT_MS"), number("OPEN_BENCH_MBPS"));
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_nodelay(true).unwrap();
            let mut buf = [0; 4096];
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0, "incomplete HTTP request");
                request.extend_from_slice(&buf[..n]);
            }
            std::thread::sleep(Duration::from_secs_f64(rtt / 1000.0));
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n").unwrap();
            let transfer = Instant::now();
            let mut sent = 0;
            for chunk in raw.chunks(16 * 1024) {
                sent += chunk.len();
                if mbps > 0.0 {
                    let due = Duration::from_secs_f64(sent as f64 * 8.0 / (mbps * 1e6));
                    // Absolute pacing: a late scheduler wake must not accumulate per chunk.
                    if let Some(wait) = due.checked_sub(transfer.elapsed()) { std::thread::sleep(wait); }
                }
                socket.write_all(chunk).unwrap();
            }
        });
        let start = Instant::now();
        let (data, timing) = server.read_timed("/api/session/fixture/message").unwrap();
        let decode = timing.parsing.elapsed().as_secs_f64() * 1000.0;
        let http = timing.parsing.duration_since(start).as_secs_f64() * 1000.0;
        let model_start = Instant::now();
        let messages = parse_messages(&data);
        let model = model_start.elapsed().as_secs_f64() * 1000.0;
        let count = messages.len();
        let markdown_parts = messages.iter().filter_map(|m| match m {
            opencode_kit::model::Message::Assistant(a) => Some(a.parts.iter().filter(|p| matches!(p, opencode_kit::model::Part::Text { .. })).count()),
            _ => None,
        }).sum::<usize>();
        let h = Host { talk: RefCell::new(Some(Transcript { session: SESSION_ID.into(), messages })), ..Host::default() };
        let rows = timeline::rows(&h, h.talk.borrow().as_ref().unwrap()).len();
        timeline::timeline_reset_row_builds();
        rune::take_frame_phases();
        let render_start = Instant::now();
        let mut rt = Runtime::new(h, view, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let first = render_start.elapsed().as_secs_f64() * 1000.0;
        let visible = start.elapsed().as_secs_f64() * 1000.0;
        let builds = timeline::timeline_row_builds();
        assert!(builds > 0, "no transcript rows built");
        let phases = rune::take_frame_phases();
        let mut frames = 0;
        let mut settled = false;
        for _ in 0..240 {
            frames += 1;
            if !rt.frame(1.0 / 120.0) { settled = true; break; }
        }
        let sample = render_start.elapsed().as_secs_f64() * 1000.0;
        let p = rune::PHASES;
        println!("METRIC variant={} run={run} raw_bytes={bytes} messages={count} rows={rows} built={builds} markdown_parts={markdown_parts} http_ms={http:.3} decode_ms={decode:.3} model_ms={model:.3} first_frame_ms={first:.3} ready_to_paint_ms={visible:.3} sampled_frames_ms={sample:.3} settled={settled} settle_frames={frames} view_ms={:.3} commit_ms={:.3} layout_ms={:.3} text_ms={:.3}", std::path::Path::new(path).file_name().unwrap().to_string_lossy(), phases[p], phases[p+1], phases[p+2], phases[p+3]);
        worker.join().unwrap();
      }
    }
}
