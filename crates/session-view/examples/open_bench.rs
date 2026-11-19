//! Offline newest-page replay. Pass a private, newest-first API response, kept outside Git.
//! `cargo run --release -p session-view --features open-bench --example open_bench -- PAGE.json [LIMIT]`
//! An optional second page after LIMIT alternates A/B on the same worker.
//! OPEN_BENCH_RTT_MS / OPEN_BENCH_MBPS shape the local replay's response.
//! Measures readiness for paint, not presentation; `read_only` + RUNE_FRAME_LOG times paint.
use std::{
    cell::RefCell,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::ExitCode,
    time::Duration,
};
use rune::time::Instant;
use opencode_kit::client::Server;
use opencode_kit::model::{Transcript, parse_messages};
use rune::{Env, Runtime};
use session_view::{fixture::SESSION_ID, story::{Host, view}, timeline};

// Standalone CLI benchmark in `session-view`; `clippy.toml`'s `config.rs` is not reachable from this crate.
#[allow(clippy::disallowed_methods)]
fn env_f64(key: &str) -> Result<f64, String> {
    match std::env::var(key) {
        Ok(raw) if !raw.is_empty() => {
            let value: f64 = raw.parse().map_err(|e| format!("invalid {key}={raw:?}: {e}"))?;
            if !value.is_finite() || value < 0.0 {
                return Err(format!("invalid {key}={raw:?}: expected a finite non-negative number"));
            }
            Ok(value)
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(0.0),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("invalid {key}: value is not valid UTF-8")),
    }
}

fn serve_replay(listener: TcpListener, raw: Vec<u8>, rtt: f64, mbps: f64) -> Result<(), String> {
    let bytes = raw.len();
    let (mut socket, _) = listener.accept().map_err(|e| format!("accept replay connection: {e}"))?;
    socket.set_nodelay(true).map_err(|e| format!("set TCP_NODELAY: {e}"))?;
    let mut buf = [0; 4096];
    let mut request = Vec::new();
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = socket.read(&mut buf).map_err(|e| format!("read HTTP request: {e}"))?;
        if n == 0 {
            return Err("incomplete HTTP request".into());
        }
        request.extend_from_slice(&buf[..n]);
    }
    std::thread::sleep(Duration::from_secs_f64(rtt / 1000.0));
    write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("write HTTP response headers: {e}"))?;
    let transfer = Instant::now();
    let mut sent = 0;
    for chunk in raw.chunks(16 * 1024) {
        sent += chunk.len();
        if mbps > 0.0 {
            let due = Duration::from_secs_f64(sent as f64 * 8.0 / (mbps * 1e6));
            // Absolute pacing: a late scheduler wake must not accumulate per chunk.
            if let Some(wait) = due.checked_sub(transfer.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        socket.write_all(chunk).map_err(|e| format!("write HTTP response body: {e}"))?;
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let Some(first_path) = args.get(1) else {
        return Err("usage: open_bench PAGE.json [LIMIT [SECOND_PAGE.json]]".into());
    };
    let limit: usize = match args.get(2) {
        Some(raw) => raw.parse().map_err(|e| format!("invalid LIMIT {raw:?}: {e}"))?,
        None => 20,
    };
    let (rtt, mbps) = (env_f64("OPEN_BENCH_RTT_MS")?, env_f64("OPEN_BENCH_MBPS")?);
    let mut paths = vec![first_path.clone()];
    paths.extend(args.get(3).cloned());
    for run in 0..9 {
        if run > 0 {
            paths.reverse();
        }
        for path in &paths {
            let variant = Path::new(path)
                .file_name()
                .ok_or_else(|| format!("{path}: path has no file name"))?
                .to_string_lossy();
            let raw = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            let mut data: serde_json::Value = serde_json::from_slice(&raw)
                .map_err(|e| format!("parse JSON in {path}: {e}"))?;
            data.get_mut("data")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| format!("{path}: expected JSON object with a \"data\" array"))?
                .truncate(limit);
            let raw = serde_json::to_vec(&data)
                .map_err(|e| format!("serialize truncated JSON for {path}: {e}"))?;
            let bytes = raw.len();
            let listener = TcpListener::bind("127.0.0.1:0")
                .map_err(|e| format!("bind local replay listener: {e}"))?;
            let addr = listener
                .local_addr()
                .map_err(|e| format!("read local replay listener address: {e}"))?;
            let server = Server::new(&format!("http://{addr}")).with_password("fixture");
            let worker = std::thread::spawn(move || serve_replay(listener, raw, rtt, mbps));
            let start = Instant::now();
            let (data, timing) = server
                .read_timed("/api/session/fixture/message")
                .map_err(|e| format!("replay request for {path}: {e}"))?;
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
            let rows = {
                let talk = h.talk.borrow();
                let transcript = talk.as_ref().ok_or_else(|| format!("{path}: replay host missing transcript"))?;
                timeline::rows(&h, transcript).len()
            };
            timeline::timeline_reset_row_builds();
            rune::take_frame_phases();
            let render_start = Instant::now();
            let mut rt = Runtime::new(h, view, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let first = render_start.elapsed().as_secs_f64() * 1000.0;
            let visible = start.elapsed().as_secs_f64() * 1000.0;
            let builds = timeline::timeline_row_builds();
            if builds == 0 {
                return Err(format!("{path}: no transcript rows built"));
            }
            let phases = rune::take_frame_phases();
            let mut frames = 0;
            let mut settled = false;
            for _ in 0..240 {
                frames += 1;
                if !rt.frame(1.0 / 120.0) {
                    settled = true;
                    break;
                }
            }
            let sample = render_start.elapsed().as_secs_f64() * 1000.0;
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(format!("replay worker failed for {path}: {err}")),
                Err(_) => return Err(format!("replay worker panicked for {path}")),
            }
            let p = rune::PHASES;
            println!(
                "METRIC variant={variant} run={run} raw_bytes={bytes} messages={count} rows={rows} built={builds} markdown_parts={markdown_parts} http_ms={http:.3} decode_ms={decode:.3} model_ms={model:.3} first_frame_ms={first:.3} ready_to_paint_ms={visible:.3} sampled_frames_ms={sample:.3} settled={settled} settle_frames={frames} view_ms={:.3} commit_ms={:.3} layout_ms={:.3} text_ms={:.3}",
                phases[p], phases[p + 1], phases[p + 2], phases[p + 3],
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Err(err) = run() {
        eprintln!("open_bench: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
