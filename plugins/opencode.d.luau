-- OpenCode plugin contract 1.0: global types for luau-lsp.
-- Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.
-- Load it with `luau-lsp analyze --definitions=@opencode=plugins/opencode.d.luau`.

-- Semantic tokens ---------------------------------------------------------------------------------

--- A theme color by meaning. The host resolves it for the current theme and color scheme.
export type SemanticToken =
    "bg-base" -- The window's base background.
    | "bg-weak" -- A background one step quieter than `bg-base`, such as a sidebar.
    | "bg-strong" -- A background one step stronger than `bg-base`, such as a title bar.
    | "surface-1" -- The first raised layer: cards and grouped lists.
    | "surface-2" -- The second raised layer: controls on a card.
    | "surface-3" -- The third raised layer: a control's track, such as an off switch.
    | "surface-4" -- The fourth raised layer: the strongest fill a control uses.
    | "surface-hover" -- The fill of a row or button under the pointer.
    | "surface-press" -- The fill of a row or button while it is pressed.
    | "surface-selected" -- The fill of a selected row or tab.
    | "text-strong" -- Headings and the most important text.
    | "text-base" -- Body text.
    | "text-weak" -- Secondary text: descriptions and labels.
    | "text-weaker" -- Tertiary text: hints, placeholders and timestamps.
    | "text-muted" -- Secondary text in the app's chrome: header figures' labels, menu row names, tab titles. Hosts without a separate chrome scale use `text-weak`.
    | "text-faint" -- Quiet figures and hints in the app's chrome, such as a header's counts. Hosts without a separate chrome scale use `text-weaker`.
    | "text-on-accent" -- Text on an `accent` fill.
    | "text-interactive" -- Links and other interactive text.
    | "icon-strong" -- An icon that needs emphasis.
    | "icon-base" -- The default icon color.
    | "icon-weak" -- A secondary icon.
    | "icon-weaker" -- A decorative or disabled icon.
    | "icon-on-accent" -- An icon on an `accent` fill.
    | "border-strong" -- A strong border, such as a focused field's outline.
    | "border-base" -- The default border.
    | "border-weak" -- A hairline divider or a card's border.
    | "border-focus" -- The keyboard focus ring.
    | "accent" -- The brand accent: primary buttons and switches that are on.
    | "accent-hover" -- An `accent` fill under the pointer.
    | "accent-subtle" -- A quiet accent tint, such as a selected chip's fill.
    | "status-success" -- Done or passing.
    | "status-warning" -- Needs attention.
    | "status-danger" -- Failed, blocked or destructive.
    | "status-info" -- Neutral information.

-- Enums -------------------------------------------------------------------------------------------

--- Which half of a plugin runs the code.
export type Half = "window" | "app"

--- The operating system the host runs on.
export type Platform = "windows" | "macos" | "linux" | "web"

--- The app build's release channel.
export type Channel = "local" | "beta" | "prod"

--- A light or dark color scheme.
export type ColorScheme = "light" | "dark"

--- A status color for an icon or a pill.
export type Tone = "success" | "warning" | "danger"

--- A toast's tone.
export type ToastVariant = "default" | "success" | "error" | "loading"

--- The section of Settings › Shortcuts that lists a command.
export type CommandSection = "general" | "session" | "navigation" | "model" | "terminal" | "prompt"

--- A host menu a menu item joins.
export type MenuName = "session.panel" | "server.add" | "server.row"

--- A submenu of the app menu bar.
export type MenubarMenu = "app" | "file" | "edit" | "view" | "go" | "window" | "help"

--- A fixed place in the app's shell that a slot fills.
export type SlotName = "window.bottom" | "session.header" | "session.location.menu" | "session.panel.end"

--- A host settings page a settings page can add a section to.
export type HostSettingsPage = "general" | "servers"

--- Where a contributed server stands.
export type ServerState = "stopped" | "starting" | "auth" | "ready" | "failed" | "incompatible"

--- Where a plugin's panel tab is in a session.
export type PanelState = "closed" | "listed" | "selected" | "visible"

--- How a layout change moves.
export type Transition = "default" | "none"

--- How hover and press fills change.
export type HoverTransition = "default" | "instant"

--- A side of an anchor.
export type Side = "top" | "bottom" | "left" | "right"

--- Alignment along an edge or an axis.
export type Align = "start" | "center" | "end"

--- How a stack places its children across its axis.
export type UiAlign = "start" | "center" | "end" | "stretch"

--- How a stack places its children along its axis.
export type UiJustify = "start" | "center" | "end" | "between"

--- A text weight.
export type FontWeight = "regular" | "medium" | "semibold" | "bold"

--- One of the user's font choices.
export type FontFamily = "sans" | "mono"

--- A button's look, as the app's own buttons.
export type ButtonVariant = "neutral" | "contrast" | "primary" | "ghost" | "ghost-muted" | "outline" | "danger"

--- A control's size.
export type ControlSize = "small" | "normal" | "large"

--- A shadow from the app's elevation scale.
export type Elevation = "raised" | "floating" | "overlay"

--- An element's accessibility role.
export type Role = "none" | "group" | "region" | "button" | "link" | "switch" | "checkbox" | "radio" | "tab" | "list" | "listitem" | "heading" | "img" | "status"

--- The state of a contract another plugin provides.
export type LiveState = "pending" | "active" | "inactive" | "failed"

--- The state of a cached server read.
export type ResourceStatus = "loading" | "ready" | "error"

--- An HTTP method.
export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"

--- A process's output stream.
export type ProcessStream = "stdout" | "stderr"

--- How a process encodes its output.
export type TextEncoding = "utf8" | "utf16le"

--- How a diff lays out its changes.
export type DiffMode = "split" | "unified"

--- What a diff compares.
export type DiffSource = "git" | "branch" | "turn"

--- How an image fills its box.
export type ImageFit = "contain" | "cover" | "fill"

--- An Office document's format.
export type OfficeKind = "docx" | "xlsx" | "pptx"

--- Where the app updater stands.
export type UpdaterPhase = "idle" | "checking" | "up-to-date" | "downloading" | "ready" | "installing" | "error"

--- What a `UiEvent` reports.
export type UiEventKind = "press" | "hover" | "submit" | "change" | "select" | "visible-rows"

--- A kind of menu entry.
export type MenuEntryKind = "item" | "group" | "separator"

--- A dialog's width.
export type DialogSize = "small" | "medium" | "large"

--- What a declared store keeps a value for.
export type StoreScope = "global" | "session"

--- How `ui.progressCircle` draws, as the app's ProgressCircle.
export type ProgressAppearance = "compact" | "indicator"

--- Which way the interface language reads.
export type TextDirection = "ltr" | "rtl"

--- A CLDR plural category: which form of a counted message the language uses for a count.
export type PluralCategory = "zero" | "one" | "two" | "few" | "many" | "other"

--- A date and time format, as the app writes dates.
export type DateStyle = "time" | "monthDay" | "monthDayYear" | "dateTime" | "monthDayTime" | "monthDayYearTime" | "monthDayTimeZone"

--- A unit of relative time.
export type TimeUnit = "second" | "minute" | "hour" | "day" | "week" | "month" | "year"

--- What a number means.
export type NumberStyle = "decimal" | "percent" | "currency"

--- How a number is written.
export type NumberNotation = "standard" | "compact"

-- Opaque host types -------------------------------------------------------------------------------

--- A literal color from `ui.hex("#RRGGBB")` or `ui.hex("#RRGGBBAA")`. Prefer a `SemanticToken`: a literal color does not follow the theme.
declare extern type HexColor with end

--- An element a `ui.*` constructor built. Pass it to a parent or return it from a render.
declare extern type UiNode with end

--- An event handler `view:Handler` made. It runs as a coroutine, so it may yield.
declare extern type UiHandler with end

--- An image the host holds, for `ui.image`.
declare extern type ImageId with end

--- What `plugin.defineWindow` and `plugin.defineApp` return. Return it from the entry module.
declare extern type PluginHalf with end

-- Aliases, records and host objects ---------------------------------------------------------------

--- A semantic token, or a literal color from `ui.hex`.
export type Color = SemanticToken | HexColor

--- The name of one of the app's icons, such as `"circle-check"`.
export type IconName = string

--- A memo's version. The host rebuilds the memo only when it changes.
export type MemoVersion = string | number | boolean

--- A length in logical points, or `"full"` to fill the parent.
export type UiSize = number | "full"

--- An event source. Connections disconnect on their own when the plugin half that made them goes away.
export type Signal<T...> = {
    --- Calls `callback` on every fire, as a coroutine.
    Connect: (self: Signal<T...>, callback: (T...) -> ()) -> Connection,
    --- Calls `callback` on the next fire only.
    Once: (self: Signal<T...>, callback: (T...) -> ()) -> Connection,
    --- Yields until the next fire and returns its values. Yields: call it from a handler, a task or a signal callback.
    Wait: (self: Signal<T...>) -> T...,
}

--- A signal a plugin fires itself, from `ctx:CreateSignal()`. Its version counts fires: put it in a memo's version.
export type ScriptSignal<T...> = {
    --- Calls `callback` on every fire, as a coroutine.
    Connect: (self: ScriptSignal<T...>, callback: (T...) -> ()) -> Connection,
    --- Calls `callback` on the next fire only.
    Once: (self: ScriptSignal<T...>, callback: (T...) -> ()) -> Connection,
    --- Yields until the next fire and returns its values. Yields: call it from a handler, a task or a signal callback.
    Wait: (self: ScriptSignal<T...>) -> T...,
    --- Fires the signal, adds 1 to its version and asks the window to redraw.
    Fire: (self: ScriptSignal<T...>, T...) -> (),
    --- How many times the signal fired.
    Version: (self: ScriptSignal<T...>) -> number,
}

--- One connection to a signal.
export type Connection = {
    --- False once disconnected.
    Connected: boolean,
    --- Stops the callback. Does nothing when already disconnected.
    Disconnect: (self: Connection) -> (),
}

--- A contribution a plugin added. The host withdraws it when the plugin half goes away.
export type Contribution = {
    --- Removes the contribution now.
    Withdraw: (self: Contribution) -> (),
    --- Asks the host to call the contribution's `show`, `list` or `render` again.
    Refresh: (self: Contribution) -> (),
}

--- A contract another plugin provides, followed live.
export type Live<T> = {
    --- The provider's state.
    State: (self: Live<T>) -> LiveState,
    --- The provided value while the state is `active`.
    Get: (self: Live<T>) -> T?,
    --- Changes on every state change and every new value: put it in a memo's version.
    Generation: (self: Live<T>) -> number,
    --- Fires on every state change and every new value.
    Changed: Signal<LiveState>,
}

--- What a render callback receives first. Render callbacks must not yield.
export type View = {
    --- Returns the subtree `build` returns, and calls `build` again only when `version` changes. An unchanged frame costs no Luau call. Keep `key` unique within the render.
    Memo: (self: View, key: string, version: MemoVersion, build: () -> UiNode?) -> UiNode?,
    --- Wraps `callback` for an `on*` property. The handler runs as a coroutine, so it may call `Async` methods and `task.wait`.
    Handler: (self: View, callback: (event: UiEvent) -> ()) -> UiHandler,
}

--- What a handler receives.
export type UiEvent = {
    --- What happened.
    kind: UiEventKind,
    --- The `key` of the element the event came from.
    key: string?,
    --- A `hover` event: true as the pointer enters, false as it leaves.
    entered: boolean?,
    --- A `submit` event: the field's text.
    text: string?,
    --- A `change` event: the new state.
    checked: boolean?,
    --- A `select` event: the picked path.
    path: string?,
    --- A `visible-rows` event: the first visible row, from 1.
    first: number?,
    --- A `visible-rows` event: the last visible row.
    last: number?,
    --- Shift was held.
    shift: boolean?,
    --- Alt (Option) was held.
    alt: boolean?,
    --- The platform's command key was held: Ctrl, or Cmd on macOS.
    mod: boolean?,
}

