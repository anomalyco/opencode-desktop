//! Element builders over the host's render arena.
//!
//! `row().w_full().h(36.0).px(12.0).fill(c)` fills a stack-local [`ElSpec`]; the element crosses
//! the boundary in one `el_build` call when a parent takes it (or when it takes a child), and
//! becomes a real `rune::El` in the host's arena. Strings ride along as borrowed views for that
//! call; nothing is serialised. Builders only work inside a render callback.

use std::borrow::Cow;

use crate::abi::{AbiStr, Callback, EditorHandle, ElHandle, ElKind, ElSpec, Rgba, flags};
use crate::boundary::{ctx, drop_packed, host};

/// An element being built. Pass it to a parent's [`El::child`] or return it from a render.
#[must_use]
pub struct El {
    spec: ElSpec<'static>,
    key: Cow<'static, str>,
    role: Cow<'static, str>,
    label: Cow<'static, str>,
    help: Cow<'static, str>,
    text: Cow<'static, str>,
    press: Option<Handler>,
    hover: Option<Handler>,
    anchor: Cow<'static, str>,
    /// A button's leading icon.
    lead: Cow<'static, str>,
    handle: Option<ElHandle>,
    dirty: bool,
    /// Built by the host (a memo's subtree): setters no longer reach it.
    sealed: bool,
}

/// A press handler made by `ViewCx::handler`, owned until an element hands it to the host.
pub struct Handler(pub(crate) Callback);

impl Drop for Handler {
    fn drop(&mut self) {
        drop_packed(self.0.data, self.0.drop_data);
    }
}

fn new(kind: ElKind) -> El {
    El {
        spec: ElSpec { kind, ..ElSpec::EMPTY },
        key: Cow::Borrowed(""),
        role: Cow::Borrowed(""),
        label: Cow::Borrowed(""),
        help: Cow::Borrowed(""),
        text: Cow::Borrowed(""),
        press: None,
        hover: None,
        anchor: Cow::Borrowed(""),
        lead: Cow::Borrowed(""),
        handle: None,
        dirty: false,
        sealed: false,
    }
}

/// A number whose changed digits roll as it changes (the app's odometer): `text` is what it
/// reads, `value` the number it stands for, `key` its identity across renders.
pub fn odometer(key: impl Into<Cow<'static, str>>, label: impl Into<Cow<'static, str>>, value: f64) -> El {
    let mut e = new(ElKind::Odometer);
    e.key = key.into();
    e.text = label.into();
    e.spec.value = value;
    e
}

/// A horizontal stack.
pub fn row() -> El {
    new(ElKind::Row)
}

/// A vertical stack.
pub fn col() -> El {
    new(ElKind::Col)
}

/// A box with no direction of its own.
pub fn el() -> El {
    new(ElKind::Box)
}

/// Flexible space in a stack.
pub fn spacer() -> El {
    new(ElKind::Spacer)
}

pub fn text(s: impl Into<Cow<'static, str>>) -> El {
    let mut e = new(ElKind::Text);
    e.text = s.into();
    e
}

/// An icon by name: one this plugin interned (`cx.icon`), or a host icon (`"code"`, `"globe"`,
/// `"console"`, `"file-tree"`, `"folder"`).
pub fn icon(name: &'static str, size: f32) -> El {
    let mut e = new(ElKind::Icon);
    e.text = Cow::Borrowed(name);
    e.spec.font_size = size;
    e
}

/// An icon this plugin interned, by id.
pub fn icon_id(id: crate::shared::IconId, size: f32) -> El {
    let mut e = new(ElKind::Icon);
    e.spec.icon_id = id.0;
    e.spec.font_size = size;
    e
}

/// A text field over a host-owned editor (`WindowSetupCx::create_editor`): typing, IME,
/// selection and undo all happen in the host.
pub fn text_input(editor: EditorHandle) -> El {
    let mut e = new(ElKind::TextInput);
    e.spec.editor = editor;
    e
}

