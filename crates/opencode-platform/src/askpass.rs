//! The app's executable as `SSH_ASKPASS` for the SSH plugin, as Desktop's CLI is: `ssh` runs it
//! with the prompt as its argument and `OPENCODE_SSH_ASKPASS_PORT` and `_TOKEN` set; it hands the
//! prompt to the plugin over that loopback port, prints the answer the user gave in the app's
//! dialog, and exits, without opening a window.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Answers `ssh`'s prompt and exits, when this run is an askpass one.
pub fn run_if_asked() {
    let Some(port) = std::env::var_os("OPENCODE_SSH_ASKPASS_PORT") else { return };
    let token = std::env::var_os("OPENCODE_SSH_ASKPASS_TOKEN").unwrap_or_default();
    let text = std::env::args().nth(1).unwrap_or_default();
    // OpenSSH sets `SSH_ASKPASS_PROMPT=confirm` for a yes/no question, as a host's key is.
    let confirm = std::env::var_os("SSH_ASKPASS_PROMPT").is_some_and(|p| p == "confirm") || text.contains("(yes/no");
    let answer = ask(&port.to_string_lossy(), &token.to_string_lossy(), &text, confirm);
    match answer {
        Some(value) if !value.is_empty() => {
            println!("{value}");
            std::process::exit(0);
        }
        _ => std::process::exit(1),
    }
}

fn ask(port: &str, token: &str, text: &str, confirm: bool) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().ok()?)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(300))).ok()?;
    let request = serde_json::json!({ "token": token, "text": text, "confirm": confirm });
    stream.write_all(format!("{request}\n").as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v["value"].as_str().map(str::to_owned)
}