--- A host-owned text field's content: typing, IME, selection and undo stay in the host and survive a hot reload.
export type Editor = {
    --- The field's text.
    Text: (self: Editor) -> string,
    --- Replaces the text and puts the caret at its end.
    SetText: (self: Editor, text: string) -> (),
    --- Changes whenever the text, selection, composition or focus does: put it in a memo's version.
    Version: (self: Editor) -> number,
    --- Moves keyboard focus to the field.
    Focus: (self: Editor) -> (),
    --- Fires with the new text after each edit.
    Changed: Signal<string>,
    --- Fires with the text when the user presses Enter.
    Submitted: Signal<string>,
}

--- Size, spacing, position and accessibility: every `ui.*` node takes these.
export type UiLayoutProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
}

--- Offsets from the parent's edges for an out-of-flow node. Give one or two per axis.
export type UiPin = {
    --- From the left edge.
    left: number?,
    --- From the top edge.
    top: number?,
    --- From the right edge.
    right: number?,
    --- From the bottom edge.
    bottom: number?,
}

--- A container's props: `ui.box`, `ui.row` and `ui.col`.
export type UiBoxProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- Space between children.
    gap: number?,
    --- How children sit across the axis.
    align: UiAlign?,
    --- How children sit along the axis.
    justify: UiJustify?,
    --- Centers children on both axes.
    center: boolean?,
    --- Children wrap onto more lines.
    wrap: boolean?,
    --- Background.
    fill: Color?,
    --- Background under the pointer.
    hover: Color?,
    --- Background while pressed.
    press: Color?,
    --- Draws the `surface-selected` fill and reports the selected state to assistive technology.
    selected: boolean?,
    --- Border color.
    border: Color?,
    --- Border width. Defaults to 1 when `border` is set.
    borderWidth: number?,
    --- Corner radius.
    radius: number?,
    --- Per-corner radii: top-left, top-right, bottom-right, bottom-left.
    corners: { number }?,
    --- A shadow from the app's elevation scale.
    shadow: Elevation?,
    --- Clips children to the box.
    clip: boolean?,
    --- Scrolls vertical overflow.
    scrollY: boolean?,
    --- Scrolls horizontal overflow.
    scrollX: boolean?,
    --- Takes keyboard focus.
    focusable: boolean?,
    --- Takes no presses.
    disabled: boolean?,
    --- Runs on a press.
    onPress: UiHandler?,
    --- Runs as the pointer enters and leaves.
    onHover: UiHandler?,
    --- How hover and press fills change.
    hoverTransition: HoverTransition?,
    --- Starts a hover group: descendants with `hoverGroupColor` or `revealOnHover` react while this box is hovered.
    hoverGroup: boolean?,
    --- The text or icon color while the nearest hover group is hovered.
    hoverGroupColor: Color?,
    --- Shows only while the nearest hover group is hovered or focused.
    revealOnHover: boolean?,
    --- The default text and icon color for descendants.
    color: Color?,
}

--- Text styling: `ui.text` and `ui.shimmer`.
export type UiTextProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- Font size in points. Defaults to 13.
    size: number?,
    --- Weight.
    weight: FontWeight?,
    --- Line height in points.
    lineHeight: number?,
    --- Letter spacing in points.
    tracking: number?,
    --- Text color. Defaults to `text-base`.
    color: Color?,
    --- The code font.
    mono: boolean?,
    --- Tabular figures.
    tabular: boolean?,
    --- Wraps onto more lines. The default for text in a column.
    wrap: boolean?,
    --- Keeps to one line.
    noWrap: boolean?,
    --- Ends with an ellipsis when it does not fit.
    truncate: boolean?,
    --- Fades out at the end when it does not fit, instead of an ellipsis.
    fadeOut: boolean?,
    --- The user can select and copy it.
    selectable: boolean?,
    --- Alignment within its box.
    textAlign: Align?,
    --- The color while the nearest hover group is hovered.
    hoverGroupColor: Color?,
    --- Shows only while the nearest hover group is hovered.
    revealOnHover: boolean?,
}

--- `ui.spacer` props.
export type UiSpacerProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
}

--- `ui.divider` props.
export type UiDividerProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- A vertical rule. Defaults to horizontal.
    vertical: boolean?,
    --- Defaults to `border-weak`.
    color: Color?,
}

--- `ui.icon` props.
export type UiIconProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- Width and height in points. Defaults to 16.
    size: number?,
    --- Defaults to `icon-base`.
    color: Color?,
    --- A status color, instead of `color`.
    tone: Tone?,
    --- The color while the nearest hover group is hovered.
    hoverGroupColor: Color?,
    --- Shows only while the nearest hover group is hovered.
    revealOnHover: boolean?,
}

--- `ui.image` props.
export type UiImageProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The image, from the `Images` service or an `Async` render.
    image: ImageId,
    --- How it fills its box.
    fit: ImageFit?,
    --- Corner radius.
    radius: number?,
}

--- `ui.metric` props: a number shown as text.
export type UiMetricProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The number the text stands for. Hosts that animate compare it with the last value.
    value: number,
    --- What it reads, such as `"12.4K"`.
    text: string,
    --- Tabular figures. Defaults to true.
    tabular: boolean?,
    --- Animates changed digits where the host can. Defaults to true. Other hosts show the text.
    animate: boolean?,
    --- Font size in points.
    size: number?,
    --- Weight.
    weight: FontWeight?,
    --- Line height in points.
    lineHeight: number?,
    --- Letter spacing in points.
    tracking: number?,
    --- Color.
    color: Color?,
}

--- `ui.button` props: the app's own button.
export type UiButtonProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The visible label. Also the accessible name unless `label` is set.
    text: string?,
    --- A leading icon.
    icon: IconName?,
    --- The look.
    variant: ButtonVariant?,
    --- The size.
    size: ControlSize?,
    --- Takes no presses.
    disabled: boolean?,
    --- Shows the app's loader and takes no presses.
    loading: boolean?,
    --- A toggle button's pressed state.
    pressed: boolean?,
    --- Runs on a press.
    onPress: UiHandler?,
}

--- `ui.iconButton` props: a square button with an icon. Give it a `label`.
export type UiIconButtonProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The icon.
    icon: IconName,
    --- The look. Defaults to `ghost`.
    variant: ButtonVariant?,
    --- The size.
    size: ControlSize?,
    --- Takes no presses.
    disabled: boolean?,
    --- A toggle button's pressed state.
    pressed: boolean?,
    --- Runs on a press.
    onPress: UiHandler?,
}

--- `ui.switch` props: the app's own switch.
export type UiSwitchProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- On.
    checked: boolean,
    --- Cannot change.
    disabled: boolean?,
    --- Runs with the new state in `event.checked`.
    onChange: UiHandler?,
}

--- `ui.radio` props: a radio row with its label and description.
export type UiRadioProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The row's label.
    text: string,
    --- A line under the label.
    description: string?,
    --- Selected.
    checked: boolean,
    --- Colors the label.
    tone: Tone?,
    --- Cannot change.
    disabled: boolean?,
    --- Runs as the user picks the row.
    onChange: UiHandler?,
}

--- `ui.field` props: the app's own text field over an `Editor`.
export type UiFieldProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The field's content, from `ctx:CreateEditor`.
    editor: Editor,
    --- Shows while the field is empty.
    placeholder: string?,
    --- The size.
    size: ControlSize?,
    --- Drawn as invalid.
    invalid: boolean?,
    --- Hides the text, for a password.
    secure: boolean?,
    --- Takes no typing.
    disabled: boolean?,
    --- Grows to more lines. Enter inserts a line break; Mod+Enter submits.
    multiline: boolean?,
    --- Runs on Enter with the text in `event.text`.
    onSubmit: UiHandler?,
}

--- `ui.loader` props: the app's spinner.
export type UiLoaderProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- Width and height in points. Defaults to 16.
    size: number?,
}

--- `ui.pill` props: a status capsule like the composer's pills.
export type UiPillProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The capsule's label.
    text: string,
    --- A leading icon.
    icon: IconName?,
    --- Colors the icon.
    tone: Tone?,
    --- Shows the capsule pressed.
    pressed: boolean?,
    --- Runs on a press.
    onPress: UiHandler?,
}

--- `ui.keybind` props: a shortcut chip.
export type UiKeybindProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- A published command id. The chip shows the user's current binding.
    command: string?,
    --- A literal binding, such as `"mod+shift+t"`, when there is no command.
    keys: string?,
}

--- `ui.settingsRow` props: one settings row, as the app's own.
export type UiSettingsRowProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The row's search id: settings search scrolls to it.
    id: string,
    --- The row's title.
    title: string,
    --- A line under the title.
    description: string?,
}

--- `ui.settingsList` props: a card of settings rows split by hairlines.
export type UiSettingsListProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- A section title above the card.
    title: string?,
}

--- `ui.flyoutRow` props: a popover row that opens a nested popover beside it. The host owns placement, hover intent and dismissal.
export type UiFlyoutRowProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The row's label.
    text: string,
    --- A leading icon.
    icon: IconName?,
    --- Muted text at the row's end, such as a count.
    detail: string?,
    --- The id of this plugin's popover the row opens.
    popover: string,
    --- Passed to the nested popover's render as `props.data`.
    props: { [string]: any }?,
}

--- `ui.markdown` props. Native: parsing, shaping, code fences and Mermaid diagrams stay in the host.
export type UiMarkdownProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The Markdown.
    source: string,
    --- Body font size in points. Defaults to 14.
    size: number?,
    --- Body line height in points.
    lineHeight: number?,
    --- Words to highlight, such as search matches.
    highlights: { string }?,
    --- The user can select and copy it. Defaults to true.
    selectable: boolean?,
    --- The source is still growing: the host keeps the last block open.
    streaming: boolean?,
}

--- `ui.virtualList` props. Native: windowing, the height cache and scroll anchoring stay in the host; Luau builds only the rows that come into view.
export type UiVirtualListProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- How many rows.
    count: number,
    --- A row's height before it is measured.
    estimatedRowHeight: number,
    --- Change it when rows change. While it stays the same, scrolling builds only rows that enter the view.
    version: MemoVersion,
    --- Builds row `index`, from 1. It must not yield.
    renderRow: (index: number) -> UiNode,
    --- A stable identity for row `index`, so rows keep their state as the list changes. Defaults to the index.
    rowKey: ((index: number) -> string)?,
    --- Space between rows.
    gap: number?,
    --- Runs as the visible rows change, with `event.first` and `event.last`.
    onVisibleRows: UiHandler?,
}

--- `ui.terminalSurface` props. Native: the VT parser, scrollback, glyph grid, IME and PTY stream stay in the host.
export type UiTerminalSurfaceProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The terminal session, from the `Terminal` service.
    session: PtySession,
    --- A font family. Defaults to the user's code font.
    fontFamily: string?,
    --- Font size in points.
    fontSize: number?,
    --- Takes keyboard focus.
    focused: boolean?,
}

--- `ui.browserSurface` props. Native: the browser engine, its compositor, and input forwarding stay in the host.
export type UiBrowserSurfaceProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The page, from the `Browser` service.
    tab: BrowserTab,
    --- Takes keyboard focus.
    focused: boolean?,
}

