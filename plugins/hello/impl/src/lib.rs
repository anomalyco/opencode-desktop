//! The smallest useful plugin: a side panel with a counter and a text field, a palette command
//! with a keybind, and a settings row. It exists to prove the plugin boundary, so it also
//! carries two deliberate panics (one in a handler, one in render).

use opencode_plugin::prelude::*;

const GREETING: &str = "Hello";

pub struct Hello {
    count: i64,
    name: EditorHandle,
    shout: bool,
    panic_in_render: bool,
}

impl WindowPlugin for Hello {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.add_command(
            CommandSpec::new("count", "Hello: Count up").description("Add one to the Hello panel's counter").bind("mod+shift+h"),
            |s, cx| {
                s.count += 1;
                cx.open_panel("main");
            },
        );
        cx.add_command(CommandSpec::new("panic", "Hello: Panic in a handler").description("Crash this plugin on purpose"), |_, _| {
            panic!("Hello panicked on purpose in a command");
        });
        cx.add_panel(PanelSpec::new("main", GREETING).icon("code").order(40), Hello::panel);
        cx.add_setting(
            SettingSpec::new("shout", "Shout the greeting").description("Write the Hello panel's greeting in capitals").keywords("hello greeting"),
            |s, cx, _| ui::switch(s.shout).key("hello-shout").on_press(cx.handler(|s: &mut Hello, _| s.shout = !s.shout)),
        );
        let name = cx.create_editor("");
        Ok(Hello { count: 0, name, shout: false, panic_in_render: false })
    }

    fn save_hot_state(&self) -> Vec<u8> {
        let mut bytes = self.count.to_le_bytes().to_vec();
        bytes.push(u8::from(self.shout));
        bytes
    }

    fn load_hot_state(&mut self, bytes: &[u8]) {
        if let Some(count) = bytes.get(..8).and_then(|b| b.try_into().ok()) {
            self.count = i64::from_le_bytes(count);
        }
        self.shout = bytes.get(8) == Some(&1);
    }
}

impl Hello {
    fn panel(&self, cx: &mut ViewCx<Self>, _props: &Props) -> El {
        let version = (self.count, self.shout, self.panic_in_render, cx.editor_version(self.name), cx.dark());
        cx.memo("panel", version, |cx| {
            if self.panic_in_render {
                panic!("Hello panicked on purpose while drawing");
            }
            let p = cx.pal();
            let name = cx.editor_text(self.name);
            let who = if name.trim().is_empty() { "world" } else { name.trim() };
            let greeting = format!("{GREETING}, {who}!");
            let greeting = if self.shout { greeting.to_uppercase() } else { greeting };
            let button = |key: &'static str, label: &'static str| ui::button(label).key(key);
            col()
                .w_full()
                .h_full()
                .padding(16.0)
                .gap(16.0)
                .fill(p.bg_base)
                .child(text(greeting).key("hello-greeting").font(15.0, 640.0).line_height(24.0).color(p.text))
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
                                .child(button("hello-minus", "−1").on_press(cx.handler(|s: &mut Hello, _| s.count -= 1)))
                                .child(button("hello-plus", "+1").on_press(cx.handler(|s: &mut Hello, _| s.count += 1)))
                                .child(button("hello-reset", "Reset").on_press(cx.handler(|s: &mut Hello, _| s.count = 0))),
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

opencode_plugin::export_plugin!(window = Hello);
