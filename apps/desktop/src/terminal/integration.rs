//! Prompt marks for the shells that don't send them, when the user turns them on per shell
//! (Settings > General > "Mark commands in ..."): the shell starts with a script on its command
//! line that adds OSC 133 marks (prompt, command, how it ended) and reports its folder. Nothing
//! is written to the user's profile or startup files. Nushell marks its commands itself.
//!
//! | Shell | How the script gets in | Marks |
//! | --- | --- | --- |
//! | PowerShell 7, Windows PowerShell | `-NoExit -EncodedCommand <script>` | prompt, command, exit status, folder |
//! | cmd | `/S /K prompt ...`, around the user's own `PROMPT` | prompt and folder: cmd has no hook before a command, nor its exit status |
//! | Git Bash | `--init-file <file>` (which runs `~/.bashrc` first) | prompt, command, exit status, folder |
//! | WSL | `wsl.exe -e sh -c` running bash with `--init-file`, as a login shell; another login shell starts as it would | the same |
//!
//! Bash reads its script from a file, so Git Bash and WSL get marks only from a server on this
//! computer, where the app writes it (its data folder, `shell-integration/bash.sh`).

use std::path::{Path, PathBuf};

const PWSH: &str = include_str!("integration/pwsh.ps1");
const BASH: &str = include_str!("integration/bash.sh");

/// A shell the user can turn marks on for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Pwsh,
    PowerShell,
    Cmd,
    GitBash,
    Wsl,
}

impl Shell {
    pub const ALL: [Shell; 5] = [Shell::Pwsh, Shell::PowerShell, Shell::Cmd, Shell::GitBash, Shell::Wsl];

    /// Its id in the settings (`terminal.marks`).
    pub fn id(self) -> &'static str {
        match self {
            Shell::Pwsh => "pwsh",
            Shell::PowerShell => "powershell",
            Shell::Cmd => "cmd",
            Shell::GitBash => "gitbash",
            Shell::Wsl => "wsl",
        }
    }

    /// Its row's key in Settings.
    pub fn setting_key(self) -> &'static str {
        match self {
            Shell::Pwsh => "terminal-marks-pwsh",
            Shell::PowerShell => "terminal-marks-powershell",
            Shell::Cmd => "terminal-marks-cmd",
            Shell::GitBash => "terminal-marks-gitbash",
            Shell::Wsl => "terminal-marks-wsl",
        }
    }

    /// Its product name, the same in every language.
    pub fn label(self) -> &'static str {
        match self {
            Shell::Pwsh => "PowerShell 7",
            Shell::PowerShell => "Windows PowerShell",
            Shell::Cmd => "cmd",
            Shell::GitBash => "Git Bash",
            Shell::Wsl => "WSL",
        }
    }

    /// Whether its script is a file the server has to read (bash), so only a server on this
    /// computer can start it with marks.
    pub fn reads_a_file(self) -> bool {
        matches!(self, Shell::GitBash | Shell::Wsl)
    }

    /// The shell `program` (a path or a name, as the server or a plugin gives it) is. Windows'
    /// own `bash.exe` (in System32) starts WSL; any other bash on Windows is Git Bash's.
    pub fn of(program: &str) -> Option<Shell> {
        let file = program.rsplit(['\\', '/']).next().unwrap_or(program).to_ascii_lowercase();
        let stem = file.strip_suffix(".exe").unwrap_or(&file);
        match stem {
            "pwsh" => Some(Shell::Pwsh),
            "powershell" => Some(Shell::PowerShell),
            "cmd" => Some(Shell::Cmd),
            "wsl" => Some(Shell::Wsl),
            "bash" if program.to_ascii_lowercase().contains("\\system32\\") => Some(Shell::Wsl),
            "bash" => Some(Shell::GitBash),
            _ => None,
        }
    }
}

/// The command and arguments that start `program` (shell `shell`) with its marks; `None` when
/// its script is a file and there's no folder to write it to.
pub fn launch(shell: Shell, program: &str, files: Option<&Path>) -> Option<(String, Vec<String>)> {
    match shell {
        Shell::Pwsh | Shell::PowerShell => {
            let utf16: Vec<u8> = PWSH.encode_utf16().flat_map(u16::to_le_bytes).collect();
            Some((program.to_owned(), vec!["-NoExit".into(), "-EncodedCommand".into(), base64(&utf16)]))
        }
        // `PROMPT` is the user's prompt (cmd sets `$P$G` when there's none), kept between the marks.
        Shell::Cmd => Some((
            program.to_owned(),
            vec![
                "/S".into(),
                "/K".into(),
                "if defined PROMPT (prompt $e]9;9;$P$e\\$e]133;A$e\\%PROMPT%$e]133;B$e\\) else (prompt $e]9;9;$P$e\\$e]133;A$e\\$P$G$e]133;B$e\\)".into(),
            ],
        )),
        Shell::GitBash => {
            let file = script_file(files?)?;
            Some((program.to_owned(), vec!["--init-file".into(), file.to_string_lossy().replace('\\', "/")]))
        }
        Shell::Wsl => {
            let file = script_file(files?)?.to_string_lossy().replace('\\', "/");
            // `wsl.exe` runs the user's login shell; when that's bash, the same bash with the script.
            let wsl = if program.to_ascii_lowercase().ends_with("bash.exe") { "wsl.exe".to_owned() } else { program.to_owned() };
            let sh = format!(
                "s=$(getent passwd \"$(id -un)\" | cut -d: -f7); case \"$s\" in */bash) OC_LOGIN=1 exec bash --init-file \"$(wslpath -u '{file}')\" ;; *) exec \"${{s:-sh}}\" -l ;; esac"
            );
            Some((wsl, vec!["-e".into(), "sh".into(), "-c".into(), sh]))
        }
    }
}

/// `WSLENV` with `COLORTERM` added, so a WSL shell started from a terminal sees the colours the
/// terminal offers; Windows passes only `TERM` across by itself.
pub fn wslenv(current: &str) -> String {
    let has = current.split(':').any(|v| v.split('/').next() == Some("COLORTERM"));
    match (has, current.is_empty()) {
        (true, _) => current.to_owned(),
        (false, true) => "COLORTERM".to_owned(),
        (false, false) => format!("{current}:COLORTERM"),
    }
}

/// The bash script in `dir`, written when it's missing or different (with LF line ends, which
/// bash needs).
fn script_file(dir: &Path) -> Option<PathBuf> {
    let file = dir.join("bash.sh");
    let body = BASH.replace('\r', "");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(body.as_str()) {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&file, body).ok()?;
    }
    Some(file)
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= chunk.len() { char::from(ABC[(n >> (18 - 6 * i)) as usize & 63]) } else { '=' });
        }
    }
    out
}
