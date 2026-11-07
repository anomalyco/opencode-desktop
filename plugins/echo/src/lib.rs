//! Echo: a third-party plugin that calls another plugin's service in-process. It asks Hello's
//! Greeter (`hello-api`) for a greeting, shows the service's live state (pending, active with
//! its generation, or inactive), and counts the answer's words on a host worker thread.
//!
//! It also fills every contribution point once, as a working sample of each: two panels, a
//! command, a `+` menu item, an app menu item, a settings page with search entries, a link
//! handler (`echo://<name>`), a titlebar item, a composer pill, a code block action, a header
//! slot, a theme, a file viewer (`.echo`), a server source and an MCP tool.

use opencode_plugin::prelude::*;
use opencode_plugin_hello_api::{GREETER, Greeter};

/// The name last asked about, kept by the host across launches.
const LAST_NAME: Store<String> = Store::new("last-name");
/// Whether the composer pill shows.
const PILL: Store<bool> = Store::new("pill");

const ECHO_ICON: &str = r#"<path d="M3 10a7 7 0 0 1 14 0M6 10a4 4 0 0 1 8 0M10 10v.01" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none"/><path d="M10 13v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#;

const THEME: &str = r##"{
  "light": { "background-bg-base": "#f4f1f8ff", "background-bg-deep": "#ebe6f2ff", "background-bg-layer-01": "#e7e1efff", "background-bg-accent": "#7a4fd6ff" },
  "dark": { "background-bg-base": "#1d1828ff", "background-bg-deep": "#16121fff", "background-bg-layer-01": "#262033ff", "background-bg-accent": "#9c7bf0ff" }
}"##;

/// Echo's process-wide half: an app menu item that asks Hello.
pub struct EchoApp {
    greeter: Live<Greeter>,
}

impl AppPlugin for EchoApp {
    fn setup(cx: &mut AppSetupCx<Self>) -> Result<Self, PluginError> {
        let greeter = cx.consume(&GREETER);
        cx.add_menubar_item(Contribution::menubar_item(Menubar::View, "ask", "Ask Hello (Echo)"), |s, cx| {
            let answer = s.greeter.with(|g| g.greet("app menu")).unwrap_or_else(|| "Hello isn't running".to_owned());
            cx.toast(&answer);
        });
        Ok(EchoApp { greeter })
    }
}

pub struct Echo {
    greeter: Live<Greeter>,
    name: EditorHandle,
    answer: String,
    asked: u32,
    words: LatestTask<usize>,
    word_count: Option<usize>,
}

impl WindowPlugin for Echo {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        let greeter = cx.consume(&GREETER);
        cx.icon("echo", ECHO_ICON, 20.0);
        let name = cx.create_editor(&LAST_NAME.get().unwrap_or_default());
        let words = cx.latest_task(120);

