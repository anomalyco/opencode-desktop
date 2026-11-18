#!/usr/bin/env bash
# Zero-thought feature scaffolder for rune-desktop (Lane A Luau plugin or Lane B Rust feature).
#
# Usage:
#   scripts/new-feature.sh plugin <id> [Title]   # Lane A (default): Luau plugin under plugins/<id>/
#   scripts/new-feature.sh feature <id> [Title]  # Lane B: Rust feature crate under features/<id>/
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage:
  scripts/new-feature.sh plugin <id> [Title]   # Lane A (default): Luau plugin under plugins/<id>/
  scripts/new-feature.sh feature <id> [Title]  # Lane B: Rust feature crate under features/<id>/

Rules:
  - <id> must be lowercase kebab-case (^[a-z][a-z0-9-]*$, max 64 chars)
  - Existing target directories are never overwritten
EOF
}

capitalize() {
  local word="$1"
  local first rest
  first="$(printf '%s' "${word:0:1}" | LC_ALL=C tr '[:lower:]' '[:upper:]')"
  rest="${word:1}"
  printf '%s%s' "$first" "$rest"
}

escape_quoted() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

if [ "$#" -lt 1 ]; then
  usage >&2
  exit 1
fi

case "$1" in
  -h|--help|help)
    usage
    exit 0
    ;;
  plugin|feature)
    LANE="$1"
    ;;
  *)
    echo "error: unknown lane '$1' (expected 'plugin' or 'feature')" >&2
    usage >&2
    exit 1
    ;;
esac

if [ "$#" -lt 2 ]; then
  echo "error: missing <id> for lane '$LANE'" >&2
  usage >&2
  exit 1
fi

ID="$2"
shift 2

if ! (LC_ALL=C; [[ "$ID" =~ ^[a-z][a-z0-9-]*$ ]]) || [[ "$ID" == *- ]] || [[ "$ID" == *--* ]]; then
  echo "error: id '$ID' must be lowercase kebab-case (^[a-z][a-z0-9-]*$, no trailing or repeated hyphens)" >&2
  exit 1
fi

if [ "${#ID}" -gt 64 ]; then
  echo "error: id '$ID' exceeds 64 characters" >&2
  exit 1
fi

IFS='-' read -r -a PARTS <<< "$ID"
PASCAL=""
DEFAULT_TITLE=""
for part in "${PARTS[@]}"; do
  cap="$(capitalize "$part")"
  PASCAL+="$cap"
  if [ -n "$DEFAULT_TITLE" ]; then
    DEFAULT_TITLE+=" "
  fi
  DEFAULT_TITLE+="$cap"
done

SNAKE="${ID//-/_}"
TITLE="${*:-$DEFAULT_TITLE}"
TITLE_ESC="$(escape_quoted "$TITLE")"

if [ "$LANE" = "plugin" ]; then
  TARGET="$ROOT/plugins/$ID"
  if [ -e "$TARGET" ]; then
    echo "error: '$TARGET' already exists; refusing to overwrite" >&2
    exit 1
  fi

  mkdir -p "$TARGET/locales"

  cat <<EOF > "$TARGET/plugin.toml"
#:schema ../plugin.schema.json
id = "${ID}"
name = "${TITLE_ESC}"
version = "0.1.0"
contract = "1.4"
description = "${TITLE_ESC} panel and command"
EOF

  cat <<EOF > "$TARGET/locales/en.json"
{
  "title": "${TITLE_ESC}",
  "description": "Open the ${TITLE_ESC} panel",
  "category.view": "View",
  "command.open": "Open ${TITLE_ESC}",
  "status.count": "Clicks: {{count}}",
  "action.increment": "Increment"
}
EOF

  cat <<EOF > "$TARGET/init.luau"