--- `ui.codeView` props. Native: highlighting, the line buffer, the gutter, selection and find stay in the host.
export type UiCodeViewProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The file, from `Workspace:OpenDocument`.
    doc: CodeDocument,
    --- Wraps long lines.
    lineWrap: boolean?,
    --- Shows the gutter. Defaults to true.
    showLineNumbers: boolean?,
    --- Scrolls to this line, from 1, and highlights it.
    revealLine: number?,
}

--- `ui.diffView` props. Native: hunk layout, intraline diffing, highlighting and comment anchors stay in the host.
export type UiDiffViewProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The changes, from `Diff:Open`.
    diff: DiffDocument,
    --- Split or unified.
    mode: DiffMode,
    --- Shows unchanged lines in full.
    expand: boolean?,
    --- Shows only this file's changes.
    file: string?,
}

--- `ui.fileTree` props. Native: tree flattening, file icons and keyboard navigation stay in the host.
export type UiFileTreeProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The tree, from `Workspace:OpenTree`.
    tree: FileTreeModel,
    --- The selected row.
    selectedPath: string?,
    --- A row to highlight, such as the open file.
    highlightedPath: string?,
    --- Runs as the user picks a file, with `event.path`.
    onSelect: UiHandler?,
    --- Runs on a secondary click, with `event.path`.
    onContextMenu: UiHandler?,
}

--- `ui.genUi` props. Native: the outline parser and its animated widgets stay in the host.
export type UiGenUiProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- The ```` ```ui ```` outline source.
    source: string,
    --- The source is still growing.
    streaming: boolean?,
}

--- `ui.progressCircle` props: the app's progress ring.
export type UiProgressCircleProps = {
    --- Identity among siblings across renders, and the anchor key for popovers and menus.
    key: string?,
    --- Width.
    w: UiSize?,
    --- Height.
    h: UiSize?,
    --- Minimum width.
    minW: number?,
    --- Minimum height.
    minH: number?,
    --- Maximum width.
    maxW: number?,
    --- Maximum height.
    maxH: number?,
    --- Share of a stack's free space.
    grow: number?,
    --- May shrink below its content size.
    shrink: boolean?,
    --- Padding on every side.
    pad: number?,
    --- Left and right padding.
    px: number?,
    --- Top and bottom padding.
    py: number?,
    --- Top padding.
    pt: number?,
    --- Right padding.
    pr: number?,
    --- Bottom padding.
    pb: number?,
    --- Left padding.
    pl: number?,
    --- Margin on every side.
    m: number?,
    --- Left and right margin.
    mx: number?,
    --- Top and bottom margin.
    my: number?,
    --- Top margin.
    mt: number?,
    --- Right margin.
    mr: number?,
    --- Bottom margin.
    mb: number?,
    --- Left margin.
    ml: number?,
    --- Takes the node out of flow, at these offsets from its parent's edges.
    pin: UiPin?,
    --- 0 to 1.
    opacity: number?,
    --- The accessibility role.
    role: Role?,
    --- The accessible name.
    label: string?,
    --- A tooltip.
    help: string?,
    --- Where the tooltip shows. Defaults to below.
    helpSide: Side?,
    --- How full, 0 to 100. The arc runs clockwise from twelve o'clock.
    percentage: number,
    --- Its look. Defaults to `compact`.
    appearance: ProgressAppearance?,
    --- Width and height in points. Defaults to the appearance's own: 14 or 16.
    size: number?,
    --- The stroke in the ring's own units. Defaults to 1.5 for `compact`, 3 for `indicator`.
    strokeWidth: number?,
    --- The track's color. Defaults to `surface-4`.
    track: Color?,
    --- `indicator` only: a color over the track. Defaults to none.
    overlay: Color?,
    --- The arc's color. Defaults to `icon-base`.
    progress: Color?,
}

--- The interface language.
export type LocaleInfo = {
    --- The app's id for it, such as `"de"` or `"zht"`: the name of its catalog file, `locales/<id>.json`.
    id: string,
    --- Its BCP 47 tag, such as `"de-DE"` or `"zh-Hant"`.
    tag: string,
    --- Its name in itself, such as `"Deutsch"`.
    name: string,
    --- Which way it reads.
    direction: TextDirection,
}

--- How `I18n:Number` writes a number.
export type NumberFormat = {
    --- Defaults to `decimal`.
    style: NumberStyle?,
    --- Defaults to `standard`.
    notation: NumberNotation?,
    --- An ISO 4217 code for `currency`. Defaults to `"USD"`.
    currency: string?,
    --- The most fraction digits. Defaults to 3 for `decimal`, 2 for `currency`, 0 for `percent` and `compact`.
    maxFraction: number?,
}

--- A session's token counts at its last step.
export type SessionUsage = {
    --- Input tokens.
    input: number,
    --- Output tokens.
    output: number,
    --- Reasoning tokens.
    reasoning: number,
    --- Tokens read from the cache.
    cacheRead: number,
    --- Tokens written to the cache.
    cacheWrite: number,
}

--- A session as the app holds it.
export type Session = {
    --- The session id.
    id: string,
    --- The key of the server that owns it.
    serverId: string,
    --- Its workspace folder.
    directory: string,
    --- Its title.
    title: string,
    --- The parent session of a subagent's session.
    parentId: string?,
    --- A turn is running.
    busy: boolean,
    --- Changes whenever anything here does: put it in a memo's version.
    version: number,
    --- What it cost in USD, its subagents' sessions included.
    cost: number,
    --- Token counts at the last step with tokens.
    usage: SessionUsage?,
    --- The provider of the step `usage` came from.
    usageProviderId: string?,
    --- The model of the step `usage` came from.
    usageModelId: string?,
    --- The provider of the model the session runs with.
    providerId: string?,
    --- The model the session runs with.
    modelId: string?,
    --- When it was created, in milliseconds since the Unix epoch.
    created: number?,
    --- When it last changed, in milliseconds since the Unix epoch.
    updated: number?,
    --- How many messages the app holds for it. Nil before it has read them.
    messages: number?,
}

--- A model as the server's catalog names it.
export type ModelInfo = {
    --- The provider's name. Nil when the catalog does not list the provider.
    providerName: string?,
    --- The model's name. Nil when the catalog does not list it.
    modelName: string?,
    --- Its context window in tokens.
    contextLimit: number?,
    --- Its output limit in tokens.
    outputLimit: number?,
    --- Any of its token prices is above zero.
    paid: boolean?,
}

--- A cached server read.
export type Resource = {
    --- Its state.
    status: ResourceStatus,
    --- The answer as JSON, when it is JSON.
    json: any?,
    --- The answer as text.
    text: string?,
    --- Why it failed.
    error: string?,
    --- Changes with every new answer: put it in a memo's version.
    generation: number,
}

--- A server's answer.
export type ServerResponse = {
    --- A 2xx status.
    ok: boolean,
    --- The HTTP status. 0 when nothing answered.
    status: number,
    --- The body as JSON, when it is JSON.
    json: any?,
    --- The body as text.
    text: string,
    --- Why it failed.
    error: string?,
}

--- One event from the server's event stream.
export type ServerEvent = {
    --- The event type, such as `"session.updated"`.
    type: string,
    --- The event's payload.
    data: any,
    --- The key of the server that sent it.
    serverId: string,
    --- The workspace folder it concerns.
    directory: string?,
}

--- A server the app lists.
export type ServerInfo = {
    --- Its key: `local`, or `<plugin>:<id>` for a contributed server.
    key: string,
    --- Its display name.
    name: string,
    --- Its badge, such as `"SSH"`.
    label: string?,
    --- Where it stands.
    state: ServerState,
    --- It runs on this machine.
    isLocal: boolean,
    --- This window is connected to it.
    current: boolean,
    --- The version it reported.
    version: string?,
}

--- A server's latest health check.
export type ServerHealth = {
    --- It answered.
    healthy: boolean,
    --- The version it reported.
    version: string?,
    --- This app does not support that version.
    incompatible: boolean?,
    --- A check is in flight.
    checking: boolean?,
}

--- How to reach a ready server.
export type ServerEndpoint = {
    --- The base URL.
    url: string,
    --- The HTTP basic password. The user name is always `opencode`.
    password: string?,
}

--- One server a server source lists. Its key is `<plugin>:<id>`.
export type ServerEntry = {
    --- The entry's id within the source.
    id: string,
    --- The display name.
    name: string,
    --- A short badge after the name, such as `"WSL"`.
    label: string?,
    --- Where it stands.
    state: ServerState,
    --- False keeps it out of the app's server list. Settings still shows it. Defaults to true.
    listed: boolean?,
    --- Its endpoint, once `ready`.
    http: ServerEndpoint?,
}

--- HTTP basic credentials.
export type BasicAuth = {
    --- The user name.
    user: string,
    --- The password.
    password: string,
}

--- An HTTP request.
export type HttpRequest = {
    --- The URL.
    url: string,
    --- Defaults to GET.
    method: HttpMethod?,
    --- Request headers.
    headers: { [string]: string }?,
    --- A text body.
    body: string?,
    --- A JSON body, instead of `body`. Sets the content type.
    json: any?,
    --- Sends `Authorization: Basic`.
    basicAuth: BasicAuth?,
    --- Seconds before it gives up. Defaults to 30.
    timeout: number?,
}

--- An HTTP answer.
export type HttpResponse = {
    --- A 2xx status.
    ok: boolean,
    --- The HTTP status. 0 when nothing answered.
    status: number,
    --- Response headers, with lowercase names.
    headers: { [string]: string },
    --- The body as text.
    text: string,
    --- The body as JSON, when it is JSON.
    json: any?,
    --- Why it failed.
    error: string?,
}

--- A process to start.
export type ProcessSpec = {
    --- The program, as a path or a name on PATH.
    program: string,
    --- Its arguments, passed as they are, without a shell.
    args: { string }?,
    --- Variables to add to the app's environment.
    env: { [string]: string }?,
    --- Its working folder.
    cwd: string?,
    --- Written to its stdin first. Stdin then closes unless `keepStdin` is set.
    stdin: string?,
    --- Keeps stdin open for `Write`.
    keepStdin: boolean?,
    --- Seconds before the host kills it.
    timeout: number?,
    --- How it encodes its output.
    encoding: TextEncoding?,
}

--- How a process ended.
export type ProcessExit = {
    --- Its exit code, when it has one.
    code: number?,
    --- Why it could not start.
    error: string?,
    --- Its timeout killed it.
    timedOut: boolean,
    --- `Kill`, or the plugin going away, ended it.
    killed: boolean,
}

--- A finished process and all its output.
export type ProcessOutput = {
    --- It ran and exited with 0.
    ok: boolean,
    --- Its exit code.
    code: number?,
    --- Everything it wrote to stdout.
    stdout: string,
    --- Everything it wrote to stderr.
    stderr: string,
    --- Why it could not start.
    error: string?,
    --- Its timeout killed it.
    timedOut: boolean,
}

--- A process the host supervises. The host kills it when the plugin half that started it goes away.
export type ProcessHandle = {
    --- The host's id for it.
    Id: number,
    --- It has not exited.
    Running: (self: ProcessHandle) -> boolean,
    --- Writes to its stdin, then closes stdin when `close` is true. False when stdin is closed.
    Write: (self: ProcessHandle, text: string, close: boolean?) -> boolean,
    --- Kills it. `Exited` follows.
    Kill: (self: ProcessHandle) -> (),
    --- Yields until it exits. Yields: call it from a handler, a task or a signal callback.
    WaitAsync: (self: ProcessHandle) -> ProcessExit,
    --- Fires with each chunk of output, line-buffered.
    Output: Signal<ProcessStream, string>,
    --- Fires once as it exits.
    Exited: Signal<ProcessExit>,
}

--- A value the host saves for this plugin with the app's state.
export type StoreValue<T> = {
    --- The value, or the default.
    Get: (self: StoreValue<T>) -> T,
    --- Saves a new value. It must be plain data: nil, booleans, numbers, strings and tables of them.
    Set: (self: StoreValue<T>, value: T) -> (),
    --- Changes with every `Set`: put it in a memo's version.
    Version: (self: StoreValue<T>) -> number,
    --- Fires with the new value, also when another window sets it.
    Changed: Signal<T>,
}

--- A value the host saves for this plugin per session.
export type SessionStoreValue<T> = {
    --- The session's value, or the default.
    Get: (self: SessionStoreValue<T>, sessionId: string) -> T,
    --- Saves a new value for the session. It must be plain data.
    Set: (self: SessionStoreValue<T>, sessionId: string, value: T) -> (),
    --- Changes with every `Set`: put it in a memo's version.
    Version: (self: SessionStoreValue<T>) -> number,
    --- Fires with the session id and its new value.
    Changed: Signal<string, T>,
}

--- A button on a toast.
export type ToastAction = {
    --- The button's label.
    label: string,
    --- Shows this path in the system's file manager.
    revealPath: string?,
    --- Opens this URL in the system's browser.
    openUrl: string?,
    --- Runs when the user clicks it. The toast then closes.
    run: (() -> ())?,
}

--- What a toast shows.
export type ToastSpec = {
    --- The headline.
    title: string,
    --- More detail under the title.
    description: string?,
    --- The tone. Defaults to `default`.
    variant: ToastVariant?,
    --- An icon before the title.
    icon: IconName?,
    --- Seconds before it closes. Ignored when `persistent`. Defaults to 5.
    duration: number?,
    --- Stays until the user or `Dismiss` closes it.
    persistent: boolean?,
    --- Buttons, in order.
    actions: { ToastAction }?,
}

--- One toast the plugin showed. It closes when the plugin goes away.
export type ToastHandle = {
    --- Closes it. Does nothing once it closed.
    Dismiss: (self: ToastHandle) -> (),
    --- Replaces what it shows, in place.
    Update: (self: ToastHandle, spec: ToastSpec) -> (),
}

--- Where to open one of this plugin's popovers.
export type PopoverOpenSpec = {
    --- The popover's id, from `ctx:AddPopover`.
    id: string,
    --- The `key` of the element it opens beside.
    anchorKey: string,
    --- The anchor's side. Defaults to below.
    side: Side?,
    --- Alignment along that side. Defaults to start.
    align: Align?,
    --- Points between the anchor and the popover.
    gap: number?,
    --- Passed to the popover's render as `props.data`.
    data: { [string]: any }?,
    --- How it appears.
    transition: Transition?,
}

--- One entry of a menu.
export type MenuEntry = {
    --- Defaults to `item`.
    kind: MenuEntryKind?,
    --- An item's id, returned when the user picks it.
    id: string?,
    --- An item's label, or a group's title.
    title: string?,
    --- An item's icon.
    icon: IconName?,
    --- Muted text at an item's end.
    detail: string?,
    --- A published command id whose shortcut the item shows.
    keybind: string?,
    --- False shows the item disabled. Defaults to true.
    enabled: boolean?,
    --- A destructive item.
    danger: boolean?,
    --- Shows a check mark.
    checked: boolean?,
}

--- A menu to open beside an element.
export type MenuOpenSpec = {
    --- The `key` of the element it opens beside.
    anchorKey: string,
    --- Its entries.
    items: { MenuEntry },
    --- Its accessible name.
    label: string?,
    --- The anchor's side. Defaults to below.
    side: Side?,
    --- Alignment along that side. Defaults to start.
    align: Align?,
    --- Its minimum width in points.
    minWidth: number?,
}

--- How to open a panel tab.
export type OpenPanelOptions = {
    --- The session. Defaults to the routed session.
    sessionId: string?,
    --- The tab of an instance panel.
    instance: string?,
    --- Opens the tab without selecting it or switching the narrow-screen view.
    background: boolean?,
    --- How the side region opens.
    transition: Transition?,
}

--- A local link the app routes to a link handler.
export type Link = {
    --- The target: a path, a URL or a `file://` link.
    href: string,
    --- The plugin that produced the linked item.
    origin: string?,
    --- The path is a known workspace file, not a guess from text.
    exact: boolean?,
    --- Must not switch the narrow-screen view.
    background: boolean?,
    --- The workspace-relative folder the link was written in.
    base: string?,
    --- The session the link belongs to.
    sessionId: string?,
}

