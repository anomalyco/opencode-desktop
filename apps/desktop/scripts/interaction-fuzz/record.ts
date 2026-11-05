// Records what a real OpenCode server streams for the moves the interaction fuzzer's sim
// plays (t1387): a plain prompt, a steer, a queued prompt, a stop, a prompt right after a stop,
// a tool step and a subagent step. Each scenario runs in its own session; its events go to
// apps/desktop/src/interaction_fuzz/recorded/<name>.json, which `recorded_tests.rs` compares
// with the sim's.
//
//   opencode-drive run apps/desktop/scripts/interaction-fuzz/record.ts  
//
// OUT overrides the output folder.

import { Deferred, Effect, Stream } from "effect"
import { Llm, OpenCodeDriver, type Tool } from "opencode-drive"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const out = process.env.OUT ?? join(import.meta.dirname, "../../src/interaction_fuzz/recorded")

type Event = { type: string; data: Record<string, unknown> }

const gates = new Map<string, Deferred.Deferred<void>>()

// `hang-…` commands wait for their token's gate, so a stop can land mid-tool.
const tools = (registry: Tool.Registry) => {
  registry.handle("shell", ({ input }) =>
    Effect.gen(function* () {
      const token = input.command.split("-").at(-1) ?? ""
      const d = input.command.startsWith("hang-") ? gates.get(token) : undefined
      if (d !== undefined) yield* Deferred.await(d)
      return { output: `done-${token}\n`, exit: 0 }
    }),
  )
}

