// Stand-in for `packages/widget/src/prelude.js` (@opencode/widget), written from the contract
// (`specs/v2/widgets.md`) for stories and local runs until the canonical prelude lands. The
// server serves the canonical one inside every template program, so clients never ship this.

const __props = (p) => (p === undefined || p === null ? {} : p)

const ui = {
  column: (props, children) => ({ ...__props(props), type: "column", children: children ?? [] }),
  row: (props, children) => ({ ...__props(props), type: "row", children: children ?? [] }),
  text: (text, props) => ({ ...__props(props), type: "text", text: String(text) }),
  markdown: (text) => ({ type: "markdown", text: String(text) }),
  image: (src, props) => ({ ...__props(props), type: "image", src }),
  carousel: (props, children) => ({ ...__props(props), type: "carousel", children: children ?? [] }),
  card: (props, children) => ({ ...__props(props), type: "card", children: children ?? [] }),
  button: (id, label, props) => ({ ...__props(props), type: "button", id, label }),
  checkbox: (id, label, checked, props) => ({ ...__props(props), type: "checkbox", id, label, checked: !!checked }),
  slider: (id, props) => ({ ...__props(props), type: "slider", id }),
  select: (id, options, props) => ({ ...__props(props), type: "select", id, options }),
  input: (id, props) => ({ value: "", ...__props(props), type: "input", id }),
  divider: () => ({ type: "divider" }),
}

function submit(value, state) {
  return { __widget: "submit", value: value === undefined ? null : value, state }
}

function dismiss(state) {
  return { __widget: "dismiss", state }
}

function __widget_step(input) {
  const data = input.data
  if (input.phase === "init") {
    const state = typeof init === "function" ? init(data) : {}
    return { state, view: view(state, data) }
  }
  const result = typeof update === "function" ? update(input.state, input.event, data) : input.state
  if (result !== null && typeof result === "object" && result.__widget === "submit") {
    const state = result.state === undefined ? input.state : result.state
    return { state, view: view(state, data), submit: result.value }
  }
  if (result !== null && typeof result === "object" && result.__widget === "dismiss") {
    const state = result.state === undefined ? input.state : result.state
    return { state, view: view(state, data), dismiss: true }
  }
  const state = result === undefined ? input.state : result
  return { state, view: view(state, data) }
}
