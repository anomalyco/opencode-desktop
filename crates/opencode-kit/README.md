# `opencode-kit`

Reusable OpenCode client library and session data model for Rune applications.

- **Connection (`client`, `local_service`, `pairing`):** discovers a local OpenCode server (`http://127.0.0.1:4096` or `service.json`), sends HTTP reads/writes on background worker threads, subscribes to `/api/event` live updates, and wakes the Rune window via `rune::Waker`.
- **Model (`model`, `project`, `session_ops`):** parses OpenCode sessions, transcripts, messages, tool calls, permissions, questions, inbox queues, compactions, and projects into plain Rust structs.