--- A system notification. It follows the user's notification settings.
export type NotificationSpec = {
    --- The title.
    title: string,
    --- The text under it.
    body: string?,
    --- Clicking it opens this session.
    sessionId: string?,
    --- Plays the user's notification sound.
    sound: boolean?,
}

--- The window's theme.
export type ThemeInfo = {
    --- The theme's key.
    id: string,
    --- Its display name.
    name: string,
    --- The color scheme in use.
    scheme: ColorScheme,
}

--- Where a file was saved.
export type SaveResult = {
    --- Its path.
    path: string?,
    --- Why it could not be saved.
    error: string?,
}

--- A file's text.
export type ReadResult = {
    --- Its text, as UTF-8.
    text: string?,
    --- Why it could not be read.
    error: string?,
}

--- An app that can open a path, such as an editor or a file manager.
export type OpenInApp = {
    --- Its id, for `Workspace:OpenIn`.
    id: string,
    --- Its display name.
    name: string,
    --- Its icon.
    icon: ImageId?,
}

--- A file open for `ui.codeView`. The host closes it when the plugin half goes away.
export type CodeDocument = {
    --- Its path.
    Path: string,
    --- Changes when its content does: put it in a memo's version.
    Version: (self: CodeDocument) -> number,
    --- The file no longer exists.
    Missing: (self: CodeDocument) -> boolean,
    --- The file is not text.
    Binary: (self: CodeDocument) -> boolean,
    --- Reads it again.
    Reload: (self: CodeDocument) -> (),
    --- Releases it.
    Close: (self: CodeDocument) -> (),
    --- Fires when its content changes on disk.
    Changed: Signal<()>,
}

--- A folder's tree for `ui.fileTree`.
export type FileTreeModel = {
    --- The root folder.
    Root: string,
    --- Expands a folder.
    Expand: (self: FileTreeModel, path: string) -> (),
    --- Collapses a folder.
    Collapse: (self: FileTreeModel, path: string) -> (),
    --- Shows only paths that match a fuzzy query. An empty query shows all.
    SetFilter: (self: FileTreeModel, query: string) -> (),
    --- Reads the folder again.
    Refresh: (self: FileTreeModel) -> (),
    --- Changes when the tree does.
    Version: (self: FileTreeModel) -> number,
    --- Releases it.
    Close: (self: FileTreeModel) -> (),
    --- Fires when the tree changes.
    Changed: Signal<()>,
}

--- A diff's size.
export type DiffTotals = {
    --- Changed files.
    files: number,
    --- Added lines.
    added: number,
    --- Removed lines.
    removed: number,
}

--- One changed file.
export type DiffFile = {
    --- Its path.
    path: string,
    --- `added`, `modified`, `deleted` or `renamed`.
    status: string,
    --- Added lines.
    added: number,
    --- Removed lines.
    removed: number,
}

--- Which changes to open.
export type DiffOpenSpec = {
    --- The session.
    sessionId: string,
    --- What to compare.
    source: DiffSource,
    --- A `turn` source's user message.
    messageId: string?,
}

--- A set of changes for `ui.diffView`.
export type DiffDocument = {
    --- Its size.
    Totals: (self: DiffDocument) -> DiffTotals,
    --- Its files.
    Files: (self: DiffDocument) -> { DiffFile },
    --- Compares something else.
    SetSource: (self: DiffDocument, source: DiffSource, messageId: string?) -> (),
    --- A read is in flight.
    Loading: (self: DiffDocument) -> boolean,
    --- Changes when the changes do.
    Version: (self: DiffDocument) -> number,
    --- Releases it.
    Close: (self: DiffDocument) -> (),
    --- Fires when the changes change.
    Changed: Signal<()>,
}

--- A terminal session to start on the server.
export type TerminalSpec = {
    --- The workspace folder.
    directory: string,
    --- The shell's working folder. Defaults to `directory`.
    cwd: string?,
    --- Its initial title.
    title: string?,
    --- A program to run instead of the user's shell.
    command: string?,
    --- Its arguments.
    args: { string }?,
}

--- A terminal session for `ui.terminalSurface`.
export type PtySession = {
    --- The server's id for it.
    Id: string,
    --- Its title, as the shell sets it.
    Title: (self: PtySession) -> string,
    --- Its process has not exited.
    Running: (self: PtySession) -> boolean,
    --- Types text into it.
    Write: (self: PtySession, text: string) -> (),
    --- Ends it.
    Close: (self: PtySession) -> (),
    --- Fires with the exit code as its process exits.
    Exited: Signal<number?>,
    --- Fires with the new title.
    TitleChanged: Signal<string>,
}

--- A page to open.
export type BrowserOpenSpec = {
    --- The URL.
    url: string,
    --- The session the page belongs to.
    sessionId: string?,
    --- Opens without showing it.
    background: boolean?,
}

--- A page in the app's browser, for `ui.browserSurface`.
export type BrowserTab = {
    --- The host's id for it.
    Id: string,
    --- The current URL.
    Url: (self: BrowserTab) -> string,
    --- The page title.
    Title: (self: BrowserTab) -> string,
    --- A load is in flight.
    Loading: (self: BrowserTab) -> boolean,
    --- History has an earlier page.
    CanGoBack: (self: BrowserTab) -> boolean,
    --- History has a later page.
    CanGoForward: (self: BrowserTab) -> boolean,
    --- Loads a URL.
    Navigate: (self: BrowserTab, url: string) -> (),
    --- Goes back.
    Back: (self: BrowserTab) -> (),
    --- Goes forward.
    Forward: (self: BrowserTab) -> (),
    --- Reloads.
    Reload: (self: BrowserTab) -> (),
    --- Stops loading.
    Stop: (self: BrowserTab) -> (),
    --- Closes the page.
    Close: (self: BrowserTab) -> (),
    --- Sends one DevTools protocol command and returns its result. Yields: call it from a handler, a task or a signal callback.
    DevtoolsAsync: (self: BrowserTab, method: string, params: { [string]: any }?) -> any,
    --- Captures the visible page. Yields: call it from a handler, a task or a signal callback.
    ScreenshotAsync: (self: BrowserTab) -> ImageId?,
    --- Fires with the new URL.
    Navigated: Signal<string>,
    --- Fires with the new title.
    TitleChanged: Signal<string>,
    --- Fires as a load starts and ends.
    LoadingChanged: Signal<boolean>,
    --- Fires as the page closes.
    Closed: Signal<()>,
}

--- An Office file the host lays out and draws off the UI thread.
export type OfficeDocument = {
    --- Its format.
    Kind: OfficeKind,
    --- Pages, slides, or the active sheet's pages.
    PageCount: (self: OfficeDocument) -> number,
    --- An `xlsx` file's sheet names.
    Sheets: (self: OfficeDocument) -> { string },
    --- Shows sheet `index`, from 1.
    SetSheet: (self: OfficeDocument, index: number) -> (),
    --- A slide's speaker notes.
    Notes: (self: OfficeDocument, page: number) -> string?,
    --- Draws page `page`, from 1, at `scale` (defaults to 2). Yields: call it from a handler, a task or a signal callback.
    RenderPageAsync: (self: OfficeDocument, page: number, scale: number?) -> ImageId?,
    --- Releases it.
    Close: (self: OfficeDocument) -> (),
}

