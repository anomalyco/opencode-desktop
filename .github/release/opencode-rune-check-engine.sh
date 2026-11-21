#!/usr/bin/env bash
# opencode-rune-check-engine.sh: validates the published browser engine index at
# https://anoma.ly/release/opencode-rune/browser-engine/stable.json.
#
# Modes:
#   --plan-state --version <cef-rev>
#       Prints `complete=true|false` and `linux_x64_sha256=<64-hex or empty>`.
#       Fails hard on network, HTTP, or JSON schema errors.
#   --assert-not-complete --version <cef-rev>
#       Fails if <cef-rev> already has all four targets in the live index
#       (refuses overwriting a complete engine version with new archive hashes).
#   --assert-published --version <cef-rev> [--linux-sha256 <64-hex>] [--source-rs <path>]
#       Verifies the live index serves <cef-rev> exactly, all four targets are
#       present and reachable (HEAD HTTP 200 + matching Content-Length), the
#       linux-x64 sha256 matches --linux-sha256, and --source-rs points at the
#       same hardcoded index URL.
#
# Testing only: --index-url http://127.0.0.1:<port>/... overrides the hardcoded
# URL strictly for loopback fixtures; release.yml never passes --index-url.
. "$(dirname "$0")/lib.sh"

readonly DEFAULT_INDEX_URL="https://anoma.ly/release/opencode-rune/browser-engine/stable.json"
readonly EXPECTED_WIN_TRUST="publisher:Anomaly Innovations, Inc https://anoma.ly/"
readonly EXPECTED_MAC_TRUST="publisher:5NZ4Q7NXJ4"
readonly ENGINE_TARGETS=(windows-x64 windows-arm64 macos-arm64 linux-x64)

for cmd in curl jq sed awk; do
  command -v "$cmd" > /dev/null || die "opencode-rune-check-engine.sh needs $cmd."
done

mode=""
version=""
linux_sha256=""
linux_sha256_set=false
source_rs=""
index_url="$DEFAULT_INDEX_URL"

while [ $# -gt 0 ]; do
  case "$1" in
    --plan-state | --assert-not-complete | --assert-published)
      [ -z "$mode" ] || die "Specify only one mode."
      mode=$1
      shift
      ;;
    --version)
      [ $# -ge 2 ] || die "--version requires a value."
      version=$2
      shift 2
      ;;
    --linux-sha256)
      [ $# -ge 2 ] || die "--linux-sha256 requires a value."
      [[ "$2" =~ ^[0-9a-f]{64}$ ]] || die "Invalid --linux-sha256 '$2' (expected 64 lowercase hex characters)."
      linux_sha256=$2
      linux_sha256_set=true
      shift 2
      ;;
    --source-rs)
      [ $# -ge 2 ] || die "--source-rs requires a value."
      source_rs=$2
      shift 2
      ;;
    --index-url)
      [ $# -ge 2 ] || die "--index-url requires a value."
      case "$2" in
        http://127.0.0.1:*/*) index_url=$2 ;;
        *) die "--index-url only accepts loopback http://127.0.0.1:<port>/... for local fixture tests." ;;
      esac
      shift 2
      ;;
    *)
      die "Unknown argument: $1"
      ;;
  esac
done

[ -n "$mode" ] || die "Usage: opencode-rune-check-engine.sh --plan-state|--assert-not-complete|--assert-published --version <cef-rev> ..."
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-[0-9]+$ ]] || die "Invalid engine release version '$version' (expected X.Y.Z-R)."
if [ "$mode" = --assert-published ]; then
  [ "$linux_sha256_set" = true ] || die "--assert-published requires --linux-sha256 <64-hex>."
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch_index() {
  local out="$work/index.json"
  curl -fsS --retry 3 --connect-timeout 15 --max-time 30 "$index_url" -o "$out" ||
    die "Could not fetch browser engine index $index_url."
  jq -e 'type == "object" and (.release | type == "object") and (.release.version | type == "string") and (.release.targets | type == "object")' "$out" > /dev/null ||
    die "Browser engine index $index_url is not valid release JSON."
  printf '%s' "$out"
}

target_valid() {
  local file=$1 target=$2
  jq -e --arg t "$target" '
    .release.targets[$t] as $entry
    | ($entry | type == "object")
    and ($entry.url | type == "string") and ($entry.url | length > 0)
    and ($entry.size | type == "number") and ($entry.size > 0) and ($entry.size == ($entry.size | floor))
    and ($entry.sha256 | type == "string") and ($entry.sha256 | test("^[0-9a-f]{64}$"))
  ' "$file" > /dev/null
}

