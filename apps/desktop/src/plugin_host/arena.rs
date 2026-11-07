//! `PluginRenderArena`: the slab a plugin's render fills with real `rune::El`s, one
//! `ElSpec` per element. Children link through the slots, so building a tree allocates nothing
//! once the slab has grown; the tree is assembled when the render (or a memo) hands back its
//! root, and the slab is truncated to where the render began.

use std::rc::Rc;

use opencode_plugin::abi::{ElKind, ElSpec, flags};
use opencode_plugin::{EditorHandle, ElHandle};
use rune::{Color, El, TextStyle};

use super::editors::PluginEditors;
use super::icons::intern;
use super::instance::{CallbackEnv, Press};
use crate::App;

const NONE: u32 = u32::MAX;

struct Node {
    el: Option<El>,
    press: Option<Rc<Press>>,
    first: u32,
    last: u32,
    next: u32,
}

#[derive(Default)]
pub struct PluginRenderArena {
    nodes: Vec<Node>,
}

impl PluginRenderArena {
    pub fn mark(&self) -> usize {
        self.nodes.len()
    }

    pub fn rollback(&mut self, mark: usize) {
        self.nodes.truncate(mark);
    }

    pub fn push(&mut self, el: El, press: Option<Rc<Press>>) -> ElHandle {
        self.nodes.push(Node { el: Some(el), press, first: NONE, last: NONE, next: NONE });
        ElHandle(self.nodes.len() as u32 - 1)
    }

    /// Restyles `h`, keeping its children, and its press handler unless `press` replaces it.
    pub fn replace(&mut self, h: ElHandle, el: El, press: Option<Rc<Press>>) {
        if let Some(n) = self.nodes.get_mut(h.0 as usize) {
            n.el = Some(el);
            if press.is_some() {
                n.press = press;
            }
        }
    }

    pub fn link(&mut self, parent: ElHandle, child: ElHandle) {
        let (p, c) = (parent.0, child.0);
        if p == c || p as usize >= self.nodes.len() || c as usize >= self.nodes.len() {
            return;
        }
        match self.nodes[p as usize].last {
            NONE => self.nodes[p as usize].first = c,
            last => self.nodes[last as usize].next = c,
        }
        self.nodes[p as usize].last = c;
    }

    /// The finished tree under `h`; an empty element for a bad handle.
    pub fn take(&mut self, h: ElHandle) -> El {
        let Some(node) = self.nodes.get_mut(h.0 as usize) else { return rune::el() };
        let mut el = node.el.take().unwrap_or_else(rune::el);
        if let Some(press) = node.press.take() {
            el = el.on_press_tx(move |app: &mut App, tx: &mut rune::Tx| super::run::press(app, tx, &press));
        }
        let mut child = std::mem::replace(&mut node.first, NONE);
        while child != NONE {
            let next = self.nodes[child as usize].next;
            el = el.child(self.take(ElHandle(child)));
            child = next;
        }
        el
    }
}

/// What turning an `ElSpec` into an `El` needs from the render.
pub struct Build<'a> {
    pub editors: &'a PluginEditors,
    pub faint: Color,
    /// The plugin's id: its icons are looked up under it.
    pub plugin: &'a str,
    pub slot: usize,
    pub generation: u32,
    pub lib: &'a std::sync::Arc<super::loader::LoadedLibrary>,
}

fn color(c: [f32; 4]) -> Color {
    Color { r: c[0], g: c[1], b: c[2], a: c[3] }
}

