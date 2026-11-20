// Stand-in for @opencode/widget's built-in `form` template (see prelude.js).
// data: { title, description?, submitLabel?, fields: (text | number | boolean | select)[] }
// submit: Record<key, string | number | boolean | string[]>

function init(data) {
  const values = {}
  for (const field of data.fields) {
    if (field.type === "text") values[field.key] = field.default ?? ""
    else if (field.type === "number") values[field.key] = field.default ?? field.min
    else if (field.type === "boolean") values[field.key] = field.default ?? false
    else if (field.type === "select") values[field.key] = field.default ?? (field.multiple ? [] : null)
  }
  return { values }
}

function field(f, value) {
  if (f.type === "text") {
    return ui.input(f.key, { label: f.label, value, placeholder: f.placeholder, multiline: !!f.multiline })
  }
  if (f.type === "number") {
    return ui.slider(f.key, { label: f.label, value, min: f.min, max: f.max, step: f.step })
  }
  if (f.type === "boolean") return ui.checkbox(f.key, f.label, value)
  const props = { label: f.label, multiple: !!f.multiple }
  if (value !== null) props.value = value
  return ui.select(f.key, f.options, props)
}

function view(state, data) {
  const rows = [ui.text(data.title, { style: "title" })]
  if (data.description) rows.push(ui.markdown(data.description))
  for (const f of data.fields) rows.push(field(f, state.values[f.key]))
  rows.push(ui.divider())
  rows.push(
    ui.row({ gap: 1 }, [
      ui.button("submit", data.submitLabel ?? "Submit", { variant: "primary" }),
      ui.button("cancel", "Cancel", { variant: "secondary" }),
    ]),
  )
  return ui.column({ gap: 2 }, rows)
}

function update(state, event, data) {
  if (event.id === "submit") {
    const out = {}
    for (const f of data.fields) if (state.values[f.key] !== null) out[f.key] = state.values[f.key]
    return submit(out, state)
  }
  if (event.id === "cancel") return dismiss(state)
  if (event.type === "change") return { ...state, values: { ...state.values, [event.id]: event.value } }
  return state
}
