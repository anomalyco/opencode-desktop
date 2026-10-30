//! Key, mouse, and paste encoding plus VT/plain-text serialization for [`VtCore`].

use std::ffi::{c_char, c_int};

use crate::core::VtCore;
use crate::ffi::{self, *};
use crate::types::{KeyAction, KeyInput, MouseBtn, MouseInput, MouseKind};

impl VtCore {
    pub fn encode_key(&mut self, input: &KeyInput) -> Vec<u8> {
        if input.cmd
            && !input.ctrl
            && !input.alt
            && !input.shift
            && input.key == GhosttyKey::Backspace
            && input.action != KeyAction::Release
        {
            return vec![0x15];
        }
        let mut mods: GhosttyMods = 0;
        if input.shift {
            mods |= GHOSTTY_MODS_SHIFT;
        }
        if input.ctrl {
            mods |= GHOSTTY_MODS_CTRL;
        }
        if input.alt {
            mods |= GHOSTTY_MODS_ALT;
        }
        if input.cmd {
            mods |= GHOSTTY_MODS_SUPER;
        }
        let action: c_int = match input.action {
            KeyAction::Release => GHOSTTY_KEY_ACTION_RELEASE,
            KeyAction::Press => GHOSTTY_KEY_ACTION_PRESS,
            KeyAction::Repeat => GHOSTTY_KEY_ACTION_REPEAT,
        };
        let utf8_bytes = input.text.as_deref().unwrap_or("").as_bytes();
        let unshifted = input.unshifted_codepoint.unwrap_or_else(|| {
            input
                .text
                .as_deref()
                .and_then(|s| s.chars().next())
                .map_or(0, |c| c.to_ascii_lowercase() as u32)
        });
        unsafe {
            ffi::ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.term);
            ffi::ghostty_key_event_set_action(self.key_event, action);
            ffi::ghostty_key_event_set_key(self.key_event, input.key);
            ffi::ghostty_key_event_set_mods(self.key_event, mods);
            ffi::ghostty_key_event_set_consumed_mods(self.key_event, 0);
            ffi::ghostty_key_event_set_composing(self.key_event, false);
            if utf8_bytes.is_empty() {
                ffi::ghostty_key_event_set_utf8(self.key_event, std::ptr::null(), 0);
            } else {
                ffi::ghostty_key_event_set_utf8(
                    self.key_event,
                    utf8_bytes.as_ptr().cast::<c_char>(),
                    utf8_bytes.len(),
                );
            }
            ffi::ghostty_key_event_set_unshifted_codepoint(self.key_event, unshifted);
            let mut buf = [0u8; 128];
            let mut written = 0usize;
            let rc = ffi::ghostty_key_encoder_encode(
                self.key_encoder,
                self.key_event,
                buf.as_mut_ptr().cast::<c_char>(),
                buf.len(),
                &mut written,
            );
            if rc == GHOSTTY_SUCCESS && written > 0 {
                buf[..written].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    pub fn encode_mouse(&mut self, input: MouseInput) -> Vec<u8> {
        let mut mods: GhosttyMods = 0;
        if input.shift {
            mods |= GHOSTTY_MODS_SHIFT;
        }
        if input.ctrl {
            mods |= GHOSTTY_MODS_CTRL;
        }
        if input.alt {
            mods |= GHOSTTY_MODS_ALT;
        }
        if input.cmd {
            mods |= GHOSTTY_MODS_SUPER;
        }
        let action = match input.kind {
            MouseKind::Press => GHOSTTY_MOUSE_ACTION_PRESS,
            MouseKind::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
            MouseKind::Move => GHOSTTY_MOUSE_ACTION_MOTION,
        };
        let btn = match input.button {
            MouseBtn::None => None,
            MouseBtn::Left => Some(GHOSTTY_MOUSE_BUTTON_LEFT),
            MouseBtn::Right => Some(GHOSTTY_MOUSE_BUTTON_RIGHT),
            MouseBtn::Middle => Some(GHOSTTY_MOUSE_BUTTON_MIDDLE),
            MouseBtn::WheelUp => Some(GHOSTTY_MOUSE_BUTTON_FOUR),
            MouseBtn::WheelDown => Some(GHOSTTY_MOUSE_BUTTON_FIVE),
            MouseBtn::WheelLeft => Some(GHOSTTY_MOUSE_BUTTON_SIX),
            MouseBtn::WheelRight => Some(GHOSTTY_MOUSE_BUTTON_SEVEN),
        };
        let size = GhosttyMouseEncoderSize {
            size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
            screen_width: u32::from(self.cb.size.columns) * self.cb.size.cell_width.max(1),
            screen_height: u32::from(self.cb.size.rows) * self.cb.size.cell_height.max(1),
            cell_width: self.cb.size.cell_width.max(1),
            cell_height: self.cb.size.cell_height.max(1),
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        unsafe {
            ffi::ghostty_mouse_encoder_setopt_from_terminal(self.mouse_encoder, self.term);
            ffi::ghostty_mouse_encoder_setopt(
                self.mouse_encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&size as *const GhosttyMouseEncoderSize).cast(),
            );
            ffi::ghostty_mouse_event_set_action(self.mouse_event, action);
            match btn {
                Some(b) => ffi::ghostty_mouse_event_set_button(self.mouse_event, b),
                None => ffi::ghostty_mouse_event_clear_button(self.mouse_event),
            }
            ffi::ghostty_mouse_event_set_mods(self.mouse_event, mods);
            ffi::ghostty_mouse_event_set_position(
                self.mouse_event,
                GhosttyMousePosition {
                    x: input.x_px.max(0.0),
                    y: input.y_px.max(0.0),
                },
            );
            let mut buf = [0u8; 64];
            let mut written = 0usize;
            let rc = ffi::ghostty_mouse_encoder_encode(
                self.mouse_encoder,
                self.mouse_event,
                buf.as_mut_ptr().cast::<c_char>(),
                buf.len(),
                &mut written,
            );
            if rc == GHOSTTY_SUCCESS && written > 0 {
                buf[..written].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        if text.is_empty() {
            return Vec::new();
        }
        let bracketed = self.mode_enabled(GHOSTTY_MODE_BRACKETED_PASTE);
        let mut mutable_in = text.as_bytes().to_vec();
        let mut out = vec![0u8; mutable_in.len() + 32];
        let mut written = 0usize;
        let rc = unsafe {
            ffi::ghostty_paste_encode(
                mutable_in.as_mut_ptr().cast::<c_char>(),
                mutable_in.len(),
                bracketed,
                out.as_mut_ptr().cast::<c_char>(),
                out.len(),
                &mut written,
            )
        };
        if rc == GHOSTTY_OUT_OF_SPACE && written > out.len() {
            let mut retry_in = text.as_bytes().to_vec();
            out.resize(written + 16, 0);
            if unsafe {
                ffi::ghostty_paste_encode(
                    retry_in.as_mut_ptr().cast::<c_char>(),
                    retry_in.len(),
                    bracketed,
                    out.as_mut_ptr().cast::<c_char>(),
                    out.len(),
                    &mut written,
                )
            } == GHOSTTY_SUCCESS
            {
                out.truncate(written);
                return out;
            }
        }
        if rc == GHOSTTY_SUCCESS {
            out.truncate(written);
            out
        } else {
            text.as_bytes().to_vec()
        }
    }

    pub fn is_paste_safe(text: &str) -> bool {
        unsafe { ffi::ghostty_paste_is_safe(text.as_ptr().cast::<c_char>(), text.len()) }
    }

    pub fn serialize_vt(&self) -> Vec<u8> {
        self.format_with(GhosttyFormatterTerminalOptions::vt_full())
    }

    pub fn format_plain(&self) -> String {
        String::from_utf8_lossy(&self.format_with(GhosttyFormatterTerminalOptions::plain_trimmed()))
            .into_owned()
    }

    fn format_with(&self, opts: GhosttyFormatterTerminalOptions) -> Vec<u8> {
        let mut fmt: GhosttyFormatter = std::ptr::null_mut();
        unsafe {
            if ffi::ghostty_formatter_terminal_new(std::ptr::null(), &mut fmt, self.term, opts)
                != GHOSTTY_SUCCESS
                || fmt.is_null()
            {
                return Vec::new();
            }
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0usize;
            let rc = ffi::ghostty_formatter_format_alloc(fmt, std::ptr::null(), &mut ptr, &mut len);
            ffi::ghostty_formatter_free(fmt);
            if rc != GHOSTTY_SUCCESS || ptr.is_null() {
                return Vec::new();
            }
            let out = std::slice::from_raw_parts(ptr, len).to_vec();
            ffi::ghostty_free(std::ptr::null(), ptr, len);
            out
        }
    }
}
