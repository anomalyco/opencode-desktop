<#
.SYNOPSIS
  Zero-thought feature scaffolder for rune-desktop (Lane A Luau plugin or Lane B Rust feature).

.DESCRIPTION
  Creates a ready-to-run Lane A Luau plugin under plugins/<id>/ (default) or a decoupled
  Lane B Rust feature crate under features/<id>/ matching workspace conventions.

.EXAMPLE
  pwsh scripts/new-feature.ps1 plugin my-panel "My Panel"
  pwsh scripts/new-feature.ps1 feature my-engine "My Engine"
#>
param(
  [Parameter(Position = 0)]
  [string]$Lane,
  [Parameter(Position = 1)]
  [string]$Id,
  [Parameter(Position = 2, ValueFromRemainingArguments = $true)]
  [string[]]$TitleWords,
  [Alias('h')]
  [switch]$Help
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Show-Usage {
  $text = @'
Usage:
  pwsh scripts/new-feature.ps1 plugin <id> [Title]   # Lane A (default): Luau plugin under plugins/<id>/
  pwsh scripts/new-feature.ps1 feature <id> [Title]  # Lane B: Rust feature crate under features/<id>/

Rules:
  - <id> must be lowercase kebab-case (^[a-z][a-z0-9-]*$, max 64 chars)
  - Existing target directories are never overwritten
'@
  Write-Host $text
}

function Write-Utf8Lf {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [Parameter(Mandatory = $true)]
    [string]$Content
  )
  $normalized = $Content -replace "`r`n", "`n"
  if (-not $normalized.EndsWith("`n")) {
    $normalized += "`n"
  }
  $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
  [System.IO.File]::WriteAllText($Path, $normalized, $utf8NoBom)
}

function Expand-Template {
  param(
    [string]$Template,
    [hashtable]$Tokens
  )
  $result = $Template
  foreach ($key in $Tokens.Keys) {
    $result = $result.Replace($key, [string]$Tokens[$key])
  }
  return $result
}

if ($Help -or ($Lane -in @('-h', '--help', 'help')) -or ([string]::IsNullOrEmpty($Lane) -and ($TitleWords -contains '--help' -or $TitleWords -contains '-h'))) {
  Show-Usage
  exit 0
}

if ([string]::IsNullOrEmpty($Lane)) {
  Show-Usage
  exit 1
}

if ($Lane -cne 'plugin' -and $Lane -cne 'feature') {
  [Console]::Error.WriteLine("error: unknown lane '$Lane' (expected 'plugin' or 'feature')")
  Show-Usage
  exit 1
}

if ([string]::IsNullOrEmpty($Id)) {
  [Console]::Error.WriteLine("error: missing <id> for lane '$Lane'")
  Show-Usage
  exit 1
}

if (($Id -cnotmatch '^[a-z][a-z0-9-]*$') -or $Id.EndsWith('-') -or $Id.Contains('--')) {
  [Console]::Error.WriteLine("error: id '$Id' must be lowercase kebab-case (^[a-z][a-z0-9-]*$, no trailing or repeated hyphens)")
  exit 1
}

if ($Id.Length -gt 64) {
  [Console]::Error.WriteLine("error: id '$Id' exceeds 64 characters")
  exit 1
}

$parts = $Id.Split('-')
$capParts = foreach ($part in $parts) {
  $part.Substring(0, 1).ToUpperInvariant() + $part.Substring(1)
}
$pascal = ($capParts -join '')
$defaultTitle = ($capParts -join ' ')
$snake = $Id.Replace('-', '_')

