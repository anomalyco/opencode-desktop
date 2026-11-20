// Stand-in for @opencode/widget's built-in `suggestions` template (see prelude.js).
// data: { title, options: { label, note?, images: string[], recommended? }[], allowNote? }
// submit: { choice, index, note? }

function init(data) {
  return { picked: null, note: "" }
}

function view(state, data) {
  const cards = data.options.map((option, i) => {
    const head = [ui.text(option.label, { style: "title" })]
    if (option.recommended) head.push(ui.text("Recommended", { style: "caption", tone: "accent" }))
    const body = [
      ui.carousel({}, option.images.map((src) => ui.image(src, { alt: option.label, aspect: 4 / 3 }))),
      ui.row({ gap: 1, align: "center", wrap: true }, head),
    ]
    if (option.note) body.push(ui.text(option.note, { style: "caption", tone: "muted" }))
    return ui.card({ id: "pick:" + i, selected: state.picked === i }, body)
  })
  const rows = [ui.text(data.title, { style: "title" }), ui.row({ gap: 2 }, cards)]
  if (data.allowNote !== false) {
    rows.push(ui.input("note", { value: state.note, placeholder: "Add a note (optional)" }))
  }
  rows.push(
    ui.row({ gap: 1, align: "center" }, [
      ui.button("choose", "Choose", { variant: "primary", disabled: state.picked === null }),
      ui.button("skip", "Skip", { variant: "secondary" }),
    ]),
  )
  return ui.column({ gap: 2 }, rows)
}

function update(state, event, data) {
  if (event.id.startsWith("pick:")) return { ...state, picked: Number(event.id.slice(5)) }
  if (event.id === "note") return { ...state, note: String(event.value ?? "") }
  if (event.id === "skip") return dismiss(state)
  if (event.id === "choose" && state.picked !== null) {
    const option = data.options[state.picked]
    const value = { choice: option.label, index: state.picked }
    if (state.note.trim() !== "") value.note = state.note.trim()
    return submit(value, state)
  }
  return state
}
