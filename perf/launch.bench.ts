// The launch of OpenCode (Rune), measured by strobe from the process's spawn: the four launches a
// person gets, against three sizes of profile. README.md beside this file says how to run it and
// how to get a build with symbols.
//
//   strobe bench perf/launch.bench.ts --base exe:<old>/opencode-desktop.exe --head exe:<new>/opencode-desktop.exe --filter warm --target small
//   strobe bench perf/launch.bench.ts --head exe:<exe> --check                         # every case once, no verdict
//   strobe record --bench perf/launch.bench.ts#cold --head exe:<exe> --target small    # stacks: then `strobe query <rec> hotspots`
//
// Cases (the launch):
//   first            a fresh profile, never run before (target `none` only)
//   first-new-build  a profile an earlier run made, a new exe file, none of the exe's own caches: the launch after an update
//   cold             the exe's pages not in memory (a new file with the same bytes), GPU shader cache cold
//   warm             a later launch, everything cached
// Targets (the profile, and the server behind it):
//   none   no profile; small   5 projects, 12 sessions, a 10-message session open;
//   big    40 projects, 4,800 sessions, a 10,000-message session open
// Milestones are the app's own `--launch-trace` marks, printed by `--launch-marks` with the mark's own time:
//   launch.main (`main.entered`), launch.window (`window.created`), launch.first_frame
//   (`frame.first.presented`), launch.input_ready, launch.server (`server.answered`), launch.content_live.
import { cpSync, existsSync, mkdirSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join } from "node:path"
import { bench, native } from "@hona/strobe"

// The OpenCode server the app talks to: a fixture that answers the v2 API with a profile's worth of
// data. It is not in this repository yet. Give running servers (OPENCODE_LAUNCH_SERVER_SMALL, _BIG:
// their URLs) or a script this file starts once per size and leaves to its own exit
// (OPENCODE_LAUNCH_FIXTURE: `bun <script> --port <p> --projects <n> --sessions <n> --messages <n>`).
const SHAPES = {
  none: { port: Number(process.env.OPENCODE_LAUNCH_PORT_SMALL ?? 5871), projects: 5, sessions: 12, messages: 10 },
  small: { port: Number(process.env.OPENCODE_LAUNCH_PORT_SMALL ?? 5871), projects: 5, sessions: 12, messages: 10 },
  big: { port: Number(process.env.OPENCODE_LAUNCH_PORT_BIG ?? 5872), projects: 40, sessions: 4800, messages: 10000 },
} as const
type Shape = keyof typeof SHAPES
const serverUrl = (shape: Shape) => process.env[shape === "big" ? "OPENCODE_LAUNCH_SERVER_BIG" : "OPENCODE_LAUNCH_SERVER_SMALL"] ?? `http://127.0.0.1:${SHAPES[shape].port}`
// The session the saved window of `small` and `big` has open (the fixture's long session).
const SESSION = process.env.OPENCODE_LAUNCH_SESSION ?? "ses_e37949d95ffeScaleMain00000"
// Profiles made once per exe and size, and copied into each launch's home.
const SCRATCH = process.env.OPENCODE_LAUNCH_SCRATCH ?? join(tmpdir(), "opencode-rune-launch-bench")

async function up(shape: Shape) {
  return await fetch(`${serverUrl(shape)}/api/info`, { signal: AbortSignal.timeout(500) }).then((r) => r.ok).catch(() => false)
}

async function server(shape: Shape) {
  if (await up(shape)) return
  const script = process.env.OPENCODE_LAUNCH_FIXTURE
  if (!script) throw new Error(`no OpenCode server answers at ${serverUrl(shape)}: start one, or set OPENCODE_LAUNCH_FIXTURE to the fixture server's script (perf/README.md)`)
  const s = SHAPES[shape]
  Bun.spawn(["bun", script, "--port", String(s.port), "--projects", String(s.projects), "--sessions", String(s.sessions), "--messages", String(s.messages)], { stdout: "ignore", stderr: "ignore", stdin: "ignore", windowsHide: true }).unref()
  for (let i = 0; i < 150 && !(await up(shape)); i++) await Bun.sleep(100)
  if (!(await up(shape))) throw new Error(`the fixture server for ${shape} did not answer at ${serverUrl(shape)}`)
  // Its lists built before a timed launch reads them.
  for (let i = 0; i < 3; i++) await fetch(`${serverUrl(shape)}/api/session?limit=120`).then((r) => r.arrayBuffer()).catch(() => {})
}

type Before = { case: string; target: string; home: string | null; env: Record<string, string> | null; exe: string | null }

/** The folders the profile's environment names, which the app expects to exist. */
function folders(home: string) {
  for (const d of ["home/AppData/Local", "home/AppData/Roaming", "home/Documents", "tmp"]) mkdirSync(join(home, d), { recursive: true })
}

/** One launch of `exe` outside strobe's clock, in `home`, quitting by itself once its content is live. */
async function settle(exe: string, env: Record<string, string>, from: string, home: string, shape: Shape, extra: string[]) {
  folders(home)
  const moved = Object.fromEntries(Object.entries(env).map(([k, v]) => [k, v.split(from).join(home).split(from.replaceAll("\\", "/")).join(home.replaceAll("\\", "/"))]))
  const p = Bun.spawn([exe, "--server", serverUrl(shape), "--launch-trace", join(home, "launch-trace.jsonl"), "--launch-trace-quit", "4000", ...extra], { env: moved, cwd: dirname(exe), stdout: "ignore", stderr: "ignore", stdin: "ignore" })
  const timer = setTimeout(() => p.kill(), 60_000)
  await p.exited
  clearTimeout(timer)
  if (!existsSync(join(home, "launch-trace.jsonl"))) throw new Error(`${exe} did not finish a launch in ${home} (no launch trace): does it start by hand with --server ${serverUrl(shape)}?`)
  rmSync(join(home, "launch-trace.jsonl"), { force: true })
}

