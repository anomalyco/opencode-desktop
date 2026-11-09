//! Per-keystroke cost of the `@` popover over a large workspace: 5,000 root sessions and
//! 20,000 files, typing `@runedesktop` one key a frame and deleting it again.

use super::tests::runtime;
use rune::{Key, KeyEvent, Runtime};

use crate::App;
use opencode_kit::model::SessionInfo;

const WORDS: [&str; 16] = ["fix", "flaky", "reconnect", "sidebar", "palette", "review", "scroll", "jank", "upgrade", "toolchain", "release", "cache", "mention", "search", "composer", "timeline"];

pub(crate) fn large(rt: &mut Runtime<App>) {
    let project = rt.state.sessions[0].project.clone();
    let directory = rt.state.sessions[0].directory.clone();
    for i in 0..5_000usize {
        let title = format!("{} {} {} {i}", WORDS[i % 16], WORDS[(i / 16) % 16], WORDS[(i / 256) % 16]).to_uppercase();
        rt.state.sessions.push(SessionInfo { id: format!("ses_bench{i:06}aaaaaaaaaaaaaaa"), title, project: project.clone(), directory: directory.clone(), updated: i as i64, ..Default::default() });
    }
    rt.state.sessions.push(SessionInfo { id: "ses_benchrunedesktopaaaaaaa".into(), title: "RUNE DESKTOP".into(), project, directory, updated: 2, ..Default::default() });
    rt.state.mentions.file_index = (0..20_000usize)
        .map(|i| format!("research/{}-{}/runs/audit{}/{}-{i}.json", WORDS[i % 16], WORDS[(i / 16) % 16], i % 7, WORDS[(i / 256) % 16]))
        .collect();
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn mention_typing_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut rt = runtime();
    large(&mut rt);
    super::tests::tap(&mut rt, "prompt");
    let mut times = Vec::new();
    let query = "runedesktop";
    for sample in 0..6 {
        let mut step = |rt: &mut Runtime<App>, key: Key| {
            let start = rune::thread_cpu_ms();
            rt.key(KeyEvent { key, mods: Default::default() });
            rt.frame(1.0 / 120.0);
            if sample > 0 {
                times.push(rune::thread_cpu_ms() - start);
            }
        };
        step(&mut rt, Key::Text("@".into()));
        for c in query.chars() {
            step(&mut rt, Key::Text(c.to_string()));
        }
        for _ in 0..=query.len() {
            step(&mut rt, Key::Backspace);
        }
    }
    crate::perf::report("mention_key_frame_cpu_ms", &mut times);
    let at = |p: f64| times[((times.len() - 1) as f64 * p).round() as usize];
    println!("METRIC name=mention_key_frame_cpu_ms p90={:.4}", at(0.9));
}
