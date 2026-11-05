# Interaction fuzz (t1387)

Plays seeded random sessions against the real OpenCode (Rune) app and checks, after every frame, that the screen agrees with the server. It covers sending, steering, queueing, stopping (button, Esc, empty ↵, held ↵, twice), other clients, background helpers, and a network that is slow, dead, loses a write or a reply, comes back, or loses its server. A failure is shrunk to the fewest steps and kept as a replay with a timeline of screen vs server.

## Running it

```sh
# 120 seeds from 0 on the sim; report in captures/interaction-fuzz/report-0.txt
FUZZ_SEEDS=120 FUZZ_FROM=0 cargo test -p opencode-desktop interaction_fuzz::tests::fuzz -- --ignored --nocapture --exact

# the replays and open bugs (about 15 s)
cargo test -p opencode-desktop interaction_fuzz -- --include-ignored --skip tests::fuzz --skip real_replay

# each line of bugs.tsv once against a real server under Drive, through a chaos proxy
apps/desktop/scripts/interaction-fuzz/real.sh confirm apps/desktop/src/interaction_fuzz/bugs.tsv

# record a fresh set of real event streams to compare the sim with
… real.sh record
```

Keep the fuzz under about 150 seeds per run.

## What it checks

After every step, and once the link has been good and the server idle:

- prompts show in the server's order, none lost or doubled, and none moves once shown (one the server never got may);
- no Not confirmed or Pending once the server has a prompt, and Failed to send only when it doesn't;
- Stop or Send matches whether the server is running, and Working only shows while it is;
- no toast blames the server or OpenCode while the network is what's down;
- the session settles.

## Open bugs, all confirmed on a real server

| Bug | Screen | Server | Replay |
|---|---|---|---|
| A failed prompt leaves Working up | p1 says Failed to send; Working spins under it for good | idle, never got p1 | `LoseWrite, Send` |
| A dead network is blamed on OpenCode | "Failed to send prompt / OpenCode didn't answer in time" | up the whole time | `Dead, Send, Wait(53339)` |
| A dead network is blamed on the server | "The server didn't confirm the stop" | up, stopped the run | `Send, Wait(1500), Dead, EnterEmpty, Wait(7500)` |
| Another client's prompt lands in the wrong place | `[p1 o1]` for good | `[o1 p1]` | `Slow, Other, Send` (`Send, Other` in the sim) |
| Racing steers swap places when they land | your Pending p1 above their o2, then o2 jumps above p1 | took o2 first | `Other, Wait(500), Slow, Send, Other` |

Each is an `#[ignore]`d `fails_with` test in `src/interaction_fuzz/tests.rs`; when a fix makes one pass, take its ignore off and it guards the fix.

Kit's bugs this week, as regressions that fail with their fix reverted: t1361 (prompts after a stop jumped, then reshuffled), t1309 (Stop for an idle parent with only background helpers), t1347 (↵ in an empty box stops; a held ↵ never does), and t1350 (a lost reply said Sending… for good). Two finds from before t1350 and t1349 landed no longer happen: a Pending bubble blinking for a frame after a restart, and a reset connection taking back a prompt the server had.

## How the sim differs from a real server

The sim was fitted to 15 recorded real streams (`src/interaction_fuzz/recorded/`, checked by `recorded_tests.rs`). What the recordings changed: queued prompts join the running turn; tool steps end on `tool-calls`; a stop closes open text and ends the parent before its subagent; an idle session's prompt gets its own message; background helpers wake their parent with a synthetic steer. What still differs:

- A restart is a guess: the sim keeps a prompt it had admitted in the inbox. The real replay can only play a restart as a proxy outage, because Drive can't restart its server yet (anomalyco/opencode-drive#82).
- The real replay skips background helpers, so they're checked against the sim only.
- The page reader's failure map and history timer use the wall clock, not the virtual one, so "Still trying" only shows in the sim when a page read was in flight.

## Changes outside the harness

None in `crates/opencode-kit`. Test-only hooks in the app, all behind `#[cfg(test)]`: a virtual wall clock (`main.rs`), page jobs a test can take and answer (`session_pages.rs`), and receipt checks a test's network can answer (`send_failed.rs`; the read loop moved into `read_receipts`, unchanged).

Drive gaps filed: anomalyco/opencode-drive#135 (settling waits on an interrupted reply), and comments on #132, #82 and #65.