        cx.add_panel(PanelSpec::new("main", "Echo").description("Ask Hello's Greeter service from another plugin").icon("echo").order(41), Echo::panel);
        cx.add_panel(PanelSpec::new("log", "Echo log").description("What Echo asked, and the service's state").icon("echo").order(42), Echo::log);
        cx.add_command(CommandSpec::new("ask", "Echo: Ask Hello").description("Ask Hello's Greeter for a greeting").bind("mod+shift+e").slash("echo"), |s, cx| {
            s.ask(cx);
            cx.open_panel("main");
        });
        cx.add_menu_item(Contribution::menu_item(Menu::SessionPanel, "ask", "Ask Hello").icon("echo").order(43), |s, cx| {
            s.ask(cx);
            cx.open_panel("main");
        });
        cx.add_settings_page(
            Contribution::settings_page("echo", "Echo")
                .entry("echo-pill", "Show Echo above the composer", "A capsule with Hello's last greeting", "echo pill composer")
                .entry("echo-name", "Name Echo asks about", "Remembered between launches", "echo name greeting"),
            |_, cx, _| {
                let p = cx.pal();
                let on = PILL.get().unwrap_or(true);
                row()
                    .w_full()
                    .gap(12.0)
                    .center_y()
                    .child(text("Show Echo above the composer").font(13.0, 530.0).color(p.text))
                    .child(spacer())
                    .child(ui::switch(on).key("echo-pill-switch").on_press(cx.handler(|_: &mut Echo, _| PILL.set(&!PILL.get().unwrap_or(true)))))
            },
        );
        cx.add_link_handler(
            Contribution::link_handler("echo-links", 10),
            |_, href| href.starts_with("echo://"),
            |s, cx| {
                let who = cx.input().trim_start_matches("echo://").to_owned();
                cx.set_editor_text(s.name, &who);
                s.ask(cx);
                cx.open_panel("main");
            },
        );
        cx.add_titlebar_item(
            Contribution::titlebar_item("echo", "Echo: Hello's Greeter service"),
            |s, cx, _| {
                let label = match s.greeter.get() {
                    LiveState::Active { generation, .. } => format!("Echo · gen {generation}"),
                    LiveState::Pending => "Echo · waiting".to_owned(),
                    LiveState::Inactive(_) => "Echo · off".to_owned(),
                };
                cx.memo("titlebar", &label, |_| ui::pill("echo", label.clone()))
            },
            |_, cx| cx.open_panel("main"),
        );
        cx.add_composer_pill(
            Contribution::composer_pill("echo"),
            |s, cx, _| {
                if !PILL.get().unwrap_or(true) || s.answer.is_empty() {
                    return cx.nothing();
                }
                cx.memo("pill", &s.answer, |_| ui::pill("echo", format!("Hello says: {}", s.answer)))
            },
            |_, cx| cx.open_panel("main"),
        );
        cx.add_code_block_action(Contribution::code_block_action("ask", "Ask Hello about this").icon("echo"), |s, cx| {
            let first = cx.input().lines().next().unwrap_or_default().chars().take(40).collect::<String>();
            cx.set_editor_text(s.name, &first);
            s.ask(cx);
            cx.open_panel("main");
        });
        cx.add_slot(Contribution::slot(SlotName::SessionHeader, "state"), |s, cx, _| {
            let v = s.greeter.version();
            cx.memo("slot", v, |cx| {
                let p = cx.pal();
                let (label, color) = match s.greeter.get() {
                    LiveState::Active { .. } => ("Greeter on", p.success),
                    LiveState::Pending => ("Greeter waiting", p.warning),
                    LiveState::Inactive(_) => ("Greeter off", p.danger),
                };
                row().h(20.0).px(6.0).gap(5.0).center_y().radius(10.0).fill(p.layer1).child(el().size(6.0, 6.0).radius(3.0).fill(color)).child(text(label).font(11.0, 500.0).color(p.muted))
            })
        });
        cx.add_theme(Contribution::theme("dusk", "Echo Dusk", THEME));
        cx.add_file_viewer(Contribution::file_viewer("viewer", "Echo file", &["echo"]), |_, cx, props| {
            let (path, loaded) = (props.path.clone(), props.text.clone());
            cx.memo("file", (&path, &loaded), |cx| {
                let p = cx.pal();
                // The host hands over the file as it loaded it; a file it hasn't is read here.
                let body = if loaded.is_empty() { std::fs::read_to_string(&path).unwrap_or_else(|e| format!("Couldn't read {path}: {e}")) } else { loaded.clone() };
                col()
                    .w_full()
                    .padding(24.0)
                    .gap(12.0)
                    .child(text("Echo file").font(15.0, 640.0).color(p.text))
                    .child(text(body.to_uppercase()).font(13.0, 440.0).wrap().color(p.muted))
            })
        });
        cx.add_server_source(Contribution::server_source("loopback", "Echo loopback"), |_, cx, props| {
            let p = cx.pal();
            text(format!("Echo loopback ({})", props.id)).font(13.0, 530.0).color(p.text)
        });
        cx.add_mcp_tool(
            Contribution::mcp_tool("greet", "Ask Hello's Greeter service (through the Echo plugin) to greet someone.", r#"{"type":"object","properties":{"name":{"type":"string"}},"additionalProperties":false}"#),
            |s, _, args| {
                let name = args.split("\"name\"").nth(1).and_then(|rest| rest.split('"').nth(1)).unwrap_or("world").to_owned();
                s.greeter.with(|g| format!("{{\"greeting\":\"{}\"}}", g.greet(&name).replace('"', "'"))).ok_or_else(|| "Hello's Greeter service isn't running".to_owned())
            },
        );
        Ok(Echo { greeter, name, answer: String::new(), asked: 0, words, word_count: None })
    }

    fn poll(&mut self, _cx: &mut PollCx) -> bool {
        let Some(count) = self.words.take() else { return false };
        self.word_count = Some(count);
        true
    }
}

