#!/usr/bin/env bun
// Lightweight upstream sync radar for anomalyco/opencode-rune.
// Reads docs/upstream-sync/ledger.json, compares against anomalyco/opencode dev (via local
// --opencode clone or `gh api`), classifies touched paths by watched surface and workstream,
// flags high-risk areas, and lists open PRs on anomalyco/opencode-rune by author with any
// upstream PR numbers referenced in their titles or bodies.
//
// Zero external dependencies, zero builds, runs in seconds.
//
// Usage:
//   bun scripts/upstream-sync-check.ts [--ledger docs/upstream-sync/ledger.json]
//     [--opencode <path-to-local-opencode-clone>] [--since <sha>]
//     [--repo anomalyco/opencode-rune] [--no-open-prs]
//     [--append-draft] [--markdown-out <file>] [--json-out <file>]

import { existsSync, readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"

type LedgerRow = {
  id: string
  window: string
  pr: number | null
  sha: string
  title: string
  merged: string
  packages: string[]
  main_files: string[]
  summary: string
  size: string
  status: string
  status_evidence: string
  depends_on: string
  risks: string
  workstream: string
  rune_pr?: string
  open_pr_note?: string
}

type Ledger = {
  generated_at: string
  upstream_repo: string
  upstream_reference_base: string
  upstream_head: string
  upstream_pre_window_base: string
  rune_desktop_repo: string
  rune_desktop_head: string
  post_1114_commit_count: number
  post_1114_in_ledger: number
  post_1114_skipped: number
  post_1114_skipped_breakdown: Record<string, number>
  pre_1114_carryover_rows: number
  total_ledger_rows: number
  status_counts: Record<string, number>
  rows: LedgerRow[]
}

type UpstreamCommit = {
  sha: string
  title: string
  date: string
  author: string
  files: string[]
  pr: number | null
}

type OpenPr = {
  number: number
  title: string
  author: { login?: string; name?: string }
  headRefName: string
  isDraft: boolean
  updatedAt: string
  body: string
  url: string
}

type ClassifiedCommit = UpstreamCommit & {
  category: "watched" | "protocol" | "skipped"
  skipReason?: string
  workstream: string
  redFlags: string[]
  packages: string[]
  watchedFiles: string[]
}

const WATCHED_PREFIXES = [
  "packages/app/",
  "packages/desktop/",
  "packages/ui/",
  "packages/session-ui/",
  "packages/gui-plugins/",
  "packages/plugin-browser/",
  "packages/theme/",
]

const PROTOCOL_PREFIXES = [
  "packages/protocol/",
  "packages/schema/",
  "packages/server/src/handlers/",
  "packages/plugin/src/gui/",
]
const PROTOCOL_EXACT = new Set(["packages/client/src/promise/generated/types.ts"])

const RED_FLAG_PATTERNS: Array<{ label: string; match: (f: string) => boolean }> = [
  { label: "plugin-browser", match: (f) => f.startsWith("packages/plugin-browser/") },
  { label: "gui-plugins/browser", match: (f) => f.startsWith("packages/gui-plugins/src/browser/") },
  { label: "plugin/gui contract", match: (f) => f.startsWith("packages/plugin/src/gui/") },
  { label: "protocol/groups", match: (f) => f.startsWith("packages/protocol/src/groups/") },
  { label: "dialog.css", match: (f) => f.endsWith("/dialog.css") || f === "dialog.css" },
  { label: "settings/model.tsx", match: (f) => f.endsWith("settings/model.tsx") },
]

function argVal(argv: string[], flag: string): string | undefined {
  const i = argv.indexOf(flag)
  return i >= 0 && i + 1 < argv.length ? argv[i + 1] : undefined
}

function runCmd(cmd: string[], cwd?: string): { code: number; out: string; err: string } {
  const p = Bun.spawnSync(cmd, { cwd, stdout: "pipe", stderr: "pipe", windowsHide: true })
  return {
    code: p.exitCode ?? 1,
    out: Buffer.from(p.stdout).toString("utf8"),
    err: Buffer.from(p.stderr).toString("utf8"),
  }
}

function extractPrFromTitle(title: string): number | null {
  const m = title.match(/\(#(\d+)\)\s*$/)
  return m ? Number(m[1]) : null
}

function isExcludedTestOrStory(file: string): boolean {
  return (
    /\.(test|spec|fixture)\.[cm]?[jt]sx?$/.test(file) ||
    /\.stories\.[cm]?[jt]sx?$/.test(file) ||
    /(^|\/)(e2e|stories|__tests__|__snapshots__|test|tests)\//.test(file)
  )
}

function packageOf(file: string): string {
  const m = file.match(/^packages\/([^/]+)\//)
  if (m) return m[1]!
  const top = file.split("/")[0] ?? "root"
  return `<root:${top}>`
}

function skipBucket(files: string[]): string {
  if (files.every((f) => isExcludedTestOrStory(f))) return "test-only"
  if (files.every((f) => f.startsWith("packages/tui/"))) return "tui-only"
  if (files.every((f) => f.startsWith("packages/cli/") || f.startsWith("packages/acp/"))) return "cli-acp-only"
  if (files.every((f) => f.startsWith("services/") || f.startsWith("docs/") || f.endsWith(".md") || f.endsWith(".mdx"))) return "docs-site-data"
  if (files.every((f) => f.startsWith(".github/") || f.startsWith("script/") || f.startsWith("scripts/") || f === "package.json" || f === "bun.lock")) return "ci-bench-deps"
  return "server-core-ai-codemode-internal"
}

function assignWorkstream(files: string[], title: string, category: "watched" | "protocol" | "skipped"): string {
  if (category === "skipped") return "none"
  const joined = files.join("\n") + "\n" + title.toLowerCase()
  if (/packages\/gui-plugins\/src\/browser\/extensions\/|packages\/desktop\/src\/main\/browser\/extensions/.test(joined)) return "WS-9 (Browser Extensions)"
  if (/packages\/plugin-browser\/|experimental\.browser|tunnel|proxy-machine/.test(joined)) return "BROWSER-TOOLS (Browser RPC/Tools)"
  if (/packages\/gui-plugins\/src\/browser\/(scheme|preview|workspace-file|remote-preview|artifact-preview|disk\/)|oc-preview/.test(joined)) return "PREVIEWS (Browser Previews)"
  if (/packages\/gui-plugins\/src\/browser\/(address|history|settings|i18n|refusal|panel|bar\.css|device|viewport)/.test(joined) && !/packages\/plugin-browser\/|chromium\.ts|window-open|page-log/.test(joined)) return "BROWSER-UI (Browser UI/History)"
  if (/packages\/(plugin-browser|gui-plugins\/src\/browser)\/|packages\/desktop\/.*browser|browser pane/.test(joined)) return "WS-1 (Browser)"
  if (/focus[- ]ring|focus[- ]rescue|focus[- ]visibility|field-focus|high contrast|forced-colors|tooltip|keymap|shortcut|overlays\/menu/.test(joined)) return "WS-8 (Focus/Shortcuts)"
  if (/packages\/session-ui\/|packages\/app\/src\/session\/timeline|packages\/app\/src\/session\/session-identity|markdown|latex|math|\bfind\b/.test(joined)) return "WS-3 (Timeline)"
  if (/\/job|codemode|background-result|subagent|activity-chips|session\/requests\/|session-request-tree/.test(joined)) return "WS-4 (Jobs/Requests)"
  if (/packages\/app\/src\/(shell\/titlebar|home\/|shell\/layout|shell\/project)|tab-group|sidebar|consolidated/.test(joined)) return "WS-2 (Navigation/Sidebar)"
  if (/packages\/app\/src\/(providers\/connect|providers\/models|composer\/|session\/composer|session\/usage-exceeded|workspaces\/selection)/.test(joined)) return "WS-5 (Composer/Connect)"
  if (/packages\/gui-plugins\/src\/(goal|pairing|context|updater|usage)\/|packages\/plugin\/src\/gui\//.test(joined)) return "WS-6 (Plugins)"
  if (/packages\/app\/src\/(settings\/|servers\/|tips\/|runtime\/i18n\/)|\/i18n\//.test(joined)) return "WS-7 (Settings/i18n)"
  if (category === "protocol") return "Protocol/Contract Review"
  return "Unassigned (Review)"
}

function classifyCommit(c: UpstreamCommit): ClassifiedCommit {
  const watchedFiles = c.files.filter((f) => WATCHED_PREFIXES.some((p) => f.startsWith(p)) && !isExcludedTestOrStory(f))
  const protocolFiles = c.files.filter((f) => (PROTOCOL_PREFIXES.some((p) => f.startsWith(p)) || PROTOCOL_EXACT.has(f)) && !isExcludedTestOrStory(f))
  const redFlags = RED_FLAG_PATTERNS.filter((rf) => c.files.some(rf.match)).map((rf) => rf.label)
  const packages = [...new Set(c.files.map(packageOf))].sort()

  if (watchedFiles.length > 0) {
    return {
      ...c,
      category: "watched",
      workstream: assignWorkstream(watchedFiles, c.title, "watched"),
      redFlags,
      packages,
      watchedFiles,
    }
  }
  if (protocolFiles.length > 0) {
    return {
      ...c,
      category: "protocol",
      workstream: assignWorkstream(protocolFiles, c.title, "protocol"),
      redFlags,
      packages,
      watchedFiles: protocolFiles,
    }
  }
  return {
    ...c,
    category: "skipped",
    skipReason: skipBucket(c.files),
    workstream: "none",
    redFlags,
    packages,
    watchedFiles: [],
  }
}

function loadUpstreamFromLocal(repoDir: string, sinceSha: string): { headSha: string; commits: UpstreamCommit[] } {
  const headRes = runCmd(["git", "rev-parse", "origin/dev"], repoDir)
  if (headRes.code !== 0) throw new Error(`git rev-parse origin/dev failed in ${repoDir}: ${headRes.err}`)
  const headSha = headRes.out.trim()
  const logRes = runCmd(
    ["git", "log", "--reverse", "--format=@@@%H%x1f%cs%x1f%an%x1f%s", "--name-only", `${sinceSha}..${headSha}`],
    repoDir,
  )
  if (logRes.code !== 0) throw new Error(`git log failed in ${repoDir}: ${logRes.err}`)

  const commits: UpstreamCommit[] = []
  for (const block of logRes.out.split("@@@").slice(1)) {
    const lines = block.split(/\r?\n/)
    const header = lines[0] ?? ""
    const [sha, date, author, title] = header.split("\x1f")
    if (!sha || !title) continue
    const files = lines.slice(1).map((l) => l.trim()).filter(Boolean)
    commits.push({ sha, date: date ?? "", author: author ?? "", title, files, pr: extractPrFromTitle(title) })
  }
  return { headSha, commits }
}

function loadUpstreamFromGh(upstreamRepo: string, sinceSha: string): { headSha: string; commits: UpstreamCommit[] } {
  const cmp = runCmd(["gh", "api", `repos/${upstreamRepo}/compare/${sinceSha}...dev`])
  if (cmp.code !== 0) throw new Error(`gh api compare failed: ${cmp.err}`)
  const data = JSON.parse(cmp.out) as {
    commits?: Array<{ sha: string; commit: { message: string; author?: { name?: string; date?: string } } }>
  }
  const raw = data.commits ?? []
  const headSha = raw.length > 0 ? raw[raw.length - 1]!.sha : sinceSha
  const commits: UpstreamCommit[] = []
  for (const item of raw) {
    const detail = runCmd(["gh", "api", `repos/${upstreamRepo}/commits/${item.sha}`, "--jq", "[.files[].filename]"])
    const files = detail.code === 0 ? (JSON.parse(detail.out) as string[]) : []
    const title = (item.commit.message.split(/\r?\n/)[0] ?? "").trim()
    const date = (item.commit.author?.date ?? "").slice(0, 10)
    commits.push({
      sha: item.sha,
      date,
      author: item.commit.author?.name ?? "",
      title,
      files,
      pr: extractPrFromTitle(title),
    })
  }
  return { headSha, commits }
}

function extractUpstreamPrRefs(title: string, body: string): number[] {
  const text = `${title}\n${body}`
  const found = new Set<number>()
  for (const m of text.matchAll(/anomalyco\/opencode#(\d+)/gi)) found.add(Number(m[1]))
  // Upstream opencode PRs are 5+ digits (60000+); opencode-rune PRs are 1-4 digits.
  for (const m of text.matchAll(/(?<![A-Za-z0-9/])#(\d{5,})\b/g)) found.add(Number(m[1]))
  return [...found].sort((a, b) => a - b)
}

function loadOpenPrs(repo: string): OpenPr[] {
  const res = runCmd([
    "gh",
    "pr",
    "list",
    "-R",
    repo,
    "--state",
    "open",
    "--limit",
    "100",
    "--json",
    "number,title,author,headRefName,isDraft,updatedAt,body,url",
  ])
  if (res.code !== 0) {
    console.error(`[radar] warning: could not query open PRs for ${repo}: ${res.err.trim()}`)
    return []
  }
  return JSON.parse(res.out) as OpenPr[]
}

function buildMarkdown(opts: {
  upstreamRepo: string
  runeRepo: string
  sinceSha: string
  headSha: string
  classified: ClassifiedCommit[]
  openPrs: OpenPr[]
  ledgerByPr: Map<number, LedgerRow>
}): string {
  const watched = opts.classified.filter((c) => c.category === "watched")
  const protocol = opts.classified.filter((c) => c.category === "protocol")
  const skipped = opts.classified.filter((c) => c.category === "skipped")

  const skipCounts = new Map<string, number>()
  for (const s of skipped) {
    const k = s.skipReason ?? "other"
    skipCounts.set(k, (skipCounts.get(k) ?? 0) + 1)
  }

  const lines: string[] = []
  lines.push(`# Upstream Sync Radar (\`${opts.sinceSha.slice(0, 10)}..${opts.headSha.slice(0, 10)}\`)`)
  lines.push("")
  lines.push(`- **Upstream repo:** \`${opts.upstreamRepo}\``)
  lines.push(`- **New commits scanned:** **${opts.classified.length}** (**${watched.length}** watched UI/desktop · **${protocol.length}** protocol/contract review · **${skipped.length}** skipped)`)
  if (skipped.length > 0) {
    const breakdown = [...skipCounts.entries()].map(([k, v]) => `${k}: ${v}`).join(", ")
    lines.push(`- **Skipped breakdown:** ${breakdown}`)
  }
  lines.push("")

  const actionable = [...watched, ...protocol]
  if (actionable.length === 0) {
    lines.push("## New Upstream Commits")
    lines.push("")
    lines.push("_No new watched or protocol-review commits since `upstream_head`._")
    lines.push("")
  } else {
    lines.push("## New Upstream Commits by Workstream")
    lines.push("")
    const groups = new Map<string, ClassifiedCommit[]>()
    for (const c of actionable) {
      const list = groups.get(c.workstream) ?? []
      list.push(c)
      groups.set(c.workstream, list)
    }
    for (const [ws, list] of [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      lines.push(`### ${ws} (${list.length})`)
      for (const c of list) {
        const prTag = c.pr ? `#${c.pr} ` : ""
        const kindTag = c.category === "protocol" ? " `[protocol-review]`" : ""
        const flags = c.redFlags.length > 0 ? ` **[RED FLAG: ${c.redFlags.join(", ")}]**` : ""
        const sortedFiles = [...c.watchedFiles].sort((a, b) => {
          const ai = /(^|\/)i18n\//.test(a) ? 1 : 0
          const bi = /(^|\/)i18n\//.test(b) ? 1 : 0
          return ai - bi || a.localeCompare(b)
        })
        const sampleFiles = sortedFiles.slice(0, 4).map((f) => `\`${f}\``).join(", ")
        const extra = sortedFiles.length > 4 ? ` (+${sortedFiles.length - 4} more)` : ""
        lines.push(`- ${prTag}(\`${c.sha.slice(0, 10)}\`, ${c.date}) ${c.title}${kindTag}${flags}`)
        lines.push(`  - Files: ${sampleFiles}${extra}`)
      }
      lines.push("")
    }
  }

  lines.push(`## Open PRs on \`${opts.runeRepo}\` (${opts.openPrs.length})`)
  lines.push("")
  if (opts.openPrs.length === 0) {
    lines.push("_None (or skipped with `--no-open-prs`)._")
    lines.push("")
  } else {
    const byAuthor = new Map<string, OpenPr[]>()
    for (const pr of opts.openPrs) {
      const key = pr.author.login ? `@${pr.author.login}${pr.author.name ? ` (${pr.author.name})` : ""}` : "unknown"
      const list = byAuthor.get(key) ?? []
      list.push(pr)
      byAuthor.set(key, list)
    }
    for (const [author, prs] of [...byAuthor.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      lines.push(`### ${author} (${prs.length})`)
      for (const pr of prs.sort((a, b) => a.number - b.number)) {
        const draft = pr.isDraft ? " `[draft]`" : ""
        const refs = extractUpstreamPrRefs(pr.title, pr.body)
        const refText =
          refs.length > 0
            ? refs
                .map((n) => {
                  const row = opts.ledgerByPr.get(n)
                  return row ? `#${n} (${row.id}: ${row.status})` : `#${n}`
                })
                .join(", ")
            : "none named"
        lines.push(`- **#${pr.number}**${draft} \`${pr.headRefName}\` — ${pr.title}`)
        lines.push(`  - Upstream PRs named: ${refText}`)
      }
      lines.push("")
    }
  }

  return lines.join("\n")
}

function main() {
  const argv = process.argv.slice(2)
  const ledgerPath = resolve(argVal(argv, "--ledger") ?? "docs/upstream-sync/ledger.json")
  const opencodeDir = argVal(argv, "--opencode")
  const noOpenPrs = argv.includes("--no-open-prs")
  const appendDraft = argv.includes("--append-draft")
  const markdownOut = argVal(argv, "--markdown-out")
  const jsonOut = argVal(argv, "--json-out")

  if (!existsSync(ledgerPath)) {
    console.error(`upstream-sync-check: ledger not found at ${ledgerPath}`)
    process.exit(1)
  }
  const ledger = JSON.parse(readFileSync(ledgerPath, "utf8")) as Ledger
  const sinceSha = argVal(argv, "--since") ?? ledger.upstream_head
  const upstreamRepo = ledger.upstream_repo || "anomalyco/opencode"
  const runeRepo = argVal(argv, "--repo") ?? ledger.rune_desktop_repo ?? "anomalyco/opencode-rune"

  const { headSha, commits } = opencodeDir
    ? loadUpstreamFromLocal(resolve(opencodeDir), sinceSha)
    : loadUpstreamFromGh(upstreamRepo, sinceSha)

  const classified = commits.map(classifyCommit)
  const openPrs = noOpenPrs ? [] : loadOpenPrs(runeRepo)

  const ledgerByPr = new Map<number, LedgerRow>()
  for (const row of ledger.rows) {
    if (typeof row.pr === "number") ledgerByPr.set(row.pr, row)
  }

  const md = buildMarkdown({ upstreamRepo, runeRepo, sinceSha, headSha, classified, openPrs, ledgerByPr })

  const watched = classified.filter((c) => c.category === "watched")
  const protocol = classified.filter((c) => c.category === "protocol")
  const skipped = classified.filter((c) => c.category === "skipped")

  const jsonPayload = {
    since_sha: sinceSha,
    head_sha: headSha,
    total_new_commits: classified.length,
    watched_count: watched.length,
    protocol_count: protocol.length,
    skipped_count: skipped.length,
    watched,
    protocol,
    open_prs: openPrs.map((pr) => ({
      number: pr.number,
      author: pr.author.login ?? "unknown",
      branch: pr.headRefName,
      draft: pr.isDraft,
      title: pr.title,
      upstream_prs: extractUpstreamPrRefs(pr.title, pr.body),
    })),
  }

  if (appendDraft && watched.length + protocol.length > 0) {
    const existingShas = new Set(ledger.rows.map((r) => r.sha.slice(0, 10)))
    let nextNum =
      ledger.rows
        .map((r) => {
          const m = r.id.match(/^UP-(\d+)$/)
          return m ? Number(m[1]) : 0
        })
        .reduce((a, b) => Math.max(a, b), 0) + 1
    const drafts: LedgerRow[] = []
    for (const c of [...watched, ...protocol]) {
      if (existingShas.has(c.sha.slice(0, 10))) continue
      drafts.push({
        id: `UP-${String(nextNum++).padStart(3, "0")}`,
        window: "post-1120",
        pr: c.pr,
        sha: c.sha.slice(0, 10),
        title: c.title,
        merged: c.date,
        packages: c.packages,
        main_files: c.watchedFiles.slice(0, 6),
        summary: c.title,
        size: c.watchedFiles.length > 10 ? "L" : c.watchedFiles.length > 3 ? "M" : "S",
        status: c.category === "protocol" ? "needs review" : "missing",
        status_evidence: "Added as draft by scripts/upstream-sync-check.ts; awaiting code verification.",
        depends_on: "none",
        risks: c.redFlags.length > 0 ? `Red flags: ${c.redFlags.join(", ")}` : "none",
        workstream: c.workstream.split(" ")[0] ?? "none",
      })
    }
    console.log(md)
    console.log("\n## Draft Ledger Rows (`--append-draft`)\n```json")
    console.log(JSON.stringify(drafts, null, 2))
    console.log("```")
  } else {
    console.log(md)
  }

  if (markdownOut) writeFileSync(resolve(markdownOut), md, "utf8")
  if (jsonOut) writeFileSync(resolve(jsonOut), JSON.stringify(jsonPayload, null, 2) + "\n", "utf8")
}

main()