macro_rules! setters {
    ($($(#[$m:meta])* $name:ident($($arg:ident: $ty:ty),*) => |$s:ident| $body:expr;)*) => {
        $(
            $(#[$m])*
            pub fn $name(mut self, $($arg: $ty),*) -> Self {
                {
                    let $s = &mut self.spec;
                    $body;
                }
                self.dirty = true;
                self
            }
        )*
    };
}

impl El {
    setters! {
        w(v: f32) => |s| s.width = v;
        h(v: f32) => |s| s.height = v;
        size(w: f32, h: f32) => |s| (s.width, s.height) = (w, h);
        w_full() => |s| s.width = -1.0;
        h_full() => |s| s.height = -1.0;
        min_w(v: f32) => |s| s.min_w = v;
        min_h(v: f32) => |s| s.min_h = v;
        max_w(v: f32) => |s| s.max_w = v;
        max_h(v: f32) => |s| s.max_h = v;
        grow(v: f32) => |s| s.grow = v;
        shrink() => |s| s.flags |= flags::SHRINK;
        padding(v: f32) => |s| s.pad = [v; 4];
        px(v: f32) => |s| (s.pad[1], s.pad[3]) = (v, v);
        py(v: f32) => |s| (s.pad[0], s.pad[2]) = (v, v);
        pt(v: f32) => |s| s.pad[0] = v;
        pr(v: f32) => |s| s.pad[1] = v;
        pb(v: f32) => |s| s.pad[2] = v;
        pl(v: f32) => |s| s.pad[3] = v;
        mt(v: f32) => |s| s.margin[0] = v;
        mr(v: f32) => |s| s.margin[1] = v;
        mb(v: f32) => |s| s.margin[2] = v;
        ml(v: f32) => |s| s.margin[3] = v;
        mx(v: f32) => |s| (s.margin[1], s.margin[3]) = (v, v);
        my(v: f32) => |s| (s.margin[0], s.margin[2]) = (v, v);
        gap(v: f32) => |s| s.gap = v;
        radius(v: f32) => |s| s.radius = [v; 4];
        /// Top-left, top-right, bottom-right, bottom-left.
        corners(r: [f32; 4]) => |s| s.radius = r;
        border(width: f32, color: Rgba) => |s| (s.border_w, s.border_color) = (width, color);
        fill(c: Rgba) => |s| s.fill = c;
        hover(c: Rgba) => |s| s.hover_fill = c;
        press(c: Rgba) => |s| s.press_fill = c;
        color(c: Rgba) => |s| s.text_color = c;
        opacity(v: f32) => |s| s.opacity = v;
        font(size: f32, weight: f32) => |s| (s.font_size, s.font_weight) = (size, weight);
        line_height(v: f32) => |s| s.line_height = v;
        mono() => |s| s.flags |= flags::MONO;
        wrap() => |s| s.flags |= flags::WRAP;
        truncate() => |s| s.flags |= flags::TRUNCATE;
        truncate_fade() => |s| s.flags |= flags::TRUNCATE_FADE;
        focusable() => |s| s.flags |= flags::FOCUSABLE;
        clip() => |s| s.flags |= flags::CLIP;
        center() => |s| s.flags |= flags::CENTER;
        center_y() => |s| s.flags |= flags::CENTER_Y;
        items_center() => |s| s.flags |= flags::ITEMS_CENTER;
        items_end() => |s| s.flags |= flags::ITEMS_END;
        justify_between() => |s| s.flags |= flags::JUSTIFY_BETWEEN;
        selectable() => |s| s.flags |= flags::SELECTABLE;
        /// Scrolls its overflow vertically.
        scroll_y() => |s| s.flags |= flags::SCROLL;
        /// Tabular figures.
        tabular() => |s| s.flags |= flags::TABULAR;
        /// Children's [`El::group_color`] applies while this is hovered.
        hover_group() => |s| s.flags |= flags::HOVER_GROUP;
        /// Hover and press fills land at once.
        instant_hover() => |s| s.flags |= flags::INSTANT_HOVER;
        help_above() => |s| s.flags |= flags::HELP_ABOVE;
        no_wrap() => |s| s.flags |= flags::NO_WRAP;
        /// Letter spacing in points.
        tracking(v: f32) => |s| s.tracking = v;
        /// One of Desktop's elevations (`abi::SHADOW_RAISED`, `SHADOW_FLOATING`, `SHADOW_OVERLAY`).
        shadow(v: u32) => |s| s.shadow = v;
        /// The text or icon colour while the nearest [`El::hover_group`] ancestor is hovered.
        group_color(c: Rgba) => |s| s.group_color = c;
        /// Out of flow at these offsets from its parent's edges (any of left, top, right, bottom).
        pin(left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) => |s| {
            s.flags |= flags::ABSOLUTE;
            s.pin = [left.unwrap_or(f32::NAN), top.unwrap_or(f32::NAN), right.unwrap_or(f32::NAN), bottom.unwrap_or(f32::NAN)];
        };
        /// A button, radio or field that takes no presses or typing.
        disabled(on: bool) => |s| if on { s.flags |= flags::DISABLED } else { s.flags &= !flags::DISABLED };
        /// A button showing Desktop's loader.
        loading(on: bool) => |s| if on { s.flags |= flags::LOADING } else { s.flags &= !flags::LOADING };
        /// A button at `size="small"`.
        small() => |s| s.flags |= flags::SMALL;
        /// A field at `appearance="large"`.
        large() => |s| s.flags |= flags::LARGE;
        /// A field drawn as invalid.
        invalid(on: bool) => |s| if on { s.flags |= flags::INVALID } else { s.flags &= !flags::INVALID };
        /// A field whose text is hidden.
        secure() => |s| s.flags |= flags::SECURE;
        /// Shown only while the nearest [El::hover_group] ancestor is hovered.
        reveal_on_hover() => |s| s.flags |= flags::REVEAL;
    }

    /// An overlay beside the element keyed `key` (drawn before it): on `side`
    /// (`abi::SIDE_BOTTOM` …), aligned along it (`abi::ALIGN_START` …), `gap` points away.
    pub fn anchored(mut self, key: impl Into<Cow<'static, str>>, side: u32, align: u32, gap: f32) -> Self {
        self.anchor = key.into();
        self.spec.anchor_side = side;
        self.spec.anchor_align = align;
        self.spec.anchor_gap = gap;
        self.dirty = true;
        self
    }

    /// A [`ui::button_with`]'s leading icon, by name (Desktop's `Button icon`).
    pub fn lead_icon(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.lead = name.into();
        self.dirty = true;
        self
    }

    /// Runs as the pointer enters (`cx.event().x == 1.0`) and leaves (`0.0`).
    pub fn on_hover(mut self, handler: Handler) -> Self {
        self.hover = Some(handler);
        self.dirty = true;
        self
    }

    /// A key unique among its siblings, for identity across renders (and automation ids).
    pub fn key(mut self, key: impl Into<Cow<'static, str>>) -> Self {
        self.key = key.into();
        self.dirty = true;
        self
    }

    /// The accessibility role (`"button"`, `"switch"`, `"textbox"` …).
    pub fn role(mut self, role: impl Into<Cow<'static, str>>) -> Self {
        self.role = role.into();
        self.dirty = true;
        self
    }

    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = label.into();
        self.dirty = true;
        self
    }

    /// A tooltip.
    pub fn help(mut self, help: impl Into<Cow<'static, str>>) -> Self {
        self.help = help.into();
        self.dirty = true;
        self
    }

    /// The placeholder of a [`text_input`].
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.text = placeholder.into();
        self.dirty = true;
        self
    }

    pub fn on_press(mut self, handler: Handler) -> Self {
        self.press = Some(handler);
        self.dirty = true;
        self
    }

    pub fn child(mut self, child: El) -> Self {
        let parent = self.flush();
        let child = child.finish();
        // SAFETY: both handles are live slots of the arena of the render running on this thread.
        unsafe { (host().el_child)(ctx(), parent, child) };
        self
    }

    pub fn children(self, children: impl IntoIterator<Item = El>) -> Self {
        children.into_iter().fold(self, El::child)
    }

    /// An element the host already built (a memo's subtree).
    pub(crate) fn from_handle(handle: ElHandle) -> El {
        El { handle: Some(handle), sealed: true, ..new(ElKind::Box) }
    }

    /// Hands the element to the host, returning its arena slot.
    pub fn finish(mut self) -> ElHandle {
        self.flush()
    }

    fn flush(&mut self) -> ElHandle {
        if let Some(h) = self.handle
            && (!self.dirty || self.sealed)
        {
            return h;
        }
        let mut spec = self.spec;
        spec.key = AbiStr::new(&self.key);
        spec.role = AbiStr::new(&self.role);
        spec.label = AbiStr::new(&self.label);
        spec.help = AbiStr::new(&self.help);
        spec.text = AbiStr::new(&self.text);
        spec.anchor = AbiStr::new(&self.anchor);
        spec.icon = AbiStr::new(&self.lead);
        // The host owns the handlers' captures from here on.
        let give = |h: Option<Handler>| {
            h.map_or(Callback::NONE, |h| {
                let cb = h.0;
                core::mem::forget(h);
                cb
            })
        };
        spec.press = give(self.press.take());
        spec.hover = give(self.hover.take());
        let host = host();
        let handle = match self.handle {
            // SAFETY: `spec`'s strings borrow `self`, alive for the call; `h` is a live slot.
            Some(h) => unsafe {
                (host.el_apply)(ctx(), h, &spec);
                h
            },
            // SAFETY: as above.
            None => unsafe { (host.el_build)(ctx(), &spec) },
        };
        self.handle = Some(handle);
        self.dirty = false;
        handle
    }
}

/// Small composed controls drawn with the host palette.
pub mod ui {
    use super::*;
    use crate::cx::palette;

    /// A ghost button with a label (Desktop's `Button variant="ghost"`).
    pub fn button(label: impl Into<Cow<'static, str>>) -> El {
        button_with(label, crate::abi::BUTTON_GHOST)
    }

    /// Desktop's `Button` in variant `variant` (`abi::BUTTON_NEUTRAL` …), the host's own: give it
    /// `on_press`, and `disabled`, `loading` or `small` as it needs.
    pub fn button_with(label: impl Into<Cow<'static, str>>, variant: f64) -> El {
        let mut e = new(ElKind::Button);
        e.text = label.into();
        e.spec.value = variant;
        e
    }

    /// Desktop's `Loader`, `size` points across.
    pub fn loader(size: f32) -> El {
        let mut e = new(ElKind::Loader);
        e.spec.font_size = size;
        e
    }

    /// A `radio-v2` item: its control, `label` and (when not empty) `description`, coloured
    /// `tone` when given. The press goes on its row.
    pub fn radio(label: impl Into<Cow<'static, str>>, description: impl Into<Cow<'static, str>>, checked: bool, tone: Option<crate::Rgba>) -> El {
        let mut e = new(ElKind::Radio);
        e.text = label.into();
        e.help = description.into();
        e.spec.value = if checked { 1.0 } else { 0.0 };
        if let Some(c) = tone {
            e.spec.text_color = c;
        }
        e
    }

    /// Desktop's `TextInput` over a host-owned editor; `placeholder` shows while it's empty.
    /// Its `on_press` runs on Enter.
    pub fn field(editor: EditorHandle, placeholder: impl Into<Cow<'static, str>>) -> El {
        let mut e = new(ElKind::Field);
        e.spec.editor = editor;
        e.text = placeholder.into();
        e
    }

    /// Desktop's `SettingsRow`, keyed `id` for settings search: `title` and `description`
    /// beside `control`.
    pub fn settings_row(id: impl Into<Cow<'static, str>>, title: impl Into<Cow<'static, str>>, description: impl Into<Cow<'static, str>>, control: El) -> El {
        let mut e = new(ElKind::SettingsRow);
        e.key = id.into();
        e.text = title.into();
        e.help = description.into();
        e.child(control)
    }

    /// Desktop's settings list card around `rows` (`settings_row`s), split by hairlines.
    pub fn settings_list(key: impl Into<Cow<'static, str>>, rows: impl IntoIterator<Item = El>) -> El {
        let mut e = new(ElKind::SettingsList);
        e.key = key.into();
        e.children(rows)
    }

    /// Desktop's `Switch`, the app's own: give it `on_press`, and `disabled` while it can't change.
    pub fn switch_control(on: bool) -> El {
        let mut e = new(ElKind::Switch);
        e.spec.value = if on { 1.0 } else { 0.0 };
        e
    }

    /// Markdown drawn as the timeline draws a reply (`Markdown class="text-14-regular"`), links,
    /// code blocks and all.
    pub fn markdown(source: impl Into<Cow<'static, str>>) -> El {
        let mut e = new(ElKind::Markdown);
        e.text = source.into();
        e
    }

    /// Desktop's `TextShimmer`: `label` shimmering, in the font and colour you give it.
    pub fn shimmer(label: impl Into<Cow<'static, str>>) -> El {
        let mut e = new(ElKind::Shimmer);
        e.text = label.into();
        e
    }

    /// An image handed to the host with [`crate::image`], `w` × `h` points, aspect kept.
    pub fn image(id: crate::ImageId, w: f32, h: f32) -> El {
        let mut e = new(ElKind::Image);
        e.spec.icon_id = id.0;
        e.spec.width = w;
        e.spec.height = h;
        e
    }

    /// A status capsule (a titlebar item or a composer pill): an optional icon and a label,
    /// 24 high, fully rounded, on the raised layer.
    pub fn pill(icon_name: &'static str, label: impl Into<Cow<'static, str>>) -> El {
        let p = palette();
        let label = label.into();
        let mut r = row()
            .role("button")
            .label(label.clone())
            .h(24.0)
            .px(9.0)
            .gap(5.0)
            .center_y()
            .radius(12.0)
            .fill(p.layer1)
            .border(0.5, p.border_muted)
            .hover(p.hover)
            .press(p.pressed);
        if !icon_name.is_empty() {
            r = r.child(icon(icon_name, 14.0).color(p.icon_muted));
        }
        r.child(text(label).font(12.0, 500.0).line_height(16.0).color(p.text))
    }

    /// Desktop's switch: 24 × 16, the thumb 12 × 12, accent when on.
    pub fn switch(on: bool) -> El {
        let p = palette();
        let thumb = el().size(12.0, 12.0).radius(2.0).fill(p.switch_thumb);
        let track = row().role("switch").label(if on { "On" } else { "Off" }).w(24.0).h(16.0).px(2.0).center_y().radius(4.0);
        let track = track.fill(if on { p.bg_accent } else { p.layer3 });
        if on { track.child(spacer()).child(thumb) } else { track.child(thumb) }
    }
}
