//! `ssh` itself (`command.ts`, `remote-cli.ts`, `bootstrap.ts`): the target a user types, the
//! options every connection takes, the tunnel's, and the scripts run on the host to find,
//! install and start its OpenCode service and read its registration.

/// Why a connection failed (`SshError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    Connection,
    Input,
    Platform,
    Version,
    Install,
    Service,
    HostKey,
    SshMissing,
    Unpublished,
}

impl Code {
    /// What the dialog and the cover say about it (`error.*`).
    pub fn message(self) -> &'static str {
        match self {
            Code::Input => "Enter a host or SSH connection command. Remote commands and unsupported SSH options aren\u{2019}t allowed.",
            Code::Connection => "Could not establish the SSH connection. Check your network and SSH configuration.",
            Code::Platform => "This remote platform is not supported. Automatic setup currently requires Linux or macOS on x64 or arm64.",
            Code::Version => "The remote service must match this Desktop version before connecting.",
            Code::Install => "Could not install the remote server. Check connectivity, disk space, and that tar is installed.",
            Code::Unpublished => "This Desktop version has no published remote server. For development builds, install and start V2 on the host, then retry.",
            Code::Service => "SSH connected, but the OpenCode server did not become ready.",
            Code::HostKey => "The host\u{2019}s identity could not be verified. Verify its fingerprint before updating your SSH known hosts.",
            Code::SshMissing => "OpenSSH was not found. Install an OpenSSH client and ensure ssh is available on PATH.",
        }
    }
}

/// A failure with its detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: Code,
    pub detail: String,
}

impl Failure {
    pub fn new(code: Code, detail: impl Into<String>) -> Failure {
        Failure { code, detail: detail.into() }
    }
}

/// A host and the options before it, from what the user typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub args: Vec<String>,
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `parseTarget`: `host`, `user@host`, or an `ssh` command with only connection options
/// (`-p -l -i -F -J`, `-o` for a few, `-4 -6 -C -A -a`) and no remote command.
pub fn parse_target(input: &str) -> Result<Target, Failure> {
    let bad = || Failure::new(Code::Input, "");
    let mut tokens: Vec<String> = Vec::new();
    let (mut word, mut quoting, mut started) = (String::new(), None::<char>, false);
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '\n' | '\r' | '\0') {
            return Err(bad());
        }
        if c == '\\' && quoting != Some('\'') && i + 1 < chars.len() && (chars[i + 1].is_whitespace() || "\\\"'".contains(chars[i + 1])) {
            word.push(chars[i + 1]);
            started = true;
            i += 2;
            continue;
        }
        if let Some(q) = quoting {
            if c == q { quoting = None } else { word.push(c) }
        } else if c == '\'' || c == '"' {
            quoting = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                tokens.push(std::mem::take(&mut word));
            }
            word.clear();
            started = false;
        } else {
            word.push(c);
            started = true;
        }
        i += 1;
    }
    if quoting.is_some() {
        return Err(bad());
    }
    if started {
        tokens.push(word);
    }
    if tokens.first().map(String::as_str) == Some("ssh") {
        tokens.remove(0);
    }
    const OPTIONS: [&str; 10] = ["hostname", "user", "port", "identityfile", "identityagent", "identitiesonly", "proxyjump", "proxycommand", "connecttimeout", "addressfamily"];
    let mut args = Vec::new();
    while tokens.first().is_some_and(|t| t.starts_with('-')) {
        let token = tokens.remove(0);
        if ["-4", "-6", "-C", "-A", "-a"].contains(&token.as_str()) {
            args.push(token);
            continue;
        }
        let flag: String = token.chars().take(2).collect();
        if !["-p", "-l", "-i", "-F", "-J", "-o"].contains(&flag.as_str()) {
            return Err(bad());
        }
        let value = if token.len() > 2 { token[2..].to_owned() } else if tokens.is_empty() { String::new() } else { tokens.remove(0) };
        if value.is_empty() || value.starts_with('-') {
            return Err(bad());
        }
        if flag == "-p" && !value.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p)) {
            return Err(bad());
        }
        if flag == "-o" && !OPTIONS.contains(&value.split(['=', ' ', '\t']).next().unwrap_or_default().to_lowercase().as_str()) {
            return Err(bad());
        }
        args.push(flag);
        args.push(value);
    }
    let host = tokens.first().cloned().unwrap_or_default();
    let host_ok = !host.is_empty() && !host.starts_with('-') && host.chars().all(|c| c.is_ascii_alphanumeric() || "_@.:[]%-".contains(c));
    if tokens.len() != 1 || !host_ok {
        return Err(bad());
    }
    if let Some(at) = host.rfind('@')
        && host[..at].contains(':')
    {
        return Err(bad());
    }
    Ok(Target { host, args })
}

/// The program.
pub fn ssh() -> &'static str {
    if cfg!(windows) { "ssh.exe" } else { "ssh" }
}

