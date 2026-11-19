# `session-view`

Embeddable OpenCode session timeline view for Rune applications.

Renders an `opencode_kit::model::Transcript` inside a virtualized, bottom-following Rune scroll container — including user bubbles, `@` mention chips, quotes, Markdown, ```` ```ui ```` outlines (`rune-parts`), Mermaid diagrams, reasoning, folded tool groups, subagent cards, inline question rows, images, compactions, turn errors, live overlay scroll bar, and "Jump to latest" — without depending on `opencode-desktop` window chrome or side panels.

## Standalone example

Run the read-only timeline viewer and interactive playground:

```sh
cargo run -p session-view --example read_only
```

Render a headless snapshot:

```sh
cargo run -p session-view --example read_only -- --scene read-only --zoom 1.6 --size 1280x820 --scale 2 --theme dark --snap /tmp/read-only.png
```

## Embedding

1. Store `session_view::ViewState` on your Rune app state and call `view.setup(waker, headless)` during setup.
2. Implement `session_view::TimelineHost` for your state struct (use `session_view::delegate_view_state!(view);` to wire the 7 state-accessor pairs).
3. Drive `session_view::poll(state, now, tx)` each frame.
4. Render `session_view::read_only(state, p, id, transcript)` or `session_view::timeline_area(state, p, id, transcript, pills, header)`, alongside `session_view::overlays(state, p, cx)`.

See [`examples/read_only.rs`](examples/read_only.rs) and [`src/story.rs`](src/story.rs) for a minimal complete host.