export default OpenCodeDriver.use(
  {
    project: { git: true, files: { "README.md": "# Fuzz fixture\n" } },
    config: { autoupdate: false, username: "Drive" },
    tools,
    llm: { settlementTimeout: 120_000 },
  },
  ({ llm, opencode }) =>
    Effect.gen(function* () {
      const events: Array<Event> = []
      const gate = (token: string) =>
        Effect.gen(function* () {
          const d = yield* Deferred.make<void>()
          gates.set(token, d)
          return d
        })

      // Replies are routed by the last token in the request: `H…` hangs until its gate opens,
      // `T…` calls the shell (`W…` a shell that hangs), `A…`/`B…` call a subagent (`S…`/`C…`
      // one whose own reply hangs), anything else answers in a few words.
      yield* llm.serve((request) => {
        const body = JSON.stringify(request.body)
        if (body.includes("title generator")) return Stream.make(Llm.text("Fuzz"))
        // A token may follow an escaped newline (`\nH9…Q`, a subagent's prompt), so no `\b`.
        const tokens = [...body.matchAll(/(?:\\n|[^A-Za-z0-9])([HTWABSCP]\d+Q)\b/g)].map((m) => m[1])
        const token = tokens.at(-1)
        if (token === undefined) return Stream.make(Llm.text("plain"))
        const kind = token[0]
        // The turn after a tool step ends in words (a subagent's prompt sits in its call's input).
        if (tokens.some((t) => body.includes(`call_${t}`))) return Stream.make(Llm.text("after the tool"))
        if (kind === "H") {
          const d = gates.get(token)
          const tail = d === undefined ? Stream.empty : Stream.fromEffect(Deferred.await(d)).pipe(Stream.flatMap(() => Stream.make(Llm.text(" released"))))
          return Stream.make(Llm.text("working ")).pipe(Stream.concat(tail))
        }
        if (kind === "T" || kind === "W") {
          const command = kind === "W" ? `hang-${token}` : `echo done-${token}`
          return Stream.make(Llm.toolCall({ index: 0, id: `call_${token}`, name: "shell", input: { command } }), Llm.finish("tool-calls"))
        }
        // `A…`/`S…` run a subagent to the end, `B…`/`C…` start one in the background.
        if (kind === "A" || kind === "B" || kind === "S" || kind === "C")
          return Stream.make(
            Llm.toolCall({
              index: 0,
              id: `call_${token}`,
              name: "subagent",
              input: {
                agent: "general",
                description: "fuzz helper",
                prompt: `${kind === "S" || kind === "C" ? "H" : "P"}9${token.slice(1)} help`,
                ...(kind === "B" || kind === "C" ? { background: true } : {}),
              },
            }),
            Llm.finish("tool-calls"),
          )
        return Stream.make(Llm.text("a short "), Llm.pause(200), Llm.text("answer"))
      })

      yield* opencode.event.subscribe().pipe(
        Stream.runForEach((event) =>
          Effect.sync(() => {
            const e = event as unknown as { type: string; data?: Record<string, unknown> }
            events.push({ type: e.type, data: e.data ?? {} })
          }),
        ),
        Effect.forkScoped,
      )

      const { directory } = yield* opencode.location.get()
      const model = yield* opencode.model.default({ location: { directory } })
      const session = () =>
        opencode.session.create({
          title: "fuzz",
          location: { directory },
          ...(model.data ? { model: { providerID: model.data.providerID, id: model.data.id } } : {}),
          agent: "build",
        } as never)

      /** Waits up to a minute for the server to go quiet; returns what's still running if it doesn't. */
      const idle = Effect.gen(function* () {
        let active: Record<string, unknown> = {}
        for (let i = 0; i < 1200; i++) {
          active = (yield* opencode.session.active()) as Record<string, unknown>
          // Scenarios run one at a time, so a quiet server means this one and its helpers are done.
          if (Object.keys(active).length === 0 && i > 2) return undefined
          yield* Effect.sleep(50)
        }
        return active
      })

      const prompt = (sessionID: string, text: string, delivery?: "steer" | "queue") =>
        opencode.session.prompt({ sessionID, text, ...(delivery ? { delivery } : {}) } as never)

      const record = (name: string, play: (sessionID: string) => Effect.Effect<void, unknown>) =>
        Effect.gen(function* () {
          const started = Date.now()
          const s = yield* session()
          const from = events.length
          yield* play(s.id)
          const stuck = yield* idle
          // A stopped reply's gate never opened; open it so Drive can settle at the end.
          for (const d of gates.values()) yield* Deferred.succeed(d, undefined)
          yield* Effect.sleep(300)
          // This session's events and its helpers' (sessions created with it as parent).
          const ours = new Set<string>([s.id])
          const mine = events.slice(from).filter((e) => {
            const sid = e.data.sessionID
            const info = e.data.info as Record<string, unknown> | undefined
            const parent = e.data.parentID ?? info?.parentID
            if (e.type === "session.created" && typeof parent === "string" && ours.has(parent)) {
              const id = info?.id ?? e.data.id ?? sid
              if (typeof id === "string") ours.add(id)
            }
            return typeof sid === "string" && ours.has(sid)
          })
          const messages = yield* opencode.message.list({ sessionID: s.id, limit: 50, order: "asc" })
          mkdirSync(out, { recursive: true })
          writeFileSync(
            join(out, `${name}.json`),
            JSON.stringify({ scenario: name, sessionID: s.id, ...(stuck ? { stuck } : {}), events: mine, messages }, null, 1) + "\n",
          )
          console.log(`record: ${name} ${stuck ? "STILL RUNNING " + JSON.stringify(stuck) : "settled"} in ${Date.now() - started} ms, ${mine.length} events`)
          if (stuck) for (const id of Object.keys(stuck)) yield* Effect.ignore(opencode.session.interrupt({ sessionID: id } as never))
        })

      yield* record("plain", (sid) => prompt(sid, "P1Q hello"))
      yield* record("steer", (sid) =>
        Effect.gen(function* () {
          const d = yield* gate("H2Q")
          yield* prompt(sid, "H2Q wait")
          yield* Effect.sleep(300)
          yield* prompt(sid, "P3Q and this")
          yield* Effect.sleep(300)
          yield* Deferred.succeed(d, undefined)
        }),
      )
      yield* record("queue", (sid) =>
        Effect.gen(function* () {
          const d = yield* gate("H4Q")
          yield* prompt(sid, "H4Q wait")
          yield* Effect.sleep(300)
          yield* prompt(sid, "P5Q later", "queue")
          yield* Effect.sleep(300)
          yield* Deferred.succeed(d, undefined)
        }),
      )
      yield* record("stop", (sid) =>
        Effect.gen(function* () {
          yield* gate("H6Q")
          yield* prompt(sid, "H6Q wait")
          yield* Effect.sleep(300)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
        }),
      )
      yield* record("stop-then-prompt", (sid) =>
        Effect.gen(function* () {
          yield* gate("H7Q")
          yield* prompt(sid, "H7Q wait")
          yield* Effect.sleep(300)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
          yield* prompt(sid, "P8Q right after")
        }),
      )
      yield* record("stop-with-queued", (sid) =>
        Effect.gen(function* () {
          yield* gate("H10Q")
          yield* prompt(sid, "H10Q wait")
          yield* Effect.sleep(300)
          yield* prompt(sid, "P11Q later", "queue")
          yield* Effect.sleep(200)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
          yield* Effect.sleep(1_000)
          yield* prompt(sid, "P12Q wakes it")
        }),
      )
      yield* record("tool", (sid) => prompt(sid, "T13Q run it"))
      yield* record("subagent", (sid) => prompt(sid, "A14Q delegate"))
      yield* record("background-helper", (sid) => prompt(sid, "B15Q delegate"))
      yield* record("stop-during-tool", (sid) =>
        Effect.gen(function* () {
          yield* gate("W16Q")
          yield* prompt(sid, "W16Q run it")
          yield* Effect.sleep(400)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
        }),
      )
      yield* record("stop-during-subagent", (sid) =>
        Effect.gen(function* () {
          yield* gate("H917Q")
          yield* prompt(sid, "S17Q delegate")
          yield* Effect.sleep(600)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
        }),
      )
      // The parent goes idle while its background helper still runs; the helper then finishes.
      yield* record("helper-wakes-idle-parent", (sid) =>
        Effect.gen(function* () {
          const d = yield* gate("H918Q")
          yield* prompt(sid, "C18Q delegate")
          yield* Effect.sleep(1_500)
          yield* Deferred.succeed(d, undefined)
        }),
      )
      yield* record("queue-while-idle", (sid) => prompt(sid, "P19Q queued", "queue"))
      yield* record("stop-twice", (sid) =>
        Effect.gen(function* () {
          yield* gate("H20Q")
          yield* prompt(sid, "H20Q wait")
          yield* Effect.sleep(300)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
          yield* Effect.sleep(300)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
        }),
      )
      yield* record("stop-while-idle", (sid) =>
        Effect.gen(function* () {
          yield* prompt(sid, "P21Q hello")
          yield* Effect.sleep(1_500)
          yield* opencode.session.interrupt({ sessionID: sid } as never)
        }),
      )
    }).pipe(Effect.scoped),
)