impl Echo {
    fn ask(&mut self, cx: &mut ActionCx) {
        let name = cx.editor_text(self.name);
        LAST_NAME.set(&name);
        self.asked += 1;
        self.answer = self.greeter.with(|g| g.greet(&name)).unwrap_or_default();
        self.word_count = None;
        let answer = self.answer.clone();
        self.words.request(move || answer.split_whitespace().count());
        cx.set_servers(
            "loopback",
            &[ServerEntry { id: "echo".into(), name: "Echo loopback".into(), label: "Echo".into(), state: ServerState::Stopped, url: String::new(), password: String::new(), listed: false }],
            true,
        );
    }

    fn state_line(&self) -> (String, bool) {
        match self.greeter.get() {
            LiveState::Active { generation, .. } => (format!("Greeter service: active, generation {generation}"), true),
            LiveState::Pending => ("Greeter service: waiting for Hello".to_owned(), false),
            LiveState::Inactive(Inactive::Disabled) => ("Greeter service: Hello is off".to_owned(), false),
            LiveState::Inactive(Inactive::Failed) => ("Greeter service: Hello failed".to_owned(), false),
            LiveState::Inactive(Inactive::Mismatch) => ("Greeter service: a different version".to_owned(), false),
        }
    }

    fn panel(&self, cx: &mut ViewCx<Self>, _props: &Props) -> El {
        let clicks = self.greeter.with(Greeter::clicks);
        let version = (self.greeter.version(), clicks, &self.answer, self.word_count, cx.editor_version(self.name), cx.dark());
        cx.memo("panel", version, |cx| {
            let p = cx.pal();
            let (state, ok) = self.state_line();
            let answer = if self.answer.is_empty() { "Ask Hello for a greeting.".to_owned() } else { self.answer.clone() };
            let words = match (self.answer.is_empty(), self.word_count) {
                (true, _) => String::new(),
                (false, Some(n)) => format!("{n} words, counted on a host worker thread"),
                (false, None) => "Counting words…".to_owned(),
            };
            col()
                .w_full()
                .h_full()
                .padding(16.0)
                .gap(16.0)
                .fill(p.bg_base)
                .child(row().gap(8.0).center_y().child(icon("echo", 18.0).color(p.icon)).child(text("Echo").font(15.0, 640.0).line_height(24.0).color(p.text)))
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(el().size(8.0, 8.0).radius(4.0).fill(if ok { p.success } else { p.warning }))
                        .child(text(state).key("echo-state").font(13.0, 440.0).color(p.muted)),
                )
                .child(
                    col()
                        .w_full()
                        .padding(16.0)
                        .gap(10.0)
                        .radius(8.0)
                        .fill(p.layer1)
                        .border(0.5, p.border_muted)
                        .child(text("Hello's answer").font(13.0, 530.0).color(p.muted))
                        .child(text(answer).key("echo-answer").font(17.0, 600.0).line_height(24.0).wrap().color(p.text))
                        .child(text(words).key("echo-words").font(12.0, 440.0).color(p.faint))
                        .child(text(format!("Hello's counter, read through the service: {}", clicks.map_or("—".to_owned(), |c| c.to_string()))).key("echo-clicks").font(12.0, 440.0).color(p.faint)),
                )
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .child(
                            text_input(self.name)
                                .key("echo-name")
                                .placeholder("A name to greet")
                                .grow(1.0)
                                .px(10.0)
                                .py(6.0)
                                .radius(6.0)
                                .fill(p.layer2)
                                .border(0.5, p.border)
                                .font(13.0, 440.0)
                                .color(p.text),
                        )
                        .child(ui::button("Ask Hello").key("echo-ask").on_press(cx.handler(|s: &mut Echo, cx| s.ask(cx)))),
                )
        })
    }

    fn log(&self, cx: &mut ViewCx<Self>, _props: &Props) -> El {
        let version = (self.greeter.version(), self.asked, cx.dark());
        cx.memo("log", version, |cx| {
            let p = cx.pal();
            col()
                .w_full()
                .h_full()
                .padding(16.0)
                .gap(8.0)
                .fill(p.bg_base)
                .child(text("Echo log").font(15.0, 640.0).color(p.text))
                .child(text(self.state_line().0).font(13.0, 440.0).color(p.muted))
                .child(text(format!("Asked {} times", self.asked)).key("echo-asked").font(13.0, 440.0).color(p.muted))
        })
    }
}

opencode_plugin::export_plugin!(app = EchoApp, window = Echo);
