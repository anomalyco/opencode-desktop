//! WSL's commands and what they print (`runtime.ts`, `remote-cli.ts`): `wsl.exe`'s arguments,
//! the scripts run inside a distro, and the parsers for `--list --verbose`, `--list --online`
//! and `opencode --version`.

use crate::contract::{InstalledDistro, OnlineDistro};

/// How long a `wsl.exe` command may take: long enough for a cold start, short enough to fail
/// fast on a wedged distro.
pub const TIMEOUT_MS: u64 = 20_000;
/// Installs (WSL, a distro, OpenCode).
pub const INSTALL_TIMEOUT_MS: u64 = 15 * 60_000;
/// How long a started server has to answer `/api/info`.
pub const HEALTH_TIMEOUT_MS: u64 = 30_000;

/// The program: `wsl.exe` from System32, or `--wsl-exe` (a missing one reads as WSL not
/// installed, as on a machine without it).
pub fn program() -> String {
    opencode_plugin::system::arg("wsl-exe").unwrap_or_else(|| "wsl.exe".to_owned())
}

/// `wslArgs`: `--exec` runs the command itself; `--` would hand it to the distro's shell, which
/// expands `$VAR` before `sh -lc` runs the script.
pub fn in_distro(args: &[&str], distro: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(d) = distro {
        out.extend(["-d".to_owned(), d.to_owned()]);
    }
    out.push("--exec".to_owned());
    out.extend(args.iter().map(|a| (*a).to_owned()));
    out
}

pub fn sh(script: &str, distro: &str) -> Vec<String> {
    in_distro(&["sh", "-lc", script], Some(distro))
}

/// Single quotes for a POSIX shell.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// The managed CLI's path, when installed.
pub const DISCOVER_SCRIPT: &str = "cli=\"\"\nif [ -z \"$cli\" ] && [ -x \"$HOME/.opencode/bin/opencode\" ]; then cli=\"$HOME/.opencode/bin/opencode\"; fi\nif [ -n \"$cli\" ]; then printf '%s\\n' \"$cli\"; fi\n";

pub fn version_script(command: &str) -> String {
    format!("if [ -x {command} ]; then {command} --version 2>/dev/null || true; fi\n")
}

/// The managed CLI installer, which also puts it on the shell's PATH, then a check that the
/// version it installed is `version`.
pub fn install_script(version: &str) -> Option<String> {
    let valid = version == "local" || (version.starts_with(|c: char| c.is_ascii_digit()) && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)));
    if !valid {
        return None;
    }
    let v = quote(version);
    Some(format!(
        "set -eu\ncurl -fsSL https://raw.githubusercontent.com/anomalyco/opencode/v2/install | bash -s -- --version {v}\ntest \"$(\"$HOME/.opencode/bin/opencode\" --version | awk '{{print $NF}}' | sed 's/^v//')\" = {v}\n"
    ))
}

/// The server's script: off the Windows PATH, a password, no file watcher, `serve` on `port`.
pub fn sidecar_script(opencode: &str, password: &str, port: u16, packaged: bool) -> String {
    [
        "set -euo pipefail".to_owned(),
        "cd \"$HOME\" || cd /".to_owned(),
        "PATH=$(awk -v RS=: -v ORS=: '$0 !~ /^\\/mnt\\//' <<<\"$PATH\" | sed \"s/:$//\")".to_owned(),
        "export PATH".to_owned(),
        "export WSLENV=".to_owned(),
        "export OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER=true".to_owned(),
        "export OPENCODE_CLIENT=desktop".to_owned(),
        format!("export OPENCODE_SERVER_PASSWORD={}", quote(password)),
        "export XDG_STATE_HOME=\"$HOME/.local/state\"".to_owned(),
        format!("exec {} --log-level {} serve --hostname 0.0.0.0 --port {port}", quote(opencode), if packaged { "warn" } else { "info" }),
    ]
    .join("\n")
}

/// Non-empty lines, trimmed, joined.
pub fn summarize(value: &str) -> String {
    value.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

pub fn first_line(value: &str) -> Option<String> {
    value.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

/// `opencode --version`'s version: the word after its last ` v`, or the line.
pub fn parse_version(output: &str) -> Option<String> {
    let line = first_line(output)?;
    Some(match line.rfind(" v") {
        Some(at) => line[at + 2..].to_owned(),
        None => line,
    })
    .filter(|v| !v.is_empty())
}

/// `wsl --list --verbose`: `* NAME  STATE  VERSION` rows, the default marked.
pub fn parse_installed(output: &str) -> Vec<InstalledDistro> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let default = trimmed.starts_with('*');
            let rest = trimmed.trim_start_matches('*').trim();
            // NAME, then two or more spaces, then STATE, then VERSION.
            let (name, tail) = split_columns(rest)?;
            let mut words = tail.split_whitespace();
            let _state = words.next()?;
            let version = words.next()?;
            if words.next().is_some() || name.eq_ignore_ascii_case("name") {
                return None;
            }
            Some(InstalledDistro { name: name.to_owned(), version: version.parse().ok(), is_default: default })
        })
        .collect()
}

/// `wsl --list --online`: `NAME  FRIENDLY NAME` rows after the preamble.
pub fn parse_online(output: &str) -> Vec<OnlineDistro> {
    output
        .lines()
        .filter_map(|line| {
            let (name, label) = split_columns(line.trim())?;
            if name.eq_ignore_ascii_case("name") || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                return None;
            }
            Some(OnlineDistro { name: name.to_owned(), label: label.trim().to_owned() })
        })
        .collect()
}

/// A row's first column and the rest, split at the first run of two or more spaces.
fn split_columns(line: &str) -> Option<(&str, &str)> {
    let at = line.find("  ")?;
    let (head, tail) = line.split_at(at);
    let tail = tail.trim_start();
    (!head.is_empty() && !tail.is_empty()).then_some((head.trim(), tail))
}
