// Serves the browser build's folder on 127.0.0.1 (scripts/web.ps1 runs it): files only, never
// cached, the wasm with its own type so the browser compiles it as it streams in.
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
    return new Response(body, { headers: { "content-type": types[ext] ?? "application/octet-stream", "cache-control": "no-store" } })
  },
})
console.log(`http://127.0.0.1:${port}/`)