index_complete() {
  local file=$1 live_version target
  live_version=$(jq -r '.release.version' "$file")
  [ "$live_version" = "$version" ] || return 1
  for target in "${ENGINE_TARGETS[@]}"; do
    target_valid "$file" "$target" || return 1
  done
  return 0
}

resolve_url() {
  local base=$1 rel=$2 origin dir
  case "$rel" in
    https://* | http://*) printf '%s' "$rel" ;;
    /*)
      origin=$(printf '%s' "$base" | sed -E 's|^([a-zA-Z]+://[^/]+).*|\1|')
      printf '%s%s' "$origin" "$rel"
      ;;
    *)
      dir=${base%/*}
      printf '%s/%s' "$dir" "$rel"
      ;;
  esac
}

check_source_rs() {
  local file=$1
  [ -f "$file" ] || die "Missing source.rs at $file."
  awk \
    -v expected="$DEFAULT_INDEX_URL" \
    -v win_trust="$EXPECTED_WIN_TRUST" \
    -v mac_trust="$EXPECTED_MAC_TRUST" \
    -v debug_loopback="http://127.0.0.1:47981/index.json" \
    '
    BEGIN {
      cfg = ""
      release_source = ""
      has_win = 0
      has_mac = 0
      has_linux = 0
      has_win_trust = 0
      has_mac_trust = 0
      err = ""
    }
    {
      line = $0
      sub(/\r$/, "", line)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
      if (line == "" || line ~ /^\/\//) next
      if (line ~ /^#\[cfg\(/) {
        cfg = line
        next
      }
      if (line ~ /^(pub[[:space:]]+)?const[[:space:]]+[A-Z0-9_]*SOURCE[[:space:]]*:[[:space:]]*&str[[:space:]]*=[[:space:]]*/) {
        name = line
        sub(/^(pub[[:space:]]+)?const[[:space:]]+/, "", name)
        sub(/[[:space:]]*:.*/, "", name)
        rhs = line
        sub(/^[^=]*=[[:space:]]*/, "", rhs)
        sub(/[[:space:]]*;[[:space:]]*$/, "", rhs)
        if (name == "SOURCE" && rhs == "match option_env!(\"OPENCODE_BROWSER_ENGINE_SOURCE\") {") {
          cfg = ""
          next
        }
        if (rhs ~ /^".*"$/) {
          val = substr(rhs, 2, length(rhs) - 2)
        } else if (rhs == "RELEASE_SOURCE" && release_source != "") {
          val = release_source
        } else {
          err = err "unsupported SOURCE expression for " name ": " rhs "\n"
          cfg = ""
          next
        }
        if (name == "RELEASE_SOURCE") {
          if (val != expected) {
            err = err "RELEASE_SOURCE is \"" val "\", expected \"" expected "\"\n"
          } else {
            release_source = val
          }
          cfg = ""
          next
        }
        if (val != "" && val != expected) {
          if (!(cfg == "#[cfg(all(windows, debug_assertions))]" && val == debug_loopback)) {
            err = err name " under " (cfg == "" ? "<unconditional>" : cfg) " is \"" val "\", expected \"" expected "\"\n"
          }
        }
        if (name == "DEV_SOURCE") {
          if (cfg == "#[cfg(all(windows, not(debug_assertions)))]") {
            if (val == expected) has_win = 1
            else err = err "windows release DEV_SOURCE is \"" val "\", expected \"" expected "\"\n"
          } else if (cfg == "#[cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))]" || cfg == "#[cfg(target_os = \"macos\")]") {
            if (val == expected) has_mac = 1
            else err = err "macos arm64 DEV_SOURCE is \"" val "\", expected \"" expected "\"\n"
          } else if (cfg == "#[cfg(all(target_os = \"linux\", target_arch = \"x86_64\"))]") {
            if (val == expected) has_linux = 1
            else err = err "linux x86_64 DEV_SOURCE is \"" val "\", expected \"" expected "\"\n"
          }
        }
        cfg = ""
        next
      }
      if (line ~ /^(pub[[:space:]]+)?const[[:space:]]+DEV_TRUST[[:space:]]*:[[:space:]]*&str[[:space:]]*=[[:space:]]*".*";$/) {
        rhs = line
        sub(/^[^=]*=[[:space:]]*"/, "", rhs)
        sub(/";$/, "", rhs)
        if (cfg == "#[cfg(all(windows, not(debug_assertions)))]") {
          if (rhs == win_trust) has_win_trust = 1
          else err = err "windows release DEV_TRUST is \"" rhs "\", expected \"" win_trust "\"\n"
        } else if (cfg == "#[cfg(all(target_os = \"macos\", target_arch = \"aarch64\"))]" || cfg == "#[cfg(target_os = \"macos\")]") {
          if (rhs == mac_trust) has_mac_trust = 1
          else err = err "macos arm64 DEV_TRUST is \"" rhs "\", expected \"" mac_trust "\"\n"
        }
        cfg = ""
        next
      }
      if (line !~ /^#\[/) cfg = ""
    }
    END {
      if (!has_win) err = err "missing windows release DEV_SOURCE == " expected "\n"
      if (!has_mac) err = err "missing macos arm64 DEV_SOURCE == " expected "\n"
      if (!has_linux) err = err "missing linux x86_64 DEV_SOURCE == " expected "\n"
      if (!has_win_trust) err = err "missing windows release DEV_TRUST == " win_trust "\n"
      if (!has_mac_trust) err = err "missing macos arm64 DEV_TRUST == " mac_trust "\n"
      if (err != "") {
        printf "%s", err > "/dev/stderr"
        exit 1
      }
    }
    ' "$file" || die "$file does not configure the required release engine defaults for Windows, macOS arm64, and Linux x86_64."
}

index_file=$(fetch_index)

case "$mode" in
  --plan-state)
    if index_complete "$index_file"; then
      linux_hash=$(jq -r '.release.targets["linux-x64"].sha256' "$index_file")
      [[ "$linux_hash" =~ ^[0-9a-f]{64}$ ]] || die "Invalid linux-x64 sha256 '$linux_hash' in $index_url."
      echo "complete=true"
      echo "linux_x64_sha256=$linux_hash"
    else
      echo "complete=false"
      echo "linux_x64_sha256="
    fi
    ;;

  --assert-not-complete)
    if index_complete "$index_file"; then
      die "Browser engine $version is already complete in $index_url; bump ENGINE_REVISION (or CEF in apps/browser-engine/Cargo.lock) to publish a new engine."
    fi
    echo "ok - $index_url does not yet have complete browser engine $version"
    ;;

  --assert-published)
    if [ -n "$source_rs" ]; then
      check_source_rs "$source_rs"
    fi
    live_version=$(jq -r '.release.version' "$index_file")
    [ "$live_version" = "$version" ] ||
      die "Browser engine index $index_url serves '$live_version', expected '$version'."
    for target in "${ENGINE_TARGETS[@]}"; do
      target_valid "$index_file" "$target" ||
        die "Browser engine index $index_url lacks a valid '$target' target for $version."
      rel_url=$(jq -r --arg t "$target" '.release.targets[$t].url' "$index_file")
      expected_size=$(jq -r --arg t "$target" '.release.targets[$t].size' "$index_file")
      resolved=$(resolve_url "$index_url" "$rel_url")
      headers="$work/head-$target.txt"
      curl -fsSI --location --retry 3 --connect-timeout 15 --max-time 30 "$resolved" -o "$headers" ||
        die "Target '$target' archive is not reachable at $resolved."
      actual_size=$(tr -d '\r' < "$headers" | awk 'BEGIN{IGNORECASE=1} /^content-length:[[:space:]]*[0-9]+[[:space:]]*$/ {v=$2} END{print v}')
      [ -n "$actual_size" ] || die "Target '$target' at $resolved returned no Content-Length header."
      [ "$actual_size" = "$expected_size" ] ||
        die "Target '$target' at $resolved has Content-Length $actual_size, expected $expected_size."
    done
    live_linux=$(jq -r '.release.targets["linux-x64"].sha256' "$index_file")
    [ "$live_linux" = "$linux_sha256" ] ||
      die "Published linux-x64 sha256 '$live_linux' does not match Linux app pin '$linux_sha256'."
    echo "ok - $index_url serves $version (${#ENGINE_TARGETS[@]} targets verified, linux-x64 sha256 matched)"
    ;;
esac
