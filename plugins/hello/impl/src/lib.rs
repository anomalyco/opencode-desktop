//! The smallest useful plugin: a side panel with a counter and a text field, a palette command
//! with a keybind, a settings row, an app menu item, and the Greeter service (`hello-api`) other
//! plugins call in-process. It exists to prove the plugin boundary, so it also carries two
//! deliberate panics (one in a handler, one in render).
//!
//! A built-in: this rlib declares it, `plugins/hello` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.
#![allow(unsafe_code)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};

use opencode_plugin::abi::{AbiOwnedBytes, AbiStr};
use opencode_plugin::prelude::*;
use opencode_plugin_hello_api::{GREETER, Greeter};

const GREETING: &str = "Hello";

/// The window's counter, as the Greeter service reads it.
static CLICKS: AtomicI64 = AtomicI64::new(0);

/// Remembered across launches by the host.
const SHOUT: Store<bool> = Store::new("shout");

/// A waving hand, interned into the host's icon pool.
const WAVE: &str = r#"<path d="M5.5 9.5V4.75a1.25 1.25 0 0 1 2.5 0V8.5m0-4.25V3.25a1.25 1.25 0 0 1 2.5 0V8.5m0-3.75a1.25 1.25 0 0 1 2.5 0V11a5 5 0 0 1-5 5h-.4a5 5 0 0 1-3.9-1.9L2.3 11.4a1.3 1.3 0 0 1 2-1.6L5.5 11" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#;

/// Hello's process-wide half: publishes the Greeter service and an app menu item.
pub struct HelloApp;

impl AppPlugin for HelloApp {
    fn setup(cx: &mut AppSetupCx<Self>) -> Result<Self, PluginError> {
        cx.provide_service(&GREETER, Greeter { this: core::ptr::null(), greet, clicks, release: None });
        cx.add_menubar_item(Contribution::menubar_item(Menubar::Help, "about", "About Hello"), |_, cx| {
            cx.toast(&format!("{GREETING} from a native plugin"));
        });
        Ok(HelloApp)
    }
}

unsafe extern "C" fn greet(_: *const c_void, name: AbiStr, out: *mut AbiOwnedBytes) {
    // SAFETY: the caller passes a name valid for the call.
    let name = unsafe { name.as_str() };
    let who = if name.trim().is_empty() { "world" } else { name.trim() };
    let text = format!("{GREETING}, {who}!");
    // SAFETY: the caller passes a valid out pointer holding nothing it needs dropped.
    unsafe { out.write(AbiOwnedBytes::from_string(text)) };
}

unsafe extern "C" fn clicks(_: *const c_void) -> i64 {
    CLICKS.load(Ordering::Relaxed)
}

pub struct Hello {
    count: i64,
    name: EditorHandle,
    panic_in_render: bool,
}

impl WindowPlugin for Hello {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.icon("hello-wave", WAVE, 18.0);
        cx.add_command(
            CommandSpec::new("count", "Hello: Count up").description("Add one to the Hello panel's counter").bind("mod+shift+h").slash("hello"),
            |s, cx| {
                s.set(s.count + 1);
                cx.open_panel("main");
            },
        );
        cx.add_command(CommandSpec::new("panic", "Hello: Panic in a handler").description("Crash this plugin on purpose"), |_, _| {
            panic!("Hello panicked on purpose in a command");
        });
        cx.add_panel(PanelSpec::new("main", GREETING).description("A counter and a greeting").icon("hello-wave").order(40), Hello::panel);
        cx.add_setting(
            SettingSpec::new("shout", "Shout the greeting").description("Write the Hello panel's greeting in capitals").keywords("hello greeting"),
            |_, cx, _| {
                ui::switch(SHOUT.get().unwrap_or(false)).key("hello-shout").on_press(cx.handler(|_: &mut Hello, _| SHOUT.set(&!SHOUT.get().unwrap_or(false))))
            },
        );
        let name = cx.create_editor("");
        Ok(Hello { count: 0, name, panic_in_render: false })
    }

    fn save_hot_state(&self) -> Vec<u8> {
        self.count.to_le_bytes().to_vec()
    }

    fn load_hot_state(&mut self, bytes: &[u8]) {
        if let Some(count) = bytes.get(..8).and_then(|b| b.try_into().ok()) {
            self.set(i64::from_le_bytes(count));
        }
    }
}

impl Hello {
    fn set(&mut self, count: i64) {
        self.count = count;
        CLICKS.store(count, Ordering::Relaxed);
    }

    fn panel(&self, cx: &mut ViewCx<Self>, _props: &Props) -> El {
        let shout = SHOUT.get().unwrap_or(false);
        let version = (self.count, shout, self.panic_in_render, cx.editor_version(self.name), cx.dark());
        cx.memo("panel", version, |cx| {
            if self.panic_in_render {
                panic!("Hello panicked on purpose while drawing");
            }
            let p = cx.pal();
            let name = cx.editor_text(self.name);
            let who = if name.trim().is_empty() { "world" } else { name.trim() };
            let greeting = format!("{GREETING}, {who}!");
            let greeting = if shout { greeting.to_uppercase() } else { greeting };
            let button = |key: &'static str, label: &'static str| ui::button(label).key(key);
            col()
                .w_full()
                .h_full()
                .padding(16.0)
                .gap(16.0)
                .fill(p.bg_base)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(icon("hello-wave", 18.0).color(p.icon))
                        .child(text(greeting).key("hello-greeting").font(15.0, 640.0).line_height(24.0).color(p.text)),
                )
                .child(
                    col()
                        .w_full()
                        .padding(16.0)
                        .gap(12.0)
                        .radius(8.0)
                        .fill(p.layer1)
                        .border(0.5, p.border_muted)
                        .child(text("Counter").font(13.0, 530.0).color(p.muted))
                        .child(text(self.count.to_string()).key("hello-count").font(28.0, 600.0).line_height(34.0).mono().color(p.text))
                        .child(
                            row()
                                .gap(8.0)
                                .child(button("hello-minus", "−1").on_press(cx.handler(|s: &mut Hello, _| s.set(s.count - 1))))
                                .child(button("hello-plus", "+1").on_press(cx.handler(|s: &mut Hello, _| s.set(s.count + 1))))
                                .child(button("hello-reset", "Reset").on_press(cx.handler(|s: &mut Hello, _| s.set(0)))),
                        ),
                )
                .child(
                    col()
                        .w_full()
                        .gap(8.0)
                        .child(text("Your name").font(13.0, 530.0).color(p.muted))
                        .child(
                            text_input(self.name)
                                .key("hello-name")
                                .placeholder("Type a name")
                                .w_full()
                                .px(10.0)
                                .py(6.0)
                                .radius(6.0)
                                .fill(p.layer2)
                                .border(0.5, p.border)
                                .font(13.0, 440.0)
                                .color(p.text),
                        ),
                )
                .child(spacer())
                .child(
                    row()
                        .gap(8.0)
                        .child(button("hello-panic-render", "Panic in render").on_press(cx.handler(|s: &mut Hello, _| s.panic_in_render = true)))
                        .child(button("hello-panic-handler", "Panic in handler").on_press(cx.handler(|_: &mut Hello, _| {
                            panic!("Hello panicked on purpose in a press handler");
                        }))),
                )
        })
    }
}

opencode_plugin::declare_plugin!(app = HelloApp, window = Hello);