--- Where the app updater stands.
export type UpdaterState = {
    --- The phase.
    phase: UpdaterPhase,
    --- The available version.
    version: string?,
    --- Download progress, 0 to 1.
    progress: number?,
    --- The release notes, as Markdown.
    notes: string?,
    --- Why it failed.
    error: string?,
}

--- Keeps the display awake while held.
export type PowerLock = {
    --- Lets the display sleep again.
    Release: (self: PowerLock) -> (),
}

--- A QR code's module matrix.
export type QrCode = {
    --- Modules per side.
    size: number,
    --- `size * size` modules, row by row: true is dark.
    modules: { boolean },
}

--- A credential prompt from a child process.
export type AskpassPrompt = {
    --- What the process asks.
    Prompt: string,
    --- The answer is a password.
    Secret: boolean,
    --- Answers it. Nil cancels.
    Respond: (self: AskpassPrompt, answer: string?) -> (),
}

--- A loopback endpoint that turns `SSH_ASKPASS` prompts into signals.
export type AskpassListener = {
    --- Environment variables to give the child process.
    Env: { [string]: string },
    --- Fires with each prompt.
    Prompted: Signal<AskpassPrompt>,
    --- Stops listening.
    Close: (self: AskpassListener) -> (),
}

--- A plugin's live costs.
export type PluginMetrics = {
    --- Luau heap in KB.
    memoryKb: number,
    --- The last render's cost in microseconds.
    renderUs: number,
    --- Share of memo lookups that reused their subtree, 0 to 1.
    memoHitRate: number,
}

--- A command's slash form in the composer.
export type CommandSlash = {
    --- The name after the slash, such as `open` for `/open`.
    name: string,
    --- The text after the name goes to `run`.
    arguments: boolean?,
    --- Lists the entry right after the slash entry with this name.
    after: string?,
}

--- A command in the palette, with an optional keybind and slash command. Published as `<plugin>.<id>`. Desktop's plugin API calls it `Command`.
export type CommandSpec = {
    --- Its id within the plugin.
    id: string,
    --- The palette label.
    title: string,
    --- The palette's second line.
    description: string?,
    --- More words the palette matches, separated by spaces.
    keywords: string?,
    --- The palette category.
    group: string?,
    --- The Settings › Shortcuts section. Defaults to `general`.
    section: CommandSection?,
    --- The default keybind, such as `"mod+shift+t"`.
    bind: string?,
    --- Also offers it as a slash command.
    slash: CommandSlash?,
    --- Keeps it out of the palette. The keybind still works.
    hidden: boolean?,
    --- Also lists it under Suggested.
    suggested: boolean?,
    --- Lists it when the palette opens without a query.
    featured: boolean?,
    --- False takes it out of the palette and turns its keybind off. Defaults to true.
    enabled: boolean?,
    --- The keybind also fires while a text field has focus.
    editable: boolean?,
    --- Offered only while a session is open.
    requiresSession: boolean?,
    --- Runs it, as a coroutine. `input` is the slash command's arguments. Return `false` before the first yield to decline the key, so it reaches its target; any other return handles it.
    run: (input: string?) -> ...any,
}

--- What a panel's render receives.
export type PanelProps = {
    --- The routed session.
    sessionId: string,
    --- Its workspace folder.
    directory: string,
    --- An instance panel's tab.
    instance: string?,
    --- The tab is selected.
    active: boolean,
    --- Where it renders.
    placement: "side" | "mobile",
    --- The pane's width in points: lay out for it as a container query would.
    width: number,
    --- The pane's height in points.
    height: number,
}

--- What a panel's `label` render receives.
export type PanelTabLabelProps = {
    --- The routed session.
    sessionId: string,
    --- An instance panel's tab.
    instance: string?,
    --- The tab is selected.
    active: boolean,
    --- The tab is the replaceable preview tab.
    preview: boolean,
}

--- One tab an instance panel lists.
export type PanelTab = {
    --- The instance id.
    instance: string,
    --- The accessible name, and the label without a `label` render.
    title: string,
    --- Listed before every other tab, never closed or dragged.
    pinned: boolean?,
    --- Struck through, such as a deleted file.
    missing: boolean?,
    --- The workspace file the tab shows.
    file: string?,
}

--- A panel: tabs in a session's side region. Desktop's plugin API calls it `Panel`.
export type PanelSpec = {
    --- Its id within the plugin.
    id: string,
    --- Its tab and launcher title.
    title: string,
    --- The launcher card's second line.
    description: string?,
    --- Its icon.
    icon: IconName?,
    --- Launcher order, ascending.
    order: number?,
    --- Asks for the wider session minimum while open.
    wide: boolean?,
    --- A width of its own for the side region, in points.
    width: number?,
    --- Its tabs are not restored.
    transient: boolean?,
    --- Offered in the + menu and the New tab view. Defaults to true.
    launcher: boolean?,
    --- Its tab sits at the front of the strip.
    first: boolean?,
    --- Its tab is selected when the selected tab closes.
    fallback: boolean?,
    --- Makes it an instance panel: lists the session's tabs from the stored instance ids in `open`.
    list: ((sessionId: string, open: { string }) -> { PanelTab })?,
    --- Draws a tab's content.
    render: (view: View, props: PanelProps) -> UiNode?,
    --- Draws the tab's label in the strip, instead of its icon and title.
    label: ((view: View, props: PanelTabLabelProps) -> UiNode?)?,
    --- Runs after the tab leaves the strip.
    onClose: ((props: PanelProps) -> ())?,
    --- Runs as the tab becomes selected.
    onFocus: ((props: PanelProps) -> ())?,
}

--- A row in Settings › Plugins, under this plugin.
export type SettingSpec = {
    --- Its search id.
    id: string,
    --- The row's title.
    title: string,
    --- A line under the title.
    description: string?,
    --- More words settings search matches.
    keywords: string?,
    --- Draws the row's control.
    render: (view: View) -> UiNode?,
}

--- A settings search entry, indexed without drawing the page.
export type SettingEntry = {
    --- The `ui.settingsRow` id search reveals.
    id: string,
    --- The title in search results.
    title: string,
    --- Its second line.
    description: string?,
    --- More words search matches.
    keywords: string?,
}

--- What a settings page's render receives.
export type SettingsPageProps = {
    --- The entry id settings search is revealing.
    target: string?,
}

--- A settings page, or a section on a host page. Desktop's plugin API calls it `SettingsPage`.
export type SettingsPageSpec = {
    --- The page's id, also for `Layout:OpenSettings`.
    id: string,
    --- Its navigation label.
    title: string,
    --- Its navigation icon.
    icon: IconName?,
    --- Adds a section to this host page instead of a page of its own.
    page: HostSettingsPage?,
    --- Lists the page in the About group, such as release notes.
    about: boolean?,
    --- Search entries.
    entries: { SettingEntry }?,
    --- Draws the page or section.
    render: (view: View, props: SettingsPageProps) -> UiNode?,
}

--- An item of a host menu. Desktop's plugin API calls it `MenuItem`.
export type MenuItemSpec = {
    --- The menu.
    menu: MenuName,
    --- Its id within the plugin.
    id: string,
    --- Its label.
    title: string,
    --- Ascending order.
    order: number?,
    --- `session.panel`: its icon.
    icon: IconName?,
    --- `session.panel`: a published command id whose shortcut it shows.
    keybind: string?,
    --- `session.panel`: muted detail in launcher lists.
    description: string?,
    --- `session.panel`: + runs this item directly.
    primary: boolean?,
    --- `server.row`: lists it only for rows where it returns true.
    when: ((target: string) -> boolean)?,
    --- `server.row`: shows it disabled while it returns false.
    enabled: ((target: string) -> boolean)?,
    --- Runs it. `server.row` passes the row's server key.
    run: (target: string?) -> (),
}

--- An item of the app menu bar. Desktop's plugin API calls it `MenubarItem`.
export type MenubarItemSpec = {
    --- The submenu.
    menu: MenubarMenu,
    --- Its id within the plugin.
    id: string,
    --- Its label.
    label: string,
    --- Ascending order.
    order: number?,
    --- Its accelerator, such as `"mod+,"`.
    bind: string?,
    --- False shows it disabled.
    enabled: boolean?,
    --- Runs it.
    run: () -> (),
}

--- Opens local links it matches. The matching handler with the highest priority wins. Desktop's plugin API calls it `LinkHandler`.
export type LinkHandlerSpec = {
    --- Its id within the plugin.
    id: string,
    --- Higher wins. Defaults to 0.
    priority: number?,
    --- Whether it opens the link. It must not yield.
    match: (link: Link) -> boolean,
    --- Opens a link `match` accepted.
    open: (link: Link) -> (),
    --- Whether the target exists, so text renders as a link. It must not read file contents.
    exists: ((link: Link) -> boolean)?,
}

--- What a titlebar item shows.
export type TitlebarItemView = {
    --- The visible label.
    label: string,
    --- The accessible name, when it differs from the label.
    title: string?,
    --- Its icon. Defaults to a download arrow.
    icon: IconName?,
    --- Shows a spinner and disables it.
    busy: boolean?,
    --- Shows it pressed.
    pressed: boolean?,
}

--- A pill at the titlebar's end. Desktop's plugin API calls it `TitlebarItem`.
export type TitlebarItemSpec = {
    --- Its id within the plugin.
    id: string,
    --- What it shows, or nil to hide it. Called again after `Contribution:Refresh` and after any signal the plugin fires.
    show: () -> TitlebarItemView?,
    --- Runs when the user clicks it.
    run: (() -> ())?,
}

--- What a composer pill receives.
export type ComposerPillProps = {
    --- The session whose composer it sits above.
    sessionId: string,
    --- Its workspace folder.
    directory: string,
}

--- One row of the menu a composer pill opens.
export type ComposerPillRow = {
    --- Its id within the pill.
    id: string,
    --- Its text.
    label: string,
    --- Its icon.
    icon: IconName?,
    --- Colors its icon.
    tone: Tone?,
    --- Muted text at its end.
    detail: string?,
    --- Runs when the user picks it.
    run: (() -> ())?,
}

--- What a composer pill shows above one session's composer.
export type ComposerPillView = {
    --- The capsule's text.
    label: string,
    --- The capsule's icon.
    icon: IconName?,
    --- Colors the icon.
    tone: Tone?,
    --- The accessible name and the popover heading, when it differs from `label`.
    title: string?,
    --- The menu it opens, when the pill has no `renderMenu`.
    rows: { ComposerPillRow }?,
}

--- A pill in the row above a session's composer. Desktop's plugin API calls it `ComposerPill`.
export type ComposerPillSpec = {
    --- Its id within the plugin.
    id: string,
    --- Ascending order among plugin pills.
    order: number?,
    --- What it shows above this session's composer, or nil for nothing. It must not yield.
    show: (props: ComposerPillProps) -> ComposerPillView?,
    --- Content for the popover the pill opens. The host owns the capsule, the popover, its heading and its dismissal.
    renderMenu: ((view: View, props: ComposerPillProps) -> UiNode?)?,
    --- Runs on a click when the pill has no menu.
    run: ((props: ComposerPillProps) -> ())?,
}

--- One complete fenced code block.
export type CodeBlockProps = {
    --- The source between the fences.
    code: string,
    --- The fence's language, lowercased. Empty when it has none.
    language: string,
    --- The session that shows it.
    sessionId: string?,
}

