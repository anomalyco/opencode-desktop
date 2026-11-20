// Stand-in for @opencode/widget's built-in `confirm` template (see prelude.js).
// data: { title, message, confirmLabel?, cancelLabel?, danger? }
// submit: { confirmed }

function view(state, data) {
  return ui.column({ gap: 2 }, [
    ui.text(data.title, { style: "title" }),
    ui.markdown(data.message),
    ui.row({ gap: 1 }, [
      ui.button("confirm", data.confirmLabel ?? "Confirm", { variant: data.danger ? "danger" : "primary" }),
      ui.button("cancel", data.cancelLabel ?? "Cancel", { variant: "secondary" }),
    ]),
  ])
}

function update(state, event, data) {
  if (event.id === "confirm") return submit({ confirmed: true }, state)
  if (event.id === "cancel") return submit({ confirmed: false }, state)
  return state
}