/** The profile a later launch starts from: made by `exe` itself (its own schema), one launch that
 *  opens the session and one that settles its caches. Kept per exe (path, size, time) and size. */
async function template(exe: string, env: Record<string, string>, from: string, shape: Shape) {
  const st = statSync(exe)
  const dir = join(SCRATCH, "profiles", `${basename(dirname(exe))}-${st.size}-${Math.round(st.mtimeMs)}-${shape}`)
  if (existsSync(join(dir, "ready"))) return dir
  rmSync(dir, { recursive: true, force: true })
  mkdirSync(dir, { recursive: true })
  await settle(exe, env, from, dir, shape, ["--session", SESSION])
  await settle(exe, env, from, dir, shape, [])
  writeFileSync(join(dir, "ready"), new Date().toISOString())
  return dir
}

/** The exe as a new file with the same bytes, at the same path: nothing of it is in memory, and
 *  what Windows keeps per file (its mapping, a scanner's verdict) starts again. The copy is
 *  unbuffered, so the file cache holds none of it either. */
function coldExe(exe: string) {
  // robocopy copies a file under its own name only: to a folder beside it, then moved over the old one.
  const aside = join(dirname(exe), ".cold")
  rmSync(aside, { recursive: true, force: true })
  mkdirSync(aside, { recursive: true })
  if (process.platform === "win32") Bun.spawnSync(["robocopy", dirname(exe), aside, basename(exe), "/J", "/NJH", "/NJS", "/NP", "/NFL", "/NDL"], { windowsHide: true })
  else cpSync(exe, join(aside, basename(exe)))
  if (!existsSync(join(aside, basename(exe)))) throw new Error(`could not copy ${exe}`)
  for (let i = 0; ; i++) {
    try {
      rmSync(exe, { force: true })
      renameSync(join(aside, basename(exe)), exe)
      break
    } catch (e) {
      if (i === 40) throw e
      Bun.sleepSync(100)
    }
  }
  rmSync(aside, { recursive: true, force: true })
}

/** Before every launch, never timed: the server up, the home seeded, the exe made cold. */
async function before(ctx: Before) {
  const shape = ctx.target as Shape
  if (!ctx.home || !ctx.env || !ctx.exe) throw new Error("this bench needs an exe variant: --head exe:<path to opencode-desktop>")
  await server(shape)
  folders(ctx.home)
  if (ctx.case !== "first") {
    // `OPENCODE_LAUNCH_PROFILE_BY`: another build's exe makes the profile, for the launch after an update from it.
    const by = ctx.case === "first-new-build" ? (process.env.OPENCODE_LAUNCH_PROFILE_BY ?? ctx.exe) : ctx.exe
    const tpl = await template(by, ctx.env, ctx.home, shape)
    // A new build finds the profile and none of what the last exe kept for itself (its page list, compiled shaders).
    for (const name of readdirSync(tpl)) if (name !== "ready" && name !== "home" && name !== "tmp" && !(ctx.case === "first-new-build" && name === "cache")) cpSync(join(tpl, name), join(ctx.home, name), { recursive: true })
  }
  if (ctx.case === "cold" || ctx.case === "first-new-build") coldExe(ctx.exe)
}

// Each milestone is a line the app prints with the mark's own time (epoch ms, to the microsecond in builds from 24 Nov).
const mark = (name: string) => ({ match: new RegExp(`launch-mark ${name.replace(/\./g, "\\.")} \\d+ at=(?<at>\\d{13}(?:\\.\\d+)?)`), in: "output" as const })
const milestones = {
  main: mark("main.entered"),
  window: mark("window.created"),
  first_frame: mark("frame.first.presented"),
  input_ready: mark("input.ready"),
  server: mark("server.answered"),
  content_live: mark("content.live"),
}
// `main.entered` and `window.created` are printed by builds from 24 Nov on: OPENCODE_LAUNCH_EARLY=0 leaves them out for an older one.
const early = process.env.OPENCODE_LAUNCH_EARLY === "0" ? [] : ["launch.main", "launch.window"]
const metrics = [...early, "launch.input_ready", "launch.server", "launch.content_live", "launch.painted", "cpu.work"]
const app = (shape: Shape, name: string) => native("opencode-rune", { name, output: true, viewport: { width: 1280, height: 800 }, args: ["--server", serverUrl(shape)] })
const launch = (profile: "fresh" | "warm") => ({ until: { milestone: "content_live" }, milestones, profile, before })

export default bench({
  targets: {
    none: app("none", "OpenCode (Rune), no profile"),
    small: app("small", "OpenCode (Rune), small profile"),
    big: app("big", "OpenCode (Rune), big profile"),
  },
  cases: {
    first: { primary: "launch.first_frame", hero: true, targets: ["none"], gpuCache: "cold", metrics, launch: launch("fresh") },
    "first-new-build": { primary: "launch.first_frame", targets: ["small", "big"], gpuCache: "cold", metrics, launch: launch("fresh") },
    cold: { primary: "launch.first_frame", targets: ["small", "big"], gpuCache: "cold", metrics, launch: launch("fresh") },
    warm: { primary: "launch.first_frame", targets: ["small", "big"], metrics, launch: launch("fresh") },
  },
})
