//! Checks a live run with GPU canvases restored from its snapshot (`widgets-gpu.md`, A run
//! restored from a snapshot), on a real GPU. Its program stops canvas `a` with an error and draws
//! `b` every frame. Paused briefly, it isn't saved: it hears only `resumed`. Paused past
//! `SAVE_AFTER` it is saved (its GPU let go) and restored: it hears `resumed`, then
//! `{ type: "gpu", lost: "unknown" }`; a batch naming a handle from before is dropped quietly,
//! `a` stays stopped without being reported again, and `b` draws again on a new device.
//! The repo has no automated tests (AGENTS.md), so this is a program to run by hand (it waits
//! out `SAVE_AFTER`): `cargo run --release -p opencode-widget --example gpu_restore_check`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::gpu::{self, Status};
use opencode_widget::live::{Grants, Live, LiveProgram, Post, SAVE_AFTER, Setup};
use opencode_widget::{Frame, fixtures};

const PROGRAM: &str = r#"
async function main() {
  const log = []
  widget.on("resumed", () => log.push("resumed"))
  const view = () => ui.column([ui.canvas({ key: "a", gpu: true, height: 100 }), ui.canvas({ key: "b", gpu: true, height: 100 }), ui.text("heard " + log.join(","))])
  widget.render(view())
  const format = widget.gpu.getPreferredCanvasFormat()
  const a = widget.gpu.getContext("a")
  const b = widget.gpu.getContext("b")
  let device, viewA, viewB
  const setup = async () => {
    device = await widget.gpu.requestDevice()
    device.lost.then((info) => log.push("lost:" + info.reason))
    a.configure({ device, format })
    b.configure({ device, format })
    viewA = a.getCurrentTexture().createView()
    viewB = b.getCurrentTexture().createView()
  }
  const clear = (view, clearValue) => {
    const encoder = device.createCommandEncoder()
    encoder.beginRenderPass({ colorAttachments: [{ view, clearValue, loadOp: "clear", storeOp: "store" }] }).end()
    device.queue.submit([encoder.finish()])
  }
  await setup()
  await widget.frame()
  // A batch of its own: it stops only "a".
  clear(viewA, [NaN, 0, 0, 1])
  let redone = false
  while (true) {
    await widget.frame()
    if (log.some((l) => l.startsWith("lost")) && !redone) {
      // Handles from before the loss: dropped quietly.
      clear(viewB, [1, 0, 0, 1])
      await widget.frame()
      redone = true
      await setup()
    }
    clear(viewA, [0, 0, 1, 1])
    clear(viewB, [0, 1, 0, 1])
    widget.render(view())
  }
}
"#;

fn device() -> gpu::Device {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: widget_gpu::device_limits(), ..Default::default() })).expect("a device");
    gpu::Device { device, queue, format: wgpu::TextureFormat::Rgba8Unorm, copies: false, aborts: None }
}

/// What a run showed while it was driven: GPU errors, the last text it rendered, and its
/// canvases' ids (`a`'s, `b`'s).
#[derive(Default)]
struct Seen {
    errors: Vec<String>,
    heard: String,
    ids: Vec<String>,
}

/// Gives the run the frames it asks for, for `long`.
fn drive(live: &Live, long: Duration, seen: &mut Seen) {
    let end = Instant::now() + long;
    let mut t = 0.0;
    while Instant::now() < end {
        for p in live.take() {
            match p {
                Post::Frame => {
                    t += 1.0 / 60.0;
                    live.frame(&Frame { t, dt: 1.0 / 60.0, width: 300.0, height: 200.0, scale: 2.0, ..Default::default() });
                }
                Post::Gpu(e) => seen.errors.push(format!("{} {:?}", e.message, e.canvases)),
                Post::Render(node) => {
                    let text = format!("{node:?}");
                    if let Some(at) = text.find("heard ") {
                        seen.heard = text[at + 6..].split('"').next().unwrap_or("").to_owned();
                    }
                    seen.ids = text.split("Canvas { id: Some(\"").skip(1).filter_map(|s| s.split('"').next()).map(str::to_owned).collect();
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Canvas `key`'s status and version, as the window would show it.
fn shows(live: &Live, seen: &Seen, key: &str) -> (Status, u64) {
    let c = live.gpu().expect("a GPU");
    let id = seen.ids.iter().find(|id| c.key(id).as_deref() == Some(key)).expect("the canvas's id");
    (c.status(id), c.canvas(id).map_or(0, |c| c.version))
}

fn start() -> Live {
    let program = Arc::new(LiveProgram::compile(&fixtures::served("live", PROGRAM)).expect("compiles"));
    let setup = Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) };
    let live = Live::start(program, serde_json::Value::Null, setup);
    live.canvas("a", 200, 200);
    live.canvas("b", 200, 200);
    live
}

fn main() {
    let d = device();
    println!("device: {} ({:?})", d.device.adapter_info().name, d.device.adapter_info().backend);
    gpu::provide(move || Some(d.clone()));
    let mut bad = false;
    let mut check = |ok: bool, what: &str| {
        println!("{}  {what}", if ok { "ok  " } else { "FAIL" });
        bad |= !ok;
    };

    let live = start();
    let mut seen = Seen::default();
    drive(&live, Duration::from_millis(1500), &mut seen);
    let before = seen.errors.clone();
    check(before.len() == 1 && before[0].contains("must be a finite number") && before[0].contains("\"a\""), &format!("canvas a stopped once: {before:?}"));
    let (_, b_before) = shows(&live, &seen, "b");
    live.pause();
    std::thread::sleep(Duration::from_secs(1));
    live.resume();
    drive(&live, Duration::from_millis(1000), &mut seen);
    check(seen.heard == "resumed", &format!("paused 1 s, not saved: it heard {:?}", seen.heard));

    live.pause();
    println!("     paused past SAVE_AFTER ({} s)...", SAVE_AFTER.as_secs() + 2);
    std::thread::sleep(SAVE_AFTER + Duration::from_secs(2));
    live.resume();
    drive(&live, Duration::from_millis(2000), &mut seen);
    check(seen.heard == "resumed,resumed,lost:unknown", &format!("saved and restored: it heard {:?}", seen.heard));
    check(seen.errors == before, &format!("no error after the restore (the old handle dropped quietly, a not reported again): {:?}", &seen.errors[before.len().min(seen.errors.len())..]));
    let (a, _) = shows(&live, &seen, "a");
    check(matches!(&a, Status::Stopped(m) if m.contains("must be a finite number")), &format!("a stays stopped: {a:?}"));
    let (b, b_after) = shows(&live, &seen, "b");
    check(b == Status::Drawn && b_after > 0 && b_after != b_before, &format!("b draws again: {b:?}, version {b_before} then {b_after}"));
    live.stop();
    if bad {
        std::process::exit(1);
    }
}