/// `sshArgs`: every connection's options, then the target's.
pub fn ssh_args(target: &Target) -> Vec<String> {
    let mut a: Vec<String> = ["-T", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", "-o", "RemoteCommand=none", "-o", "RequestTTY=no", "-o", "PermitLocalCommand=no"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    a.extend(target.args.iter().cloned());
    a
}

/// Where one `ssh` run writes its own messages (`-E`), read back once it ends. Windows' OpenSSH
/// never exits after logging to a stderr that is a pipe, as every failure does, so its log goes
/// to a file instead.
pub struct Log(std::path::PathBuf);

impl Log {
    pub fn new() -> Log {
        Log(std::env::temp_dir().join(format!("opencode-ssh-{}.log", crate::token())))
    }

    /// `-E <file>`, before the host.
    pub fn args(&self) -> [String; 2] {
        ["-E".into(), self.0.to_string_lossy().into_owned()]
    }

    /// Removes logs a day old: a run killed while `ssh` held its log open leaves it behind.
    pub fn sweep() {
        let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
        let day = std::time::Duration::from_secs(86_400);
        for entry in dir.flatten() {
            let name = entry.file_name();
            let ours = name.to_string_lossy().starts_with("opencode-ssh-") && name.to_string_lossy().ends_with(".log");
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
            if ours && old {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }

    /// What `ssh` logged, and the file gone.
    pub fn take(&self) -> String {
        let text = std::fs::read_to_string(&self.0).unwrap_or_default();
        std::fs::remove_file(&self.0).ok();
        text
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// `tunnelArgs`: forwards `127.0.0.1:local` to the remote service and keeps a session open on
/// stdin, which the tunnel's process owns.
pub fn tunnel_args(target: &Target, local: u16, remote_host: &str, remote_port: u16) -> Vec<String> {
    let mut a: Vec<String> = ["-o", "ControlMaster=no", "-o", "ControlPersist=no"].iter().map(|s| (*s).to_owned()).collect();
    a.extend(ssh_args(target));
    a.extend(["-o".into(), "ExitOnForwardFailure=yes".into(), "-L".into(), format!("127.0.0.1:{local}:{remote_host}:{remote_port}"), target.host.clone(), "sh -c 'exec cat >/dev/null'".into()]);
    a
}

/// What a failed command said, without the service's private registration.
pub fn failure_detail(code: Option<i64>, stdout: &str, stderr: &str) -> String {
    let mut out = stdout.to_owned();
    while let Some(start) = out.find("OPENCODE_SSH_REGISTRATION_BEGIN") {
        let end = out[start..].find("OPENCODE_SSH_REGISTRATION_END").map_or(out.len(), |e| start + e + "OPENCODE_SSH_REGISTRATION_END".len());
        out.replace_range(start..end, "");
    }
    let parts: Vec<&str> = [stderr.trim(), out.trim()].into_iter().filter(|s| !s.is_empty()).collect();
    if parts.is_empty() { format!("{{\"exitCode\":{}}}", code.map_or("null".into(), |c| c.to_string())) } else { parts.join("\n") }
}

const REGISTRATION: &str = r#"status=$("$cli" service status) || exit 0
if [ "$status" = stopped ]; then exit 0; fi
printf 'OPENCODE_SSH_STATUS=%s\n' "$status"
for file in "${XDG_STATE_HOME:-$HOME/.local/state}"/opencode/service*.json; do
  if [ ! -f "$file" ]; then continue; fi
  printf 'OPENCODE_SSH_REGISTRATION_BEGIN\n'
  cat "$file"
  printf '\nOPENCODE_SSH_REGISTRATION_END\n'
done
"#;

/// The CLI on PATH, the managed one, or one this app installed, then its service's registration.
pub fn discover_script() -> String {
    format!(
        "set -eu\ncli=$(command -v opencode || true)\nif [ -z \"$cli\" ] && [ -x \"$HOME/.opencode/bin/opencode\" ]; then cli=\"$HOME/.opencode/bin/opencode\"; fi\nif [ -z \"$cli\" ]; then\n  for binary in \"$HOME\"/'.opencode/desktop-ssh'/'0.0.0-beta-'*/opencode; do\n    if [ -x \"$binary\" ]; then cli=\"$binary\"; fi\n  done\nfi\nif [ -n \"$cli\" ]; then printf '%s\\n' \"$cli\"; fi\nif [ -z \"$cli\" ]; then exit 0; fi\n{REGISTRATION}"
    )
}

pub fn valid_version(version: &str) -> bool {
    version == "local" || (version.starts_with(|c: char| c.is_ascii_digit()) && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)))
}

pub fn binary_path(version: &str) -> String {
    format!("$HOME/.opencode/desktop-ssh/{version}/opencode")
}

pub fn start_script(version: &str, replace: bool) -> String {
    format!("set -eu\ncli=\"{}\"\n\"$cli\" service {}\n{REGISTRATION}", binary_path(version), if replace { "restart" } else { "start" })
}

pub fn version_script(command: &str) -> String {
    format!("if [ -x {command} ]; then {command} --version 2>/dev/null || true; fi\n")
}

pub const PROBE_SCRIPT: &str = "set -eu\nos=$(uname -s | tr '[:upper:]' '[:lower:]')\narch=$(uname -m)\ncase \"$os\" in linux|darwin) ;; *) exit 2 ;; esac\ncase \"$arch\" in x86_64|amd64) arch=x64 ;; aarch64|arm64) arch=arm64 ;; *) exit 2 ;; esac\ntarget=\"$os-$arch\"\nif [ \"$arch\" = x64 ]; then target=\"$target-baseline\"; fi\nif [ \"$os\" = linux ]; then\n  if [ -f /etc/alpine-release ] || (ldd --version 2>&1 | grep -qi musl); then target=\"$target-musl\"; fi\nfi\nprintf 'OPENCODE_REMOTE_TARGET=%s\\n' \"$target\"\n";

pub fn archive_url(target: &str, version: &str) -> Option<String> {
    let ok = ["linux-x64-baseline", "linux-arm64", "darwin-x64-baseline", "darwin-arm64", "linux-x64-baseline-musl", "linux-arm64-musl"].contains(&target);
    ok.then(|| format!("https://registry.npmjs.org/@opencode/cli-{target}/-/cli-{target}-{version}.tgz"))
}

/// Installs the CLI from `url` into this app's own folder on the host, checking its version.
pub fn install_script(version: &str, url: &str) -> String {
    let destination = quote(&format!(".opencode/desktop-ssh/{version}/opencode"));
    format!(
        "set -eu\numask 077\ndestination=\"$HOME\"/{destination}\nmkdir -p \"$(dirname \"$destination\")\"\nstage=$(mktemp -d \"$(dirname \"$destination\")/.install-XXXXXX\")\ntrap 'rm -rf \"$stage\"' EXIT\nurl={}\nif command -v curl >/dev/null 2>&1; then\n  curl -fsSL --connect-timeout 15 --max-time 180 \"$url\" -o \"$stage/archive.tgz\"\nelse\n  wget -T 180 -O \"$stage/archive.tgz\" \"$url\"\nfi\ntar -xzf \"$stage/archive.tgz\" -C \"$stage\"\nchmod 755 \"$stage/package/bin/opencode\"\ntest \"$(\"$stage/package/bin/opencode\" --version | awk '{{print $NF}}' | sed 's/^v//')\" = {}\nmv \"$stage/package/bin/opencode\" \"$destination\"\n",
        quote(url),
        quote(version)
    )
}

pub fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(match line.rfind(" v") {
        Some(at) => line[at + 2..].to_owned(),
        None => line.to_owned(),
    })
}

/// The service's registration the scripts print, when its URL is the one its status names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub url: String,
    pub password: String,
    pub version: String,
}

pub fn parse_registration(output: &str) -> Option<Registration> {
    let status = output.lines().rev().find_map(|l| l.strip_prefix("OPENCODE_SSH_STATUS="))?.trim().to_owned();
    let mut rest = output;
    while let Some(start) = rest.find("OPENCODE_SSH_REGISTRATION_BEGIN") {
        let body = &rest[start + "OPENCODE_SSH_REGISTRATION_BEGIN".len()..];
        let Some(end) = body.find("OPENCODE_SSH_REGISTRATION_END") else { break };
        let v: serde_json::Value = serde_json::from_str(body[..end].trim()).unwrap_or_default();
        let s = |k: &str| v[k].as_str().map(str::to_owned);
        if let (Some(url), Some(password), Some(version)) = (s("url"), s("password"), s("version"))
            && url == status
            && v["pid"].as_i64().is_some_and(|p| p > 0)
        {
            return Some(Registration { url, password, version });
        }
        rest = &body[end..];
    }
    None
}

/// The service's loopback address on the host, to forward to.
pub fn remote_address(url: &str) -> Option<(String, u16)> {
    let rest = url.strip_prefix("http://")?;
    let hostport = rest.split('/').next()?;
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || hostport.starts_with('[') => (h, p.parse().ok()?),
        _ => (hostport, 80),
    };
    if !["127.0.0.1", "localhost", "0.0.0.0", "[::]", "[::1]"].contains(&host) {
        return None;
    }
    Some((if host == "[::1]" { "[::1]".into() } else { "127.0.0.1".into() }, port))
}

/// `sshHostname`: the host a target ends with, for display.
pub fn hostname(target: &str) -> String {
    let last = target.split_whitespace().last().unwrap_or_default().replace(['"', '\'', '\\'], "");
    last.rsplit('@').next().unwrap_or_default().to_owned()
}
