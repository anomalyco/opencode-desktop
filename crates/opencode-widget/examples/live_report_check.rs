//! Checks what a live run's failures report (the app's `report`: phase, message, and line and
//! column in the widget template's source, the program's less its preludes' lines), with the real
//! preludes and the widget template's lines declared (`LiveProgram::compile_at`), as opencode-ios's
//! `AgentWidgetFetchReportTests` and `AgentWidgetLiveReportTests` do: an uncaught raw
//! `__host.fetch` rejection keeps its kind; an uncaught `widget.fetch` denial is a fetch error at
//! its line; a GPU op error is at the frame request that posted its batch; the recording API's
//! `TypeError` and an invalid draw are at their lines. On a real GPU (Metal here, Vulkan on Linux).
//! The repo has no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run -p opencode-widget --example live_report_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Frame, fixtures, gpu};

/// A report: phase, message, and line and column in the widget template's source.
#[derive(Debug, PartialEq)]
struct Report {
    phase: String,
    message: String,
    at: Option<(u32, u32)>,
}

fn device() -> gpu::Device {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: gpu::device_limits(), ..Default::default() })).expect("a device");
    gpu::Device { device, queue, format: wgpu::TextureFormat::Rgba8Unorm, handoff: None }
}

/// Runs `program`, whose widget template's source is its last `source` lines, until its first
/// report (or 6 s), answering every frame request; what it reports, as the app does.
fn report(program: &str, source: &str, hosts: Option<Vec<String>>) -> Option<Report> {
    let lines = |s: &str| s.matches('\n').count() as u32 + 1;
    let offset = lines(program) - lines(source);
    let live = Live::start(
        Arc::new(LiveProgram::compile_at(program, offset).expect("compiles")),
        serde_json::Value::Null,
        Setup { grants: Grants { https: hosts }, transport: None, allow_local_network: false, notify: Arc::new(|| {}) },
    );
    live.canvas("scene", 300, 200);
    // As the app's `report`: the location in the source, its ` (line, col)` left out of the message.
    let made = |phase: &str, message: &str, at: Option<(u32, u32)>| {
        let message = match at {
            Some((l, c)) => message.strip_suffix(&format!(" (line {l}, col {c})")).unwrap_or(message),
            None => message,
        };
        Some(Report { phase: phase.into(), message: message.into(), at: at.filter(|(l, _)| *l > offset).map(|(l, c)| (l - offset, c)) })
    };
    let end = Instant::now() + Duration::from_secs(6);
    let mut t = 0.0;
    let found = loop {
        if Instant::now() > end {
            break None;
        }
        let ended = live.ended();
        for post in live.take() {
            match post {
                Post::Frame => {
                    t += 1.0 / 60.0;
                    live.frame(&Frame { t, dt: 1.0 / 60.0, width: 300.0, height: 200.0, scale: 3.0, ..Default::default() });
                }
                Post::Gpu(e, at) if e.kind == gpu::ErrorKind::Program => {
                    live.stop();
                    return made("gpu", &e.message, at);
                }
                _ => {}
            }
        }
        match ended {
            Some(Ended::Failed(m)) => break made(live.failed_phase().unwrap_or("main"), &m, live.failed_at()),
            Some(other) => break Some(Report { phase: format!("{other:?}"), message: String::new(), at: None }),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    };
    live.stop();
    found
}

fn main() {
    let d = device();
    println!("device: {} ({:?})", d.device.adapter_info().name, d.device.adapter_info().backend);
    gpu::provide(move || Some(d.clone()));
    let mut bad = false;
    let mut check = |ok: bool, what: &str, got: &Option<Report>| {
        println!("{}  {what}: {got:?}", if ok { "ok  " } else { "FAIL" });
        bad |= !ok;
    };

    // (1) A raw `__host.fetch` rejection, uncaught (no prelude): a fetch error that keeps its kind.
    let raw = "const __widget_protocol = 2\nasync function __widget_live() { await __host.fetch({ url: \"https://example.org\", method: \"GET\", headers: {} }) }";
    let got = report(raw, raw, None);
    check(got.as_ref().is_some_and(|r| r.phase == "fetch" && r.message.starts_with("CapabilityDenied: ")), "(1) an uncaught raw __host.fetch rejection keeps its kind", &got);

    let live = |source: &str| fixtures::served("live", source);
    // (2), (3) `widget.fetch` of a host not declared, uncaught: a fetch error at its line.
    let source = "async function main() {\n  widget.render(ui.text(\"x\"))\n  await widget.fetch(\"https://example.org/a\")\n}\n";
    let got = report(&live(source), source, Some(vec!["api.example.com".into()]));
    check(got.as_ref().is_some_and(|r| r.phase == "fetch" && r.message.starts_with("CapabilityDenied: ")), "(2) an uncaught widget.fetch denial is a fetch error", &got);
    check(got.as_ref().is_some_and(|r| r.at.is_some_and(|(l, c)| l == 3 && c > 0)), "(3) at its line 3, with a column", &got);

    // (4) A GPU op over a limit: at the frame request that posted its batch.
    let source = "async function main() {\n  widget.render(ui.canvas({ key: \"scene\", gpu: true, height: 100 }))\n  const device = await widget.gpu.requestDevice()\n  widget.gpu.getContext(\"scene\").configure({ device, format: widget.gpu.getPreferredCanvasFormat() })\n  device.createBuffer({ size: 64 * 1024 * 1024 + 4, usage: GPUBufferUsage.VERTEX })\n  for (let i = 0; i < 20; i++) await widget.frame()\n}\n";
    let got = report(&live(source), source, None);
    check(
        got.as_ref().is_some_and(|r| r.phase == "gpu" && r.message == "gpu op 1: \"buffer\" argument 2 is over the limit of 67108864" && r.at.is_some_and(|(l, c)| l == 6 && c > 0)),
        "(4) a GPU op error at the frame request that posted it, line 6",
        &got,
    );

    // (5) The recording API's own check, in the prelude: at the program's line 4.
    let source = "async function main() {\n  widget.render(ui.canvas({ key: \"scene\", gpu: true, height: 100 }))\n  const device = await widget.gpu.requestDevice()\n  device.createBuffer()\n  await widget.frame()\n}\n";
    let got = report(&live(source), source, None);
    check(
        got.as_ref().is_some_and(|r| r.phase == "main" && r.message == "TypeError: createBuffer's descriptor must be an object" && r.at.is_some_and(|(l, _)| l == 4)),
        "(5) a recording API TypeError at line 4",
        &got,
    );

    // (6) A draw command the encoder can't write: phase draw, at the `widget.draw`.
    let source = "async function main() {\n  widget.render(ui.canvas({ key: \"c\", height: 10 }))\n  widget.draw(\"c\", [[\"bogus\"]])\n  await widget.frame()\n}\n";
    let got = report(&live(source), source, None);
    check(got.as_ref().is_some_and(|r| r.phase == "draw" && r.at.is_some_and(|(l, _)| l == 3)), "(6) an invalid draw in phase draw at line 3", &got);

    if bad {
        std::process::exit(1);
    }
}