--- Renders the fenced code blocks of some languages as your own UI. Desktop's plugin API calls it `CodeBlock`.
export type CodeBlockSpec = {
    --- Its id within the plugin.
    id: string,
    --- The languages it renders. The host resolves no aliases.
    languages: { string },
    --- Draws a block. Return nil to keep the highlighted code.
    render: (view: View, props: CodeBlockProps) -> UiNode?,
}

--- A button beside a code block's Copy button. Desktop's plugin API calls it `CodeBlockAction`.
export type CodeBlockActionSpec = {
    --- Its id within the plugin.
    id: string,
    --- The languages whose blocks show it.
    languages: { string },
    --- Its icon.
    icon: IconName,
    --- Its accessible name and tooltip.
    title: string,
    --- Ascending order, before Copy.
    order: number?,
    --- Runs on a click.
    run: (props: CodeBlockProps) -> (),
}

--- What a slot's render receives.
export type SlotProps = {
    --- Session slots: the session.
    sessionId: string?,
    --- Session slots: its workspace folder.
    directory: string?,
    --- `session.header`: the timeline is on screen.
    active: boolean?,
    --- `session.header`: the title needs the room, so draw the compact form.
    compact: boolean?,
    --- `session.location.menu`: the menu is open.
    shown: boolean?,
}

--- Content for one of the shell's slots. Desktop's plugin API calls it `Slot`.
export type SlotSpec = {
    --- Its id within the plugin.
    id: string,
    --- The slot.
    at: SlotName,
    --- Ascending order.
    order: number?,
    --- Draws its content, or nil for nothing.
    render: (view: View, props: SlotProps) -> UiNode?,
}

--- The seed colors a theme variant generates its scales from, as `#RRGGBB` strings.
export type ThemeSeeds = {
    --- The gray scale's base.
    neutral: string,
    --- The brand and accent color.
    primary: string,
    --- Success states.
    success: string,
    --- Warning states.
    warning: string,
    --- Error states.
    error: string,
    --- Informational states.
    info: string,
    --- Links and interactive elements.
    interactive: string,
    --- Added lines.
    diffAdd: string,
    --- Deleted lines.
    diffDelete: string,
}

--- One color scheme of a theme.
export type ThemeVariant = {
    --- The seed colors.
    seeds: ThemeSeeds,
    --- Exact values for single tokens of the app's theme file format.
    overrides: { [string]: string }?,
}

--- A color theme, listed in Settings › Appearance as `<plugin>:<id>`. Desktop's plugin API calls it `Theme`.
export type ThemeSpec = {
    --- Its id within the plugin.
    id: string,
    --- The name the theme list shows.
    name: string,
    --- The light scheme.
    light: ThemeVariant,
    --- The dark scheme.
    dark: ThemeVariant,
}

--- What a file viewer's render receives.
export type FileViewerProps = {
    --- The file.
    path: string,
    --- The session that opened it.
    sessionId: string?,
    --- Its workspace folder.
    directory: string,
}

--- Shows files with some extensions in the file panel. Desktop's plugin API calls it `FileViewer`.
export type FileViewerSpec = {
    --- Its id within the plugin.
    id: string,
    --- Its name, such as `"Word document"`.
    title: string,
    --- The extensions it shows, without the dot.
    extensions: { string },
    --- Draws the file.
    render: (view: View, props: FileViewerProps) -> UiNode?,
}

--- What a server source's row render receives.
export type ServerRowProps = {
    --- The entry's id.
    id: string,
    --- Its key: `<plugin>:<id>`.
    key: string,
    --- Its latest health check.
    health: ServerHealth?,
}

--- A source of servers, such as SSH hosts or WSL distributions. Desktop's plugin API calls it `Server`.
export type ServerSourceSpec = {
    --- Its id within the plugin.
    id: string,
    --- Its name in Settings › Servers.
    title: string,
    --- Ascending order among sources.
    order: number?,
    --- Draws an entry's connection row in Settings › Servers. Window halves only.
    row: ((view: View, props: ServerRowProps) -> UiNode?)?,
    --- Starts an entry that is not ready, as a coroutine. Return true once it is.
    connect: ((id: string) -> boolean)?,
    --- Runs before the host forgets an entry.
    remove: ((id: string) -> ())?,
}

--- A server source a plugin added.
export type ServerSource = {
    --- Replaces its entries. Once `ready`, the list is complete and the host forgets servers it no longer lists.
    SetEntries: (self: ServerSource, entries: { ServerEntry }, ready: boolean) -> (),
    --- Removes the source.
    Withdraw: (self: ServerSource) -> (),
}

--- What a dialog's render receives.
export type DialogProps = {
    --- What `Dialog:Open` passed.
    data: { [string]: any }?,
}

--- A modal dialog, opened with `Dialog:Open`.
export type DialogSpec = {
    --- Its id within the plugin.
    id: string,
    --- Its heading and accessible name.
    title: string?,
    --- Its width.
    size: DialogSize?,
    --- Draws its body inside the host's dialog frame.
    render: (view: View, props: DialogProps) -> UiNode?,
    --- Runs when Esc or a click outside closes it.
    onDismiss: (() -> ())?,
}

--- What a popover's render receives.
export type PopoverProps = {
    --- What `Popover:Open` or `ui.flyoutRow` passed.
    data: { [string]: any }?,
}

--- Popover content, opened with `Popover:Open` or `ui.flyoutRow`. The host owns placement, focus and dismissal.
export type PopoverSpec = {
    --- Its id within the plugin.
    id: string,
    --- Its heading and accessible name.
    title: string?,
    --- Its width in points.
    width: number?,
    --- Draws its content.
    render: (view: View, props: PopoverProps) -> UiNode?,
}

--- A tool on the app's client MCP server, named `<plugin>_<name>`. Desktop's plugin API calls it `McpTool`.
export type McpToolSpec = {
    --- Its name within the plugin.
    name: string,
    --- What it does, for the model.
    description: string,
    --- Its arguments' JSON Schema.
    inputSchema: { [string]: any },
    --- Runs it as a coroutine and returns its result: a string, or a table sent as JSON. Raise an error to fail.
    call: (args: { [string]: any }) -> any,
}

-- Services ----------------------------------------------------------------------------------------

--- The sessions the app holds. Both halves: `ctx:GetService("Sessions")`.
export type SessionsService = {
    --- A session by id.
    Get: (self: SessionsService, sessionId: string) -> Session?,
    --- The session this window shows. Always nil in the app half.
    Routed: (self: SessionsService) -> Session?,
    --- The sessions of open tabs.
    Open: (self: SessionsService) -> { Session },
    --- Fires with a session after it changes.
    Changed: Signal<Session>,
    --- Fires as this window routes another session, or none.
    RoutedChanged: Signal<Session?>,
}

--- The server's model catalog. Both halves: `ctx:GetService("Models")`.
export type ModelsService = {
    --- How the catalog names a model.
    Get: (self: ModelsService, providerId: string, modelId: string) -> ModelInfo,
    --- Fires when the catalog changes.
    Changed: Signal<()>,
}

--- The OpenCode server this window is connected to. In the app half, the app's own server. Both halves: `ctx:GetService("Server")`.
export type ServerService = {
    --- Sends a request with a JSON body, off the UI thread. Yields: call it from a handler, a task or a signal callback.
    RequestAsync: (self: ServerService, method: HttpMethod, path: string, body: any?) -> ServerResponse,
    --- A GET of `path`, cached until `version` changes. Safe in a render: it never yields, and the window redraws when the answer lands.
    Resource: (self: ServerService, path: string, version: MemoVersion) -> Resource,
    --- Drops this plugin's cached reads whose path starts with `prefix`.
    Invalidate: (self: ServerService, prefix: string) -> (),
    --- A signal for the server's events whose type starts with `prefix`.
    OnEvent: (self: ServerService, prefix: string) -> Signal<ServerEvent>,
    --- Percent-encodes text for a path or query.
    EncodeUri: (self: ServerService, text: string) -> string,
    --- The version it reported.
    Version: (self: ServerService) -> string?,
    --- It runs on this machine.
    IsLocal: (self: ServerService) -> boolean,
    --- The app may change things on it.
    CanWrite: (self: ServerService) -> boolean,
    --- Fires as its event connection goes up or down.
    Connected: Signal<boolean>,
}

--- The servers the app lists. Both halves: `ctx:GetService("Servers")`.
export type ServersService = {
    --- Every listed server.
    List: (self: ServersService) -> { ServerInfo },
    --- The key of the server this window is connected to.
    Current: (self: ServersService) -> string?,
    --- A server's latest health check.
    Health: (self: ServersService, key: string) -> ServerHealth?,
    --- Connects this window to a server and goes to its Home.
    Connect: (self: ServersService, key: string) -> (),
    --- Fires when the list or a server's state changes.
    Changed: Signal<()>,
}

--- HTTP requests off the UI thread. Both halves: `ctx:GetService("Http")`.
export type HttpService = {
    --- Sends a request. Yields: call it from a handler, a task or a signal callback.
    FetchAsync: (self: HttpService, request: HttpRequest) -> HttpResponse,
}

--- Child processes the host supervises. Both halves: `ctx:GetService("Process")`.
export type ProcessService = {
    --- Starts a process. A program that cannot start still returns a handle, whose `Exited` fires with the error.
    Spawn: (self: ProcessService, spec: ProcessSpec) -> ProcessHandle,
    --- Starts a process and yields until it exits, with all its output. Yields: call it from a handler, a task or a signal callback.
    RunAsync: (self: ProcessService, spec: ProcessSpec) -> ProcessOutput,
}

--- Values the host saves for this plugin. Both halves and every window share one copy. Both halves: `ctx:GetService("Store")`.
export type StoreService = {
    --- A global value.
    Value: <T>(self: StoreService, name: string, default: T) -> StoreValue<T>,
    --- A value per session.
    SessionValue: <T>(self: StoreService, name: string, default: T) -> SessionStoreValue<T>,
}

--- Typed values plugins share. A plugin may provide only ids under its own id. Both halves: `ctx:GetService("Contracts")`.
export type ContractsService = {
    --- Provides `value` under `id` (`<plugin>` or `<plugin>.<name>`). From the app half, for the whole process; from a window half, for that window.
    Provide: (self: ContractsService, id: string, value: any) -> Contribution,
    --- Follows a contract another plugin provides. Keep working while it is not active.
    Use: (self: ContractsService, id: string) -> Live<any>,
    --- Every contract id this half sees, with its state.
    Summary: (self: ContractsService) -> { [string]: LiveState },
}

--- The Studio console. `print` and `warn` write here too. Both halves: `ctx:GetService("Console")`.
export type ConsoleService = {
    --- Writes an information line.
    Log: (self: ConsoleService, ...any) -> (),
    --- Writes a warning.
    Warn: (self: ConsoleService, ...any) -> (),
    --- Writes an error, without raising one.
    Error: (self: ConsoleService, ...any) -> (),
}

--- Short messages in a corner of the window. From the app half, in the focused window. Both halves: `ctx:GetService("Toast")`.
export type ToastService = {
    --- Shows a toast.
    Show: (self: ToastService, spec: ToastSpec) -> ToastHandle,
    --- Shows a `default` toast.
    Info: (self: ToastService, title: string, description: string?) -> ToastHandle,
    --- Shows a `success` toast.
    Success: (self: ToastService, title: string, description: string?) -> ToastHandle,
    --- Shows an `error` toast that stays until dismissed.
    Error: (self: ToastService, title: string, description: string?) -> ToastHandle,
}

--- This plugin's popovers. The host places them, moves focus and closes them. The window half only: `ctx:GetService("Popover")`.
export type PopoverService = {
    --- Opens a popover beside an element, closing any other of this plugin's.
    Open: (self: PopoverService, spec: PopoverOpenSpec) -> (),
    --- Opens it, or closes it when it is open.
    Toggle: (self: PopoverService, spec: PopoverOpenSpec) -> (),
    --- Closes this plugin's open popover.
    Close: (self: PopoverService) -> (),
    --- Whether a popover is open.
    IsOpen: (self: PopoverService, id: string) -> boolean,
}