$title = if ($TitleWords -and $TitleWords.Count -gt 0) {
  ($TitleWords -join ' ').Trim()
} else {
  $defaultTitle
}
if ([string]::IsNullOrWhiteSpace($title)) {
  $title = $defaultTitle
}
$titleEsc = $title.Replace('\', '\\').Replace('"', '\"')

$tokens = @{
  '__ID__'        = $Id
  '__SNAKE__'     = $snake
  '__PASCAL__'    = $pascal
  '__TITLE__'     = $title
  '__TITLE_ESC__' = $titleEsc
}

if ($Lane -ceq 'plugin') {
  $target = Join-Path (Join-Path $root 'plugins') $Id
  if (Test-Path -LiteralPath $target) {
    [Console]::Error.WriteLine("error: '$target' already exists; refusing to overwrite")
    exit 1
  }

  $localesDir = Join-Path $target 'locales'
  [void](New-Item -ItemType Directory -Path $localesDir -Force)

  $pluginToml = @'
#:schema ../plugin.schema.json
id = "__ID__"
name = "__TITLE_ESC__"
version = "0.1.0"
contract = "1.4"
description = "__TITLE_ESC__ panel and command"
'@

  $enJson = @'
{
  "title": "__TITLE_ESC__",
  "description": "Open the __TITLE_ESC__ panel",
  "category.view": "View",
  "command.open": "Open __TITLE_ESC__",
  "status.count": "Clicks: {{count}}",
  "action.increment": "Increment"
}
'@

  $initLuau = @'
--!strict
-- __TITLE__ plugin (`plugins/__ID__`): Lane A UI feature hot-reloaded under `--plugin-hot`.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")

local PANEL = "main"

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Layout = ctx:GetService("Layout")
    local Theme = ctx:GetService("Theme")
    local changed = ctx:CreateSignal()
    local count = 0

    ctx:BindHotState(function()
        return { count = count }
    end, function(saved)
        if type(saved) == "table" and type(saved.count) == "number" then
            count = saved.count
        end
    end)

    ctx:AddCommand({
        id = "open",
        title = I18n:T("command.open"),
        description = I18n:T("description"),
        group = I18n:T("category.view"),
        section = "session",
        requiresSession = true,
        run = function()
            Layout:OpenPanel(PANEL)
            return nil
        end,
    })

    ctx:AddPanel({
        id = PANEL,
        title = I18n:T("title"),
        description = I18n:T("description"),
        icon = "info",
        render = function(view, props)
            local version = `{changed:Version()}:{I18n:Locale().id}:{Theme:Current().id}:{props.sessionId}`
            return view:Memo(`__ID__-panel-{props.sessionId}`, version, function()
                return ui.col({ key = "__ID__-root", w = "full", h = "full", pad = 16, gap = 12 }, {
                    ui.text(I18n:T("title"), { size = 15, weight = "semibold", color = "text-strong" }),
                    ui.text(I18n:T("status.count", { count = count }), { size = 13, color = "text-weak" }),
                    ui.row({ key = "__ID__-actions", gap = 8 }, {
                        ui.button({
                            key = "__ID__-increment",
                            text = I18n:T("action.increment"),
                            variant = "primary",
                            size = "small",
                            onPress = view:Handler(function()
                                count += 1
                                changed:Fire()
                            end),
                        }),
                    }),
                })
            end)
        end,
    })
end)
'@

  Write-Utf8Lf -Path (Join-Path $target 'plugin.toml') -Content (Expand-Template $pluginToml $tokens)
  Write-Utf8Lf -Path (Join-Path $localesDir 'en.json') -Content (Expand-Template $enJson $tokens)
  Write-Utf8Lf -Path (Join-Path $target 'init.luau') -Content (Expand-Template $initLuau $tokens)

  Write-Host "Created Lane A Luau plugin:"
  Write-Host "  plugins/$Id/plugin.toml"
  Write-Host "  plugins/$Id/init.luau"
  Write-Host "  plugins/$Id/locales/en.json"
  Write-Host ""
  Write-Host "Next step (0 ms Rust rebuild; hot reloads on save):"
  Write-Host "  pwsh scripts/dev-fast.ps1"
  exit 0
}

$target = Join-Path (Join-Path $root 'features') $Id
if (Test-Path -LiteralPath $target) {
  [Console]::Error.WriteLine("error: '$target' already exists; refusing to overwrite")
  exit 1
}

