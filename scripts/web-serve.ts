// Serves the browser build's folder on 127.0.0.1 (scripts/web.ps1 runs it): files only, as a
// static host does: each with an ETag the browser revalidates on every use (`no-cache`), so a
// repeat visit takes the module from its cache, compiled code included, and a rebuild is fetched
// again; the wasm with its own type so the browser compiles it as it streams in; Brotli-compressed
// where the build left a copy (a release).
// Usage: bun scripts/web-serve.ts <folder> <port>
import { resolve, sep } from "node:path"

const root = resolve(process.argv[2] ?? "target/web")
const port = Number(process.argv[3] ?? 8790)
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

Bun.serve({
  hostname: "127.0.0.1",
  port,
  async fetch(request) {
    const path = new URL(request.url).pathname
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
  },
})
console.log(`http://127.0.0.1:${port}/`)
