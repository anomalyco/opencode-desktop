//! [`ABI_LAYOUT_HASH`]: FNV-1a over the size, alignment and every field offset of each
//! `#[repr(C)]` type in [`crate::abi`], computed at compile time. Host and plugin compare it in
//! the handshake, so an edit to a struct that forgets to bump `ABI_EPOCH` fails the load
//! cleanly instead of misreading memory.

use core::mem::{align_of, offset_of, size_of};

use crate::abi::*;

const fn mix(mut h: u64, v: u64) -> u64 {
    let bytes = v.to_le_bytes();
    let mut i = 0;
    while i < 8 {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
        i += 1;
    }
    h
}

macro_rules! layout {
    ($h:ident, $t:ty $(, $field:ident)* $(,)?) => {{
        $h = mix($h, size_of::<$t>() as u64);
        $h = mix($h, align_of::<$t>() as u64);
        $($h = mix($h, offset_of!($t, $field) as u64);)*
    }};
}

pub const ABI_LAYOUT_HASH: u64 = {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    h = mix(h, ABI_EPOCH as u64);
    layout!(h, AbiStr<'static>, ptr, len);
    layout!(h, AbiOwnedBytes, ptr, len, cap, drop_fn);
    layout!(h, AbiOutcome<u64>, status, value, message);
    layout!(h, AbiOutcome<u8>, status, value, message);
    layout!(h, AbiOutcome<bool>, status, value, message);
    layout!(h, AbiOutcome<ElHandle>, status, value, message);
    layout!(h, ElHandle);
    layout!(h, EditorHandle);
    layout!(h, SurfaceHandle);
    layout!(h, ElKind);
    layout!(h, AbiStatus);
    layout!(h, Callback, func, data, drop_data);
    layout!(h, RenderCallback, func, data, drop_data);
    layout!(h, AbiEvent, kind, mods, x, y);
    layout!(h, RenderProps<'static>, surface, id, session_id, directory, width, height, visible, focused, dark);
    layout!(
        h, AbiPalette, dark, theme, bg_base, layer1, layer2, layer3, text, muted, faint, accent, bg_accent, on_fill, icon, icon_muted,
        border, border_muted, hover, pressed, success, warning, danger, switch_thumb
    );
    layout!(
        h, ElSpec<'static>, kind, key, role, label, help, text, icon_id, surface, editor, width, height, min_w, min_h, max_w, max_h, grow,
        shrink, pad, margin, gap, radius, border_w, fill, hover_fill, press_fill, border_color, text_color, font_size, font_weight,
        line_height, opacity, flags, press
    );
    layout!(h, CommandSpecAbi<'static>, id, title, description, keywords, slash, bind_windows, bind_macos, bind_linux, section, flags, run);
    layout!(h, PanelSpecAbi<'static>, id, title, icon, order, flags, render);
    layout!(h, SettingSpecAbi<'static>, id, title, description, keywords, render);
    layout!(h, HostAllocVtable, alloc, alloc_zeroed, dealloc, realloc);
    layout!(h, WakerAbi, data, wake, clone, drop);
    layout!(
        h, HostApiVtable, log, add_command, add_panel, add_setting, create_editor, waker, editor_text, editor_version, editor_set_text,
        instant, wake_after, copy_text, open_panel, toast, palette, el_build, el_apply, el_child, memo
    );
    layout!(h, PluginVtable, window_setup, drop_state, poll, save_hot_state, load_hot_state);
    layout!(h, HostHandshake, magic, endian, abi_epoch, abi_minor, abi_layout_hash, pointer_width, os, arch, host_vtable_size, host_vtable, host_alloc);
    layout!(h, PluginDescriptor, magic, endian, abi_epoch, abi_minor, abi_layout_hash, sdk_version, plugin_vtable_size, plugin_vtable);
    h
};
