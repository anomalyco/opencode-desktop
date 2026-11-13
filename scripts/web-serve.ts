// Serves the browser build's folder on 127.0.0.1 (scripts/web.ps1 runs it): files only, as a
// static host does: each with an ETag the browser revalidates on every use (`no-cache`), so a
// repeat visit takes the module from its cache, compiled code included, and a rebuild is fetched
// again; the wasm with its own type so the browser compiles it as it streams in; Brotli-compressed
// where the build left a copy (a release).
// With a log file, each request is a JSON line (time, method, path, status, user agent, and the
// names of the query's keys: never a value, since the address can carry a password), and the
// page's `POST diag` (what a refused sign-in looked like, no password) is logged beside them.
// Usage: bun scripts/web-serve.ts <folder> <port> [<log file>]
import { appendFileSync } from "node:fs"
import { resolve, sep } from "node:path"

const root = resolve(process.argv[2] ?? "target/web")
const port = Number(process.argv[3] ?? 8790)
const logFile = process.argv[4]
const types: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".ttf": "font/ttf",
  ".otf": "font/otf",
  ".wav": "audio/wav",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".png": "image/png",
}
// What a diagnostic may say, and nothing else: no password, no hash of one.
const DIAG = { host: "string", status: "number", length: "number", tidied: "boolean", source: "string", ua: "string", standalone: "boolean", bodyTag: "string", challenge: "boolean", bare: "string", stage: "string" } as const

const log = (entry: Record<string, unknown>) => {
  if (!logFile) return
  try { appendFileSync(logFile, JSON.stringify({ t: new Date().toISOString(), ...entry }) + "\n") } catch {}
}

Bun.serve({
  hostname: "127.0.0.1",
  port,
  async fetch(request) {
    const url = new URL(request.url)
    const response = await answer(request, url.pathname)
    // `from`: the device's address as a proxy in front reports it (Tailscale Serve sets it), so
    // one device's requests can be told from another's.
    log({ method: request.method, path: url.pathname, keys: [...new Set(url.searchParams.keys())], status: response.status, from: request.headers.get("x-forwarded-for") ?? "", ua: request.headers.get("user-agent") ?? "" })
    return response
  },
})
console.log(`http://127.0.0.1:${port}/`)

async function answer(request: Request, path: string) {
  if (request.method === "POST" && path === "/diag") return diag(request)
  const file = resolve(root, "." + (path.endsWith("/") ? path + "index.html" : path))
  if (file !== root && !file.startsWith(root + sep)) return new Response("forbidden", { status: 403 })
  const body = Bun.file(file)
  if (!(await body.exists())) return new Response("not found", { status: 404 })
  const ext = file.slice(file.lastIndexOf("."))
  const br = Bun.file(file + ".br")
  const brotli = /\bbr\b/.test(request.headers.get("accept-encoding") ?? "") && (await br.exists())
  const tag = `\"${body.size.toString(36)}-${body.lastModified.toString(36)}${brotli ? "-br" : ""}\"`
  const headers: Record<string, string> = { "content-type": types[ext] ?? "application/octet-stream", "cache-control": "no-cache", etag: tag, vary: "accept-encoding" }
  if (request.headers.get("if-none-match") === tag) return new Response(null, { status: 304, headers })
  // A release's module and fonts have a Brotli copy beside them: sent to a browser that takes br, as a host would.
  if (brotli) return new Response(br, { headers: { ...headers, "content-encoding": "br" } })
  return new Response(body, { headers })
}

async function diag(request: Request) {
  const text = await request.text()
  if (text.length > 8192) return new Response(null, { status: 413 })
  const sent = (() => { try { return JSON.parse(text) as Record<string, unknown> } catch { return {} } })()
  // The phone check (`check.js`): its code, then one line per measurement, each an item's name,
  // its state and what it measured, as short strings.
  if (typeof sent.check === "string" && /^[A-Z0-9]{4,8}$/.test(sent.check) && Array.isArray(sent.items)) {
    const from = request.headers.get("x-forwarded-for") ?? ""
    for (const entry of sent.items.slice(0, 32) as Record<string, unknown>[]) {
      if (typeof entry?.item !== "string" || !/^[a-z]{2,16}$/.test(entry.item)) continue
      const state = ["ok", "fail", "none"].includes(String(entry.state)) ? String(entry.state) : "none"
      log({ kind: "diag", from, check: sent.check, partial: sent.partial === true, item: entry.item, state, value: typeof entry.value === "string" ? entry.value.slice(0, 300) : "" })
    }
    return new Response(null, { status: 204 })
  }
  const kept = Object.fromEntries(Object.entries(DIAG).flatMap(([key, type]) => (typeof sent[key] === type ? [[key, typeof sent[key] === "string" ? String(sent[key]).slice(0, 300) : sent[key]]] : [])))
  log({ kind: "diag", from: request.headers.get("x-forwarded-for") ?? "", ...kept })
  return new Response(null, { status: 204 })
}
