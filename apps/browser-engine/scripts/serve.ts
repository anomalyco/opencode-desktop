// Serves package-windows.ps1's output where a development build of the app looks for the browser
// engine (browser-pane installer.rs, DEV_SOURCE): http://127.0.0.1:47963/index.json and the
// archives it names, with byte ranges so a download cut short resumes.
// Usage: bun apps/browser-engine/scripts/serve.ts [<folder>] [<port>]
import { resolve, sep } from "node:path"

const root = resolve(process.argv[2] ?? resolve(import.meta.dir, "../target/package"))
const port = Number(process.argv[3] ?? 47963)

Bun.serve({
  hostname: "127.0.0.1",
  port,
  async fetch(request) {
    const file = resolve(root, "." + decodeURIComponent(new URL(request.url).pathname))
    if (!file.startsWith(root + sep)) return new Response("forbidden", { status: 403 })
    const body = Bun.file(file)
    if (!(await body.exists())) return new Response("not found", { status: 404 })
    const headers = { "accept-ranges": "bytes", "cache-control": "no-store" }
    const range = /^bytes=(\d+)-$/.exec(request.headers.get("range") ?? "")
    if (!range) return new Response(body, { headers })
    const start = Number(range[1])
    if (start >= body.size) return new Response(null, { status: 416, headers: { "content-range": `bytes */${body.size}` } })
    return new Response(body.slice(start), {
      status: 206,
      headers: { ...headers, "content-range": `bytes ${start}-${body.size - 1}/${body.size}` },
    })
  },
})
console.log(`http://127.0.0.1:${port}/index.json (${root})`)