--- This plugin's modal dialogs. The window half only: `ctx:GetService("Dialog")`.
export type DialogService = {
    --- Opens a dialog, closing any other of this plugin's.
    Open: (self: DialogService, id: string, data: { [string]: any }?) -> (),
    --- Closes this plugin's open dialog.
    Close: (self: DialogService) -> (),
    --- Whether a dialog is open.
    IsOpen: (self: DialogService, id: string) -> boolean,
}

--- The app's own menus. The window half only: `ctx:GetService("Menu")`.
export type MenuService = {
    --- Opens a menu beside an element and yields until it closes. Returns the picked item's id, or nil. Yields: call it from a handler, a task or a signal callback.
    OpenAsync: (self: MenuService, spec: MenuOpenSpec) -> string?,
}

--- Panels, settings and drafts in this window. The window half only: `ctx:GetService("Layout")`.
export type LayoutService = {
    --- Opens or selects one of this plugin's panels.
    OpenPanel: (self: LayoutService, panelId: string, options: OpenPanelOptions?) -> (),
    --- Opens a panel, or closes it when it shows.
    TogglePanel: (self: LayoutService, panelId: string, options: OpenPanelOptions?) -> (),
    --- Closes a panel's tab.
    ClosePanel: (self: LayoutService, panelId: string, options: OpenPanelOptions?) -> (),
    --- Where a panel is in a session. Defaults to the routed session.
    PanelState: (self: LayoutService, panelId: string, sessionId: string?) -> PanelState,
    --- Opens Settings at one of this plugin's pages, or at the top.
    OpenSettings: (self: LayoutService, pageId: string?) -> (),
    --- Opens a new session's draft in a folder, with a prompt typed in.
    NewDraft: (self: LayoutService, directory: string, prompt: string?) -> (),
    --- The window is narrow-screen sized.
    Narrow: (self: LayoutService) -> boolean,
}

--- Routes local links to the plugins that handle them. The window half only: `ctx:GetService("Links")`.
export type LinksService = {
    --- Opens a link with the best handler. False when no handler matches.
    Open: (self: LinksService, link: Link) -> boolean,
    --- Whether a handler matches and says the target exists.
    Exists: (self: LinksService, link: Link) -> boolean,
}

--- Published commands, as the palette runs them. The window half only: `ctx:GetService("Commands")`.
export type CommandsService = {
    --- Runs a command by id: `<plugin>.<id>` or one of the app's. False when it is unknown or disabled.
    Run: (self: CommandsService, commandId: string, input: string?) -> boolean,
    --- A command's effective keybind.
    Keybind: (self: CommandsService, commandId: string) -> string?,
}

--- The theme and the user's appearance settings. Both halves: `ctx:GetService("Theme")`.
export type ThemeService = {
    --- The theme in use.
    Current: (self: ThemeService) -> ThemeInfo,
    --- The dark scheme is in use.
    IsDark: (self: ThemeService) -> boolean,
    --- A color as `#RRGGBBAA` for the current theme.
    Resolve: (self: ThemeService, color: Color) -> string,
    --- The user's font family name.
    Font: (self: ThemeService, family: FontFamily) -> string,
    --- Fires when the theme or scheme changes.
    Changed: Signal<ThemeInfo>,
}

--- The running app. Both halves: `ctx:GetService("App")`.
export type AppService = {
    --- The app's version.
    Version: string,
    --- Its release channel.
    Channel: Channel,
    --- The operating system.
    Platform: Platform,
    --- The interface language, such as `"en"`.
    Locale: (self: AppService) -> string,
    --- The value of command-line option `--name`.
    Arg: (self: AppService, name: string) -> string?,
    --- The folder the app writes its logs to.
    LogsDir: (self: AppService) -> string?,
    --- The folder the app saves downloads to.
    DownloadsDir: (self: AppService) -> string?,
    --- Fires with the new interface language.
    LocaleChanged: Signal<string>,
}

--- The clipboard, the system browser and file manager, and the clock. Both halves: `ctx:GetService("System")`.
export type SystemService = {
    --- Puts text on the clipboard.
    CopyText: (self: SystemService, text: string) -> (),
    --- Opens a URL in the system's browser.
    OpenExternal: (self: SystemService, url: string) -> (),
    --- Shows a path in the system's file manager.
    Reveal: (self: SystemService, path: string) -> (),
    --- The wall clock in milliseconds since the Unix epoch.
    Now: (self: SystemService) -> number,
    --- The local time zone's offset from UTC in seconds, at that time.
    UtcOffset: (self: SystemService, ms: number) -> number,
}

--- System notifications that follow the user's notification settings. Both halves: `ctx:GetService("Notifications")`.
export type NotificationsService = {
    --- Shows a notification when the window is not in front.
    Notify: (self: NotificationsService, spec: NotificationSpec) -> (),
}

--- Files in the workspace and the apps that open them. Both halves: `ctx:GetService("Workspace")`.
export type WorkspaceService = {
    --- Reads a text file through the server. Yields: call it from a handler, a task or a signal callback.
    ReadFileAsync: (self: WorkspaceService, path: string) -> ReadResult,
    --- Saves content as a file in the downloads folder. Yields: call it from a handler, a task or a signal callback.
    SaveDownloadAsync: (self: WorkspaceService, name: string, content: string) -> SaveResult,
    --- Opens a file for `ui.codeView`.
    OpenDocument: (self: WorkspaceService, path: string) -> CodeDocument,
    --- Opens a folder for `ui.fileTree`.
    OpenTree: (self: WorkspaceService, directory: string) -> FileTreeModel,
    --- The installed apps that can open a path.
    OpenInApps: (self: WorkspaceService) -> { OpenInApp },
    --- Opens a path in one of those apps.
    OpenIn: (self: WorkspaceService, appId: string, path: string) -> (),
    --- Fires with a workspace path after it changes on disk.
    FileChanged: Signal<string>,
}

--- Session changes for `ui.diffView`. The window half only: `ctx:GetService("Diff")`.
export type DiffService = {
    --- Opens a set of changes.
    Open: (self: DiffService, spec: DiffOpenSpec) -> DiffDocument,
}

--- Terminal sessions on the server, for `ui.terminalSurface`. Both halves: `ctx:GetService("Terminal")`.
export type TerminalService = {
    --- Starts a terminal session.
    Create: (self: TerminalService, spec: TerminalSpec) -> PtySession,
    --- Reattaches to a running session by id.
    Attach: (self: TerminalService, id: string) -> PtySession?,
}

--- Pages in the app's embedded browser. Pages belong to the process and survive a window closing when the app half opened them. Both halves: `ctx:GetService("Browser")`.
export type BrowserService = {
    --- Opens a page.
    Open: (self: BrowserService, spec: BrowserOpenSpec) -> BrowserTab,
    --- This plugin's open pages.
    Tabs: (self: BrowserService) -> { BrowserTab },
    --- The browser engine is installed and runs on this platform.
    Available: (self: BrowserService) -> boolean,
}

--- Office documents laid out and drawn by the host. Both halves: `ctx:GetService("Office")`.
export type OfficeService = {
    --- Opens a `.docx`, `.xlsx` or `.pptx` file. Nil when it cannot. Yields: call it from a handler, a task or a signal callback.
    OpenAsync: (self: OfficeService, path: string) -> OfficeDocument?,
}

--- The app's own updates. Both halves: `ctx:GetService("Updater")`.
export type UpdaterService = {
    --- Where it stands.
    State: (self: UpdaterService) -> UpdaterState,
    --- Checks for a newer version now.
    Check: (self: UpdaterService) -> (),
    --- Downloads the available version.
    Download: (self: UpdaterService) -> (),
    --- Restarts into the downloaded version.
    Install: (self: UpdaterService) -> (),
    --- Fires with each new state.
    StateChanged: Signal<UpdaterState>,
}

--- Developer diagnostics. Local and beta builds only; elsewhere every method returns nil. Both halves: `ctx:GetService("Diagnostics")`.
export type DiagnosticsService = {
    --- The process's memory counters.
    Memory: (self: DiagnosticsService) -> { [string]: any }?,
    --- A plugin's live costs. Defaults to this plugin.
    Metrics: (self: DiagnosticsService, pluginId: string?) -> PluginMetrics?,
    --- A developer setting, such as an experiment.
    Get: (self: DiagnosticsService, name: string) -> any,
    --- Changes a developer setting.
    Set: (self: DiagnosticsService, name: string, value: any) -> (),
    --- Writes a heap snapshot to the downloads folder. Yields: call it from a handler, a task or a signal callback.
    HeapSnapshotAsync: (self: DiagnosticsService) -> SaveResult,
}

--- Display sleep. Both halves: `ctx:GetService("Power")`.
export type PowerService = {
    --- Keeps the display awake until the lock is released or the plugin goes away.
    KeepAwake: (self: PowerService, reason: string) -> PowerLock,
}

--- QR codes. Both halves: `ctx:GetService("Qr")`.
export type QrService = {
    --- Encodes text, at medium error correction.
    Encode: (self: QrService, text: string) -> QrCode,
}

--- Credential prompts from child processes such as `ssh`. The app half only: `ctx:GetService("Askpass")`.
export type AskpassService = {
    --- Starts a loopback prompt endpoint.
    Listen: (self: AskpassService) -> AskpassListener,
}

--- Images handed to the host for `ui.image`. Both halves: `ctx:GetService("Images")`.
export type ImagesService = {
    --- A PNG, JPEG, GIF or WebP image, decoded off the UI thread. Nil when refused.
    FromEncoded: (self: ImagesService, data: buffer | string) -> ImageId?,
    --- An SVG image.
    FromSvg: (self: ImagesService, svg: string) -> ImageId?,
    --- Straight-alpha RGBA8 pixels.
    FromRgba: (self: ImagesService, pixels: buffer, width: number, height: number) -> ImageId?,
    --- Frees an image once nothing draws it.
    Release: (self: ImagesService, image: ImageId) -> (),
}

--- The plugin's own messages in the interface language, and numbers, dates and lists written the way the app writes them. A plugin's catalogs are `locales/<id>.json` in its folder: flat objects of message keys, the app's format. Both halves: `ctx:GetService("I18n")`.
export type I18nService = {
    --- Message `key` in the interface language, else in `locales/en.json`, else the key itself. `{{name}}` placeholders take `args`, and a missing one reads empty. With a number `args.count`, the form `key.<category>` for the count's plural category is used when the catalog has it, else `key.other`, else `key`.
    T: (self: I18nService, key: string, args: { [string]: any }?) -> string,
    --- The interface language's catalog, or the English one, has message `key`.
    Has: (self: I18nService, key: string) -> boolean,
    --- The interface language.
    Locale: (self: I18nService) -> LocaleInfo,
    --- Which way the interface language reads. Lay out rows from the reading start; the host mirrors its own layout.
    Direction: (self: I18nService) -> TextDirection,
    --- The plural category the interface language uses for `count`.
    Plural: (self: I18nService, count: number) -> PluralCategory,
    --- `value` with the language's digits, separators and signs.
    Number: (self: I18nService, value: number, format: NumberFormat?) -> string,
    --- The moment `ms` (milliseconds since the Unix epoch) in local time, in `style`.
    Date: (self: I18nService, ms: number, style: DateStyle) -> string,
    --- A relative time: `-3, "day"` reads "3 days ago", `2, "hour"` "in 2 hours".
    RelativeTime: (self: I18nService, value: number, unit: TimeUnit) -> string,
    --- "today", "yesterday" or "tomorrow" for 0, -1 and 1 days from today, in the language. Nil for other days.
    RelativeDay: (self: I18nService, days: number) -> string?,
    --- Items joined as the language's long conjunction: "a, b, and c".
    List: (self: I18nService, items: { string }) -> string,
    --- Fires as the interface language changes. Everything drawn redraws on its own; connect for state you keep.
    Changed: Signal<LocaleInfo>,
}