/// # Safety
/// `spec`'s strings must be valid for the call.
pub unsafe fn build(b: &Build, spec: &ElSpec) -> El {
    fn s(v: opencode_plugin::abi::AbiStr<'_>) -> &str {
        // SAFETY: `build`'s caller guarantees the spec's strings are valid for the call.
        unsafe { v.as_str() }
    }
    let style = || {
        let size = if spec.font_size > 0.0 { spec.font_size } else { 13.0 };
        let weight = if spec.font_weight > 0.0 { spec.font_weight } else { 440.0 };
        let style = TextStyle::new(size, weight);
        let style = if spec.line_height > 0.0 { style.lh(spec.line_height) } else { style };
        if spec.flags & flags::MONO != 0 { style.mono() } else { style }
    };
    let mut el = match spec.kind {
        ElKind::Row => rune::row(),
        ElKind::Col => rune::col(),
        ElKind::Text => rune::text(s(spec.text).to_owned()).font(style()),
        ElKind::Spacer => rune::spacer(),
        ElKind::Icon => {
            let icon = super::icons::icon(spec.icon_id).unwrap_or_else(|| super::icons::resolve(b.plugin, s(spec.text)));
            rune::icon(icon, if spec.font_size > 0.0 { spec.font_size } else { 16.0 })
        }
        ElKind::TextInput => text_input(b, spec.editor, s(spec.text), style()),
        ElKind::Box | ElKind::NativeSurface => rune::el(),
    };
    let set = |v: f32| !v.is_nan();
    el = match spec.width {
        w if w < 0.0 => el.w_full(),
        w if set(w) => el.w(w),
        _ => el,
    };
    el = match spec.height {
        h if h < 0.0 => el.h_full(),
        h if set(h) => el.h(h),
        _ => el,
    };
    if set(spec.min_w) { el = el.min_w(spec.min_w); }
    if set(spec.min_h) { el = el.min_h(spec.min_h); }
    if set(spec.max_w) { el = el.max_w(spec.max_w); }
    if set(spec.max_h) { el = el.max_h(spec.max_h); }
    if spec.grow > 0.0 { el = el.grow(spec.grow); }
    if spec.flags & flags::SHRINK != 0 { el = el.shrink(); }
    let [pt, pr, pb, pl] = spec.pad;
    if pt != 0.0 { el = el.pt(pt); }
    if pr != 0.0 { el = el.pr(pr); }
    if pb != 0.0 { el = el.pb(pb); }
    if pl != 0.0 { el = el.pl(pl); }
    let [mt, mr, mb, ml] = spec.margin;
    if mt != 0.0 { el = el.mt(mt); }
    if mr != 0.0 { el = el.mr(mr); }
    if mb != 0.0 { el = el.mb(mb); }
    if ml != 0.0 { el = el.ml(ml); }
    if spec.gap != 0.0 { el = el.gap(spec.gap); }
    let r = spec.radius;
    if r != [0.0; 4] { el = if r.iter().all(|&v| v == r[0]) { el.radius(r[0]) } else { el.corners(r) }; }
    if spec.border_w > 0.0 { el = el.border(spec.border_w, color(spec.border_color)); }
    if spec.fill[3] > 0.0 { el = el.fill(color(spec.fill)); }
    if spec.hover_fill[3] > 0.0 { el = el.hover(color(spec.hover_fill)); }
    if spec.press_fill[3] > 0.0 { el = el.press(color(spec.press_fill)); }
    if spec.text_color[3] > 0.0 { el = el.color(color(spec.text_color)); }
    if spec.opacity < 1.0 { el = el.opacity(spec.opacity.max(0.0)); }
    let f = spec.flags;
    if f & flags::WRAP != 0 { el = el.wrap(); }
    if f & flags::TRUNCATE != 0 { el = el.truncate(); }
    if f & flags::TRUNCATE_FADE != 0 { el = el.truncate_fade(); }
    if f & flags::FOCUSABLE != 0 { el = el.focusable(true); }
    if f & flags::CLIP != 0 { el = el.clip_rounded(r[0]); }
    if f & flags::CENTER != 0 { el = el.center(); }
    if f & flags::CENTER_Y != 0 { el = el.center_y(); }
    if f & flags::ITEMS_CENTER != 0 { el = el.items_center(); }
    if f & flags::ITEMS_END != 0 { el = el.items_end(); }
    if f & flags::JUSTIFY_BETWEEN != 0 { el = el.justify_between(); }
    if f & flags::SELECTABLE != 0 { el = el.selectable(true); }
    if !spec.key.is_empty() { el = el.key(s(spec.key).to_owned()); }
    if !spec.role.is_empty() { el = el.role(intern(s(spec.role))); }
    if !spec.label.is_empty() { el = el.label(s(spec.label).to_owned()); }
    if !spec.help.is_empty() { el = el.help(s(spec.help).to_owned()); }
    el
}

/// The press handler `spec` hands over, if any; the host owns its captures from here on.
pub fn press(b: &Build, spec: &ElSpec) -> Option<Rc<Press>> {
    spec.press.func.is_some().then(|| Rc::new(Press { slot: b.slot, generation: b.generation, env: CallbackEnv::new(spec.press, b.lib.clone()) }))
}

fn text_input(b: &Build, editor: EditorHandle, placeholder: &str, style: TextStyle) -> El {
    let el = match b.editors.get(editor) {
        Some(edit) => rune::text_input(edit, style),
        None => rune::text_input(&rune::TextEdit::new(""), style),
    };
    let el = el.on_edit(move |app: &mut App| app.plugins.editors.get_mut(editor));
    if placeholder.is_empty() { el } else { el.placeholder(placeholder.to_owned(), b.faint) }
}