$srcDir = Join-Path $target 'src'
[void](New-Item -ItemType Directory -Path $srcDir -Force)

$cargoToml = @'
[package]
name = "opencode-feature-__ID__"
version = "0.0.1"
description = "__TITLE_ESC__ feature crate for OpenCode (Rune)"
edition.workspace = true
license.workspace = true
publish = false

[dependencies]
opencode-kernel = { path = "../../crates/opencode-kernel" }
rune.workspace = true

[lints]
workspace = true
'@

$libRs = @'
//! __TITLE__ feature crate (`features/__ID__`).
//!
//! Keep the host boundary object-safe (`dyn __PASCAL__Host`) so `apps/desktop` and `apps/web`
//! do not re-monomorphize this crate's codegen, and keep per-window state in
//! `opencode_kernel::StateSlots` instead of adding fields to `struct App`.

#![forbid(unsafe_code)]

use opencode_kernel::{Feature, KernelBuilder, Phase};

/// Explicit state machine phase for __TITLE__.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum __PASCAL__Phase {
    #[default]
    Idle,
    NeedsRedraw,
}

/// Per-window state stored in `opencode_kernel::StateSlots`.
#[derive(Debug, Default)]
pub struct __PASCAL__State {
    pub phase: __PASCAL__Phase,
}

/// Object-safe host boundary implemented by the shell (`apps/desktop` / `apps/web`).
pub trait __PASCAL__Host: opencode_kernel::AppEnv + opencode_kernel::AppFeedback + opencode_kernel::AppOverlays {
    fn state(&self) -> Option<&__PASCAL__State>;
    fn state_mut(&mut self) -> &mut __PASCAL__State;
}

/// Non-generic per-frame poll wired into `opencode_kernel::Phase::Ingest`.
///
/// Returns `true` when state changed and the window should redraw.
pub fn poll(host: &mut dyn __PASCAL__Host, tx: &mut rune::Tx) -> bool {
    let state = host.state_mut();
    match state.phase {
        __PASCAL__Phase::Idle => false,
        __PASCAL__Phase::NeedsRedraw => {
            state.phase = __PASCAL__Phase::Idle;
            tx.instant();
            true
        }
    }
}

/// Kernel feature registration for __TITLE__.
pub struct __PASCAL__Feature;

impl<S: __PASCAL__Host> Feature<S> for __PASCAL__Feature {
    fn id(&self) -> &'static str {
        "feature.__ID__"
    }

    fn register(&self, builder: &mut KernelBuilder<S>) {
        builder.insert_slot(__PASCAL__State::default());
        builder.add_system(Phase::Ingest, "__ID__.poll", |host, _, tx| poll(host, tx));
        // Modal dialogs can self-register without editing shell view/keys/escape:
        // builder.register_overlay(opencode_kernel::OverlayRegistration::dialog(ID, render, Some(on_key), Some(on_dismiss)));
    }
}
'@

Write-Utf8Lf -Path (Join-Path $target 'Cargo.toml') -Content (Expand-Template $cargoToml $tokens)
Write-Utf8Lf -Path (Join-Path $srcDir 'lib.rs') -Content (Expand-Template $libRs $tokens)

Write-Host "Created Lane B Rust feature crate:"
Write-Host "  features/$Id/Cargo.toml"
Write-Host "  features/$Id/src/lib.rs"
Write-Host ""
Write-Host "Next steps (wire into the workspace):"
Write-Host "  1. Cargo.toml ([workspace] members):"
Write-Host "       `"features/$Id`","
Write-Host "  2. apps/desktop/Cargo.toml (dependencies):"
Write-Host "       opencode-feature-$Id = { path = `"../../features/$Id`" }"
Write-Host "  3. apps/desktop/src/kernel.rs (inside build(), after implementing ${pascal}Host for App in apps/desktop/src/surfaces/$snake.rs):"
Write-Host "       b.with_feature(&opencode_feature_${snake}::${pascal}Feature);"