-- Plugin contexts ---------------------------------------------------------------------------------

export type WindowContextCreateSignal = <T...>(self: WindowContext) -> ScriptSignal<T...>

--- What a plugin half's setup function receives. The window half's: see `plugin.defineWindow`.
declare extern type WindowContext with
    --- The plugin's id.
    id: string
    --- Which half this is.
    half: Half
    --- The contract version the host implements, such as `"1.0"`.
    contract: string
    --- A signal this plugin fires itself.
    CreateSignal: WindowContextCreateSignal
    --- A host-owned text field's content. The n-th editor a setup creates is the same field after a hot reload.
    function CreateEditor(self, initial: string?): Editor
    --- Carries plain data across a hot reload: the host calls `save` on the old code and `load` on the new.
    function BindHotState(self, save: () -> any, load: (state: any) -> ()): ()
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Sessions"): SessionsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Models"): ModelsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Server"): ServerService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Servers"): ServersService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Http"): HttpService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Process"): ProcessService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Store"): StoreService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Contracts"): ContractsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Console"): ConsoleService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Toast"): ToastService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Popover"): PopoverService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Dialog"): DialogService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Menu"): MenuService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Layout"): LayoutService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Links"): LinksService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Commands"): CommandsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Theme"): ThemeService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "App"): AppService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "System"): SystemService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Notifications"): NotificationsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Workspace"): WorkspaceService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Diff"): DiffService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Terminal"): TerminalService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Browser"): BrowserService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Office"): OfficeService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Updater"): UpdaterService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Diagnostics"): DiagnosticsService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Power"): PowerService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Qr"): QrService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "Images"): ImagesService
    --- Returns a host service by name; each overload narrows the type. The app half alone has: Askpass.
    function GetService(self, name: "I18n"): I18nService
    --- Adds a command to the palette, its keybind and its slash command.
    function AddCommand(self, spec: CommandSpec): Contribution
    --- Adds a panel: tabs in the session's side region.
    function AddPanel(self, spec: PanelSpec): Contribution
    --- Adds a row to Settings › Plugins.
    function AddSetting(self, spec: SettingSpec): Contribution
    --- Adds a settings page, or a section on a host page.
    function AddSettingsPage(self, spec: SettingsPageSpec): Contribution
    --- Adds an item to a host menu.
    function AddMenuItem(self, spec: MenuItemSpec): Contribution
    --- Opens local links it matches.
    function AddLinkHandler(self, spec: LinkHandlerSpec): Contribution
    --- Adds a pill at the titlebar's end.
    function AddTitlebarItem(self, spec: TitlebarItemSpec): Contribution
    --- Adds a pill above the session composer.
    function AddComposerPill(self, spec: ComposerPillSpec): Contribution
    --- Renders fenced code blocks of some languages.
    function AddCodeBlock(self, spec: CodeBlockSpec): Contribution
    --- Adds a button beside code blocks' Copy button.
    function AddCodeBlockAction(self, spec: CodeBlockActionSpec): Contribution
    --- Fills one of the shell's slots.
    function AddSlot(self, spec: SlotSpec): Contribution
    --- Adds a color theme.
    function AddTheme(self, spec: ThemeSpec): Contribution
    --- Shows files with some extensions.
    function AddFileViewer(self, spec: FileViewerSpec): Contribution
    --- Adds a source of servers. Add it in the app half so every window shares one list.
    function AddServerSource(self, spec: ServerSourceSpec): ServerSource
    --- Adds a modal dialog, opened with `Dialog:Open`.
    function AddDialog(self, spec: DialogSpec): Contribution
    --- Adds popover content, opened with `Popover:Open` or `ui.flyoutRow`.
    function AddPopover(self, spec: PopoverSpec): Contribution
    --- Adds a tool to the app's client MCP server.
    function AddMcpTool(self, spec: McpToolSpec): Contribution
    --- Fires as the half goes away: the plugin turns off, reloads, or its window closes. The host disconnects every connection afterwards.
    Disposing: Signal<()>
    --- Fires as the window gains or loses focus.
    FocusChanged: Signal<boolean>
end

export type AppContextCreateSignal = <T...>(self: AppContext) -> ScriptSignal<T...>

--- What a plugin half's setup function receives. The app half's: see `plugin.defineApp`.
declare extern type AppContext with
    --- The plugin's id.
    id: string
    --- Which half this is.
    half: Half
    --- The contract version the host implements, such as `"1.0"`.
    contract: string
    --- A signal this plugin fires itself.
    CreateSignal: AppContextCreateSignal
    --- Carries plain data across a hot reload: the host calls `save` on the old code and `load` on the new.
    function BindHotState(self, save: () -> any, load: (state: any) -> ()): ()
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Sessions"): SessionsService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Models"): ModelsService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Server"): ServerService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Servers"): ServersService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Http"): HttpService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Process"): ProcessService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Store"): StoreService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Contracts"): ContractsService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Console"): ConsoleService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Toast"): ToastService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Theme"): ThemeService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "App"): AppService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "System"): SystemService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Notifications"): NotificationsService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Workspace"): WorkspaceService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Terminal"): TerminalService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Browser"): BrowserService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Office"): OfficeService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Updater"): UpdaterService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Diagnostics"): DiagnosticsService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Power"): PowerService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Qr"): QrService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Askpass"): AskpassService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "Images"): ImagesService
    --- Returns a host service by name; each overload narrows the type. The window half alone has: Popover, Dialog, Menu, Layout, Links, Commands, Diff.
    function GetService(self, name: "I18n"): I18nService
    --- Adds an item to the app menu bar.
    function AddMenubarItem(self, spec: MenubarItemSpec): Contribution
    --- Adds a source of servers. Add it in the app half so every window shares one list.
    function AddServerSource(self, spec: ServerSourceSpec): ServerSource
    --- Adds a tool to the app's client MCP server.
    function AddMcpTool(self, spec: McpToolSpec): Contribution
    --- Fires as the half goes away: the plugin turns off, reloads, or its window closes. The host disconnects every connection afterwards.
    Disposing: Signal<()>
end

-- Modules -----------------------------------------------------------------------------------------

--- `require("@opencode/ui")`: declarative node constructors. Nodes build host elements directly; there is no virtual DOM.
export type UiLibrary = {
    --- A container with no direction of its own: children stack on top of each other.
    box: (props: UiBoxProps, children: { UiNode }?) -> UiNode,
    --- A horizontal stack.
    row: (props: UiBoxProps, children: { UiNode }?) -> UiNode,
    --- A vertical stack.
    col: (props: UiBoxProps, children: { UiNode }?) -> UiNode,
    --- Flexible space in a stack.
    spacer: (props: UiSpacerProps?) -> UiNode,
    --- A hairline rule.
    divider: (props: UiDividerProps?) -> UiNode,
    --- Text.
    text: (content: string, props: UiTextProps?) -> UiNode,
    --- One of the app's icons.
    icon: (name: IconName, props: UiIconProps?) -> UiNode,
    --- An image the host holds.
    image: (props: UiImageProps) -> UiNode,
    --- A number shown as text. Hosts that can animate changed digits do.
    metric: (props: UiMetricProps) -> UiNode,
    --- Text with the app's loading shimmer.
    shimmer: (content: string, props: UiTextProps?) -> UiNode,
    --- A keyboard shortcut chip.
    keybind: (props: UiKeybindProps) -> UiNode,
    --- The app's own button.
    button: (props: UiButtonProps) -> UiNode,
    --- The app's own icon button.
    iconButton: (props: UiIconButtonProps) -> UiNode,
    --- The app's own switch.
    switch: (props: UiSwitchProps) -> UiNode,
    --- A radio row.
    radio: (props: UiRadioProps) -> UiNode,
    --- The app's own text field.
    field: (props: UiFieldProps) -> UiNode,
    --- The app's spinner.
    loader: (props: UiLoaderProps?) -> UiNode,
    --- The app's progress ring: a track and an arc from twelve o'clock, clockwise.
    progressCircle: (props: UiProgressCircleProps) -> UiNode,
    --- A status capsule.
    pill: (props: UiPillProps) -> UiNode,
    --- A settings row: title and description beside a control.
    settingsRow: (props: UiSettingsRowProps, control: UiNode?) -> UiNode,
    --- A card of settings rows.
    settingsList: (props: UiSettingsListProps, rows: { UiNode }) -> UiNode,
    --- A popover row that opens a nested popover.
    flyoutRow: (props: UiFlyoutRowProps) -> UiNode,
    --- Markdown, as the timeline draws a reply. Native: the host renders it.
    markdown: (props: UiMarkdownProps) -> UiNode,
    --- A long list that builds only its visible rows. Native: the host renders it.
    virtualList: (props: UiVirtualListProps) -> UiNode,
    --- A terminal. Native: the host renders it.
    terminalSurface: (props: UiTerminalSurfaceProps) -> UiNode,
    --- A web page. Native: the host renders it.
    browserSurface: (props: UiBrowserSurfaceProps) -> UiNode,
    --- A highlighted file. Native: the host renders it.
    codeView: (props: UiCodeViewProps) -> UiNode,
    --- Changes, split or unified. Native: the host renders it.
    diffView: (props: UiDiffViewProps) -> UiNode,
    --- A folder's file tree. Native: the host renders it.
    fileTree: (props: UiFileTreeProps) -> UiNode,
    --- An interactive UI outline, from a ```` ```ui ```` code block. Native: the host renders it.
    genUi: (props: UiGenUiProps) -> UiNode,
    --- A literal color from `#RRGGBB` or `#RRGGBBAA`. Prefer a semantic token, which follows the theme.
    hex: (color: string) -> HexColor,
}

--- `require("@opencode/plugin")`: defines a plugin's halves.
export type PluginLibrary = {
    --- The contract version the host implements, such as `"1.0"`.
    contract: string,
    --- Defines the window half: `setup` runs once per window. Return the result from `init.luau`.
    defineWindow: (setup: (ctx: WindowContext) -> ()) -> PluginHalf,
    --- Defines the app half: `setup` runs once per process, before any window half. Return the result from `app.luau`.
    defineApp: (setup: (ctx: AppContext) -> ()) -> PluginHalf,
}

-- Globals -----------------------------------------------------------------------------------------

--- Cooperative scheduling on the host's frame clock. Waiting costs no CPU.
declare task: {
    --- Runs a function or thread now, until it first yields.
    spawn: <A..., R...>(callback: ((A...) -> R...) | thread, A...) -> thread,
    --- Runs a function or thread at the end of the current handler or event batch, before the frame commits.
    defer: <A..., R...>(callback: ((A...) -> R...) | thread, A...) -> thread,
    --- Runs a function or thread after `seconds`.
    delay: <A..., R...>(seconds: number, callback: ((A...) -> R...) | thread, A...) -> thread,
    --- Yields for `seconds`, or until the next frame without it. Returns the seconds that passed. Not allowed in a render. Yields: call it from a handler, a task or a signal callback.
    wait: (seconds: number?) -> number,
    --- Cancels a scheduled or waiting thread and its pending request.
    cancel: (thread: thread) -> (),
}

--- Writes a warning to the console.
declare function warn(...: any): ()
