// A real OpenCode server for the interaction fuzzer's replays (t1387, `real.rs`): an isolated
// Drive instance whose simulated model answers every prompt the way the sim's runs go, a few
// words over about three seconds, and every third turn a shell step that takes two seconds.
// It writes {url, password, directory, model} to FUZZ_REAL_OUT and runs until stopped.
//
//   FUZZ_REAL_OUT=$PWD/captures/interaction-fuzz/server.json \
//     opencode-drive start --name fuzz-real --script apps/desktop/scripts/interaction-fuzz/serve.ts
//   FUZZ_REAL=$PWD/captures/interaction-fuzz/server.json FUZZ_OPS='Send, Wait(800), Stop' \
//     cargo test -p opencode-desktop interaction_fuzz::tests::real_replay -- --ignored --nocapture
//   opencode-drive stop --name fuzz-real
//

import { Effect, Stream } from "effect"
import { defineScript, Llm, type Tool } from "opencode-drive"
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

const out = process.env.FUZZ_REAL_OUT ?? join(process.cwd(), "captures/interaction-fuzz/server.json")

const words = (n: number) =>
  Stream.fromIterable(Array.from({ length: n }, (_, k) => k)).pipe(
    Stream.flatMap((k) => Stream.make(Llm.pause(400), Llm.text(`w${k} `))),
  )

const tools = (registry: Tool.Registry) => {
  registry.handle("shell", () =>
    Effect.gen(function* () {
      yield* Effect.sleep(2_000)
      return { output: "done\n", exit: 0 }
    }),
  )
}

/** The service registration Drive's own SDK client reads (`driver/opencode.ts`). */
const endpoint = (artifacts: string) => {
  const state = join(artifacts, "home", ".local", "state", "opencode")
  const names = ["service-local.json", "service.json", ...readdirSync(state).filter((n) => /^service-[^.]+\.json$/.test(n))]
  for (const name of names) {
    try {
      const v = JSON.parse(readFileSync(join(state, name), "utf8"))
      const url = v.url ?? v.endpoint?.url
      if (typeof url === "string") return { url, password: v.auth?.password ?? v.password ?? null, raw: v }
    } catch {}
  }
  throw new Error(`no service registration under ${state}`)
}

export default defineScript({
  project: { git: true, files: { "README.md": "# Fuzz fixture\n" } },
  config: { autoupdate: false, username: "Drive" },
  tools,
  llm: { settlementTimeout: 600_000 },
  run: ({ llm, opencode, artifacts }) =>
    Effect.gen(function* () {
      let turn = 0
      yield* llm.serve((request) => {
        const body = JSON.stringify(request.body)
        if (body.includes("title generator")) return Stream.make(Llm.text("Fuzz"))
        // After a shell step, the turn ends in words.
        const lastPrompt = [...body.matchAll(/"[po]\d+"/g)].at(-1)?.index ?? -1
        if (body.lastIndexOf("done\\n") > lastPrompt) return words(4)
        turn += 1
        if (turn % 3 === 0)
          return Stream.make(Llm.toolCall({ index: 0, id: `call_${turn}`, name: "shell", input: { command: "make" } }), Llm.finish("tool-calls"))
        return words(8)
      })
      const { directory } = yield* opencode.location.get()
      const model = yield* opencode.model.default({ location: { directory } })
      const e = endpoint(artifacts)
      mkdirSync(dirname(out), { recursive: true })
      writeFileSync(
        out,
        JSON.stringify({ url: e.url, password: e.password, directory, model: model.data ?? null, service: e.raw }, null, 1) + "\n",
      )
      console.log(`interaction-fuzz: server ${e.url}, wrote ${out}`)
      yield* Effect.never
    }),
})