--!strict
-- ${TITLE} plugin (\`plugins/${ID}\`): Lane A UI feature hot-reloaded under \`--plugin-hot\`.

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
            local version = \`{changed:Version()}:{I18n:Locale().id}:{Theme:Current().id}:{props.sessionId}\`
            return view:Memo(\`${ID}-panel-{props.sessionId}\`, version, function()
                return ui.col({ key = "${ID}-root", w = "full", h = "full", pad = 16, gap = 12 }, {
                    ui.text(I18n:T("title"), { size = 15, weight = "semibold", color = "text-strong" }),
                    ui.text(I18n:T("status.count", { count = count }), { size = 13, color = "text-weak" }),
                    ui.row({ key = "${ID}-actions", gap = 8 }, {
                        ui.button({
                            key = "${ID}-increment",
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
EOF

  cat <<EOF
Created Lane A Luau plugin:
  plugins/${ID}/plugin.toml
  plugins/${ID}/init.luau
  plugins/${ID}/locales/en.json

Next step (0 ms Rust rebuild; hot reloads on save):
  scripts/dev-fast.sh
EOF
  exit 0
fi

TARGET="$ROOT/features/$ID"
if [ -e "$TARGET" ]; then
  echo "error: '$TARGET' already exists; refusing to overwrite" >&2
  exit 1
fi

mkdir -p "$TARGET/src"

cat <<EOF > "$TARGET/Cargo.toml"
[package]
name = "opencode-feature-${ID}"
version = "0.0.1"
description = "${TITLE_ESC} feature crate for OpenCode (Rune)"
edition.workspace = true
license.workspace = true
publish = false

[dependencies]
opencode-kernel = { path = "../../crates/opencode-kernel" }
rune.workspace = true

[lints]
workspace = true
EOF

cat <<EOF > "$TARGET/src/lib.rs"
//! ${TITLE} feature crate (\`features/${ID}\`).
//!
//! Keep the host boundary object-safe (\`dyn ${PASCAL}Host\`) so \`apps/desktop\` and \`apps/web\`
//! do not re-monomorphize this crate's codegen, and keep per-window state in
//! \`opencode_kernel::StateSlots\` instead of adding fields to \`struct App\`.

#![forbid(unsafe_code)]

use opencode_kernel::{Feature, KernelBuilder, Phase};

/// Explicit state machine phase for ${TITLE}.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ${PASCAL}Phase {
    #[default]
    Idle,
    NeedsRedraw,
}

/// Per-window state stored in \`opencode_kernel::StateSlots\`.
#[derive(Debug, Default)]
pub struct ${PASCAL}State {
    pub phase: ${PASCAL}Phase,
}

/// Object-safe host boundary implemented by the shell (\`apps/desktop\` / \`apps/web\`).
pub trait ${PASCAL}Host: opencode_kernel::AppEnv + opencode_kernel::AppFeedback + opencode_kernel::AppOverlays {
    fn state(&self) -> Option<&${PASCAL}State>;
    fn state_mut(&mut self) -> &mut ${PASCAL}State;
}

/// Non-generic per-frame poll wired into \`opencode_kernel::Phase::Ingest\`.
///
/// Returns \`true\` when state changed and the window should redraw.
pub fn poll(host: &mut dyn ${PASCAL}Host, tx: &mut rune::Tx) -> bool {
    let state = host.state_mut();
    match state.phase {
        ${PASCAL}Phase::Idle => false,
        ${PASCAL}Phase::NeedsRedraw => {
            state.phase = ${PASCAL}Phase::Idle;
            tx.instant();
            true
        }
    }
}

/// Kernel feature registration for ${TITLE}.
pub struct ${PASCAL}Feature;

impl<S: ${PASCAL}Host> Feature<S> for ${PASCAL}Feature {
    fn id(&self) -> &'static str {
        "feature.${ID}"
    }

    fn register(&self, builder: &mut KernelBuilder<S>) {
        builder.insert_slot(${PASCAL}State::default());
        builder.add_system(Phase::Ingest, "${ID}.poll", |host, _, tx| poll(host, tx));
    }
}
EOF

cat <<EOF
Created Lane B Rust feature crate:
  features/${ID}/Cargo.toml
  features/${ID}/src/lib.rs

Next steps (wire into the workspace):
  1. Cargo.toml ([workspace] members):
       "features/${ID}",
  2. apps/desktop/Cargo.toml and apps/web/Cargo.toml (dependencies):
       opencode-feature-${ID} = { path = "../../features/${ID}" }
  3. apps/desktop/src/kernel.rs (inside build(), after implementing ${PASCAL}Host for App in apps/desktop/src/surfaces/${SNAKE}.rs):
       b.with_feature(&opencode_feature_${SNAKE}::${PASCAL}Feature);
EOF
