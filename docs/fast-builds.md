# Fast Builds

Keep local and mesh compile times, RAM usage, and `target/` churn low without changing production release quality.

## 1. Fastest loop (0 s Rust compile)

~80% of UI features belong in `plugins/*` (Luau). Pass `--plugin-hot` to watch plugin folders and reload edits live in a running app — works on both dev and prebuilt release binaries.

```sh
# Launch once (or use scripts/dev-fast.{sh,ps1}, which pass --plugin-hot)
opencode-desktop --live --plugin-hot
```

**Across mesh devices:** build once (or pull a dev build), launch with `--plugin-hot`, and sync only `plugins/` across machines. Zero Rust compiles on remote devices.

## 2. Fast Rust dev loop

Skip Vello, BetterOffice, and ~6 MB of Office font packs (`--no-default-features --features knobs`), skip DWARF/PDB emission (`--profile dev-fast`), and keep Luau hot reload on:

```sh
# macOS / Linux
scripts/dev-fast.sh
scripts/dev-fast.sh --server http://127.0.0.1:4096
OPENCODE_DEV_OFFICE=1 scripts/dev-fast.sh          # opt back into Word/Excel/PPT previews
```

```powershell
# Windows (pwsh)
pwsh scripts/dev-fast.ps1
pwsh scripts/dev-fast.ps1 --server http://127.0.0.1:4096
pwsh scripts/dev-fast.ps1 -Office                  # opt back into Word/Excel/PPT previews
```

Dev dependencies compile at `opt-level = 1` with `debug = false`, keeping full `opt-level = 3` only on the 8 frame-hot crates (`project-icon`, `rune`, `parley_engine`, `image`, `flate2`, `crc32fast`, `opencode-search`, `memchr`).

## 3. Profile matrix

| Profile | Key settings | Peak RAM (`rustc`) | Clean build | Use case |
| --- | --- | --- | --- | --- |
| `dev` | `opt=0` workspace, `debug=2`, `opt=1` deps (`-O3` on 8 hot crates) | ~2–4 GB | ~1–3 min | Debugger (`lldb`/WinDbg), full stack traces |
| `dev-fast` | `inherits = "dev"`, `debug=0`, `codegen-units=64` | ~1.5–3 GB | ~1–2 min (fast relink) | Default local & mesh Rust iteration (`scripts/dev-fast.*`) |
| `rune-hot` | `inherits = "dev"`, `debug=0`, `incremental=true` | ~1.5–3 GB | Sub-second patch | Live-patching Rune views via `apps/desktop/scripts/hot.sh` |
| `release-quick` | `opt=3`, `lto=false`, `codegen-units=16`, `incremental=false` | ~3–5 GB | ~2–5 min | Fast optimized builds, PR/Linux packaging, quick perf checks |
| `release-thin` | `opt=3`, `lto="thin"`, `codegen-units=16`, `incremental=false` | ~4–6 GB | ~4–10 min | Near-release speed for perf checks & mesh dogfooding |
| `release` | `opt=3`, `lto="fat"`, `codegen-units=1`, `incremental=false` | ~10 GB | ~25–60 min | Shipped binaries & PGO timing (`apps/desktop/scripts/release.ps1`) |

> For native profiler symbols without fat-LTO cost, use `--profile perf` (`inherits = "release-quick"`, `debug = 1`).

## 4. Mesh-wide build caching, fast linkers, and disk cleanup

Configure these in your **user** `~/.cargo/config.toml` or environment — do **not** edit the repo's `.cargo/config.toml` (it pins Windows `crt-static`, WASM SIMD, and `LIBSQLITE3_FLAGS`).

### Shared `sccache` (Windows / macOS / Linux)

Install once per device (`cargo install sccache --locked`, `brew install sccache`, or `winget install Mozilla.sccache`), then enable in `~/.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "sccache"
```

Point every mesh device at one shared cache backend:

| Backend | Required env vars |
| --- | --- |
| **Cloudflare R2 / S3** | `SCCACHE_BUCKET`, `SCCACHE_ENDPOINT`, `SCCACHE_REGION=auto`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` |
| **LAN WebDAV** | `SCCACHE_WEBDAV_ENDPOINT` (optional `SCCACHE_WEBDAV_USERNAME`, `SCCACHE_WEBDAV_PASSWORD`) |
| **LAN Redis** | `SCCACHE_REDIS=redis://<lan-host>:6379` |

```sh
sccache --show-stats
```

External crates never build incrementally, so `sccache` shares the full dependency graph even during incremental dev builds. For cold CI/mesh pre-warm runs, set `CARGO_INCREMENTAL=0` so workspace crates populate the cache too.

### Fast linkers

Add the matching target block to **`~/.cargo/config.toml`**:

| OS | Linker | Install | `~/.cargo/config.toml` |
| --- | --- | --- | --- |
| **Windows** | `lld-link` | `winget install LLVM.LLVM` (or VS LLVM component) | `[target.x86_64-pc-windows-msvc]`<br>`linker = "lld-link"` |
| **Linux** | `lld` or `wild` | `sudo apt install lld clang` or `cargo install --locked wild-linker` | `[target.x86_64-unknown-linux-gnu]`<br>`linker = "clang"`<br>`rustflags = ["-C", "link-arg=-fuse-ld=lld"]` |
| **macOS** | `ld_prime` (default) / `lld` | Built into Xcode 15+ (`brew install lld` optional) | Default is fast; optional `rustflags = ["-C", "link-arg=-fuse-ld=lld"]` |

For `wild` on Linux, swap the flag to `rustflags = ["-C", "link-arg=--ld-path=wild"]`. On Windows, **never** set a global `RUSTFLAGS` env var — it overrides `.cargo/config.toml`'s `+crt-static` flags; `linker = "lld-link"` preserves them.

### Reclaim `target/` disk space

```sh
cargo install cargo-sweep --locked
cargo sweep --time 14          # prune build artifacts untouched for 14+ days
cargo sweep --installed        # prune artifacts from old rustc toolchains
```

## 5. Non-generic (`dyn`) crate boundaries

Keep `features/*` entrypoints non-generic (`&mut dyn Host` or concrete data) rather than generic over the host type (`<H: Host>`):

| Boundary style | Where LLVM compiles feature code | Rebuild cost on `apps/desktop` edit |
| --- | --- | --- |
| Generic (`fn view<H: Host>(h: &mut H)`) | Deferred into `apps/desktop` (re-monomorphized per host) | Re-optimizes feature codegen inside the final crate; breaks parallel leaf caching |
| Dynamic (`fn view(h: &mut dyn Host)`) | Once inside `features/<name>` (`.rlib`) | Leaf crate stays cached (`sccache` + Cargo); `apps/desktop` compiles only the thin adapter |

A vtable call at a feature/surface boundary costs ~1 ns against a multi-millisecond frame. Reserve generics for tiny inner loops (`#[inline]` pixel/glyph helpers) where monomorphization actually pays off.
