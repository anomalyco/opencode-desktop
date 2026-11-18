//! The Studio console's lines, as Roblox Studio's Output keeps them: every plugin's `print`,
//! `warn`, errors and reloads, newest last; a repeat of the last line counts up. The Debug
//! plugin draws them (`Console:Lines`) with its command bar (`Console:Eval`), which runs Luau in
//! a plugin's live VM.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;

use mlua::{FromLua, Lua, MultiValue, Value};
use rune_luau::{Data, LogLevel, Reload, Script};

#[derive(Clone, Debug)]
pub struct Line {
    pub plugin: String,
    pub level: LogLevel,
    pub message: String,
    /// `file.luau:line` from the message, if it names one.
    pub at: Option<(String, u32)>,
    pub count: u32,
}

thread_local! {
    static LINES: RefCell<VecDeque<Line>> = const { RefCell::new(VecDeque::new()) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Adds a line from plugin `plugin`; a repeat of the last line counts up.
pub fn push(plugin: &str, level: LogLevel, message: &str) {
    if level != LogLevel::Info {
        crate::tasks::log(format!("plugin {plugin}: {message}"));
    }
    LINES.with_borrow_mut(|lines| {
        if let Some(last) = lines.back_mut()
            && last.plugin == plugin
            && last.level == level
            && last.message == message
        {
            last.count += 1;
            return;
        }
        if lines.len() == 500 {
            lines.pop_front();
        }
        lines.push_back(Line {
            plugin: plugin.to_owned(),
            level,
            message: message.to_owned(),
            at: location(message),
            count: 1,
        });
    });
    VERSION.set(VERSION.get() + 1);
}

pub fn reloaded(plugin: &str, r: Reload) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    crate::tasks::log(format!(
        "plugin {plugin}: reloaded: compile {:.3} ms off the UI thread, swap {:.3} ms on it",
        ms(r.compile),
        ms(r.swap)
    ));
    push(
        plugin,
        LogLevel::Info,
        &format!(
            "Reloaded in {:.2} ms (compile {:.2} ms off the UI thread, swap {:.2} ms)",
            ms(r.compile) + ms(r.swap),
            ms(r.compile),
            ms(r.swap)
        ),
    );
}

/// Moves with every line written, and with Clear.
pub fn version() -> u64 {
    VERSION.get()
}

pub fn clear() {
    LINES.with_borrow_mut(VecDeque::clear);
    VERSION.set(VERSION.get() + 1);
}

/// The first `name.luau:line` in a message.
fn location(message: &str) -> Option<(String, u32)> {
    let at = message.find(".luau:")?;
    let start = message[..at]
        .rfind(|c: char| c.is_whitespace() || c == '[' || c == '"' || c == '@')
        .map_or(0, |i| i + 1);
    let digits: String = message[at + 6..].chars().take_while(char::is_ascii_digit).collect();
    Some((message[start..at + 5].to_owned(), digits.parse().ok()?))
}

/// Every line as `ConsoleLine` records, oldest first. `resolve_dir` returns a plugin's folder on
/// disk when it runs from one, used to build `file://` source links.
pub fn lines_with<'a>(mut resolve_dir: impl FnMut(&str) -> Option<&'a Path>) -> Data {
    lines_with_href(|plugin, file, line| {
        let path = resolve_dir(plugin)?.join(file);
        Some(format!("file://{}#L{line}", path.display()))
    })
}

/// Every line as `ConsoleLine` records, oldest first, using `source_href(plugin, file, line)` to
/// build each line's optional `href` link.
pub fn lines_with_href(mut source_href: impl FnMut(&str, &str, u32) -> Option<String>) -> Data {
    LINES.with_borrow(|lines| {
        Data::List(
            lines
                .iter()
                .map(|l| {
                    let level = match l.level {
                        LogLevel::Error => "error",
                        LogLevel::Warn => "warn",
                        LogLevel::Info => "info",
                    };
                    let href = l.at.as_ref().and_then(|(file, line)| source_href(&l.plugin, file, *line));
                    crate::luau::map([
                        ("plugin", Data::String(l.plugin.clone())),
                        ("level", Data::String(level.into())),
                        ("message", Data::String(l.message.clone())),
                        ("file", l.at.as_ref().map_or(Data::Nil, |(f, _)| Data::String(f.clone()))),
                        ("line", l.at.as_ref().map_or(Data::Nil, |(_, n)| Data::Integer(i64::from(*n)))),
                        ("href", href.map_or(Data::Nil, Data::String)),
                        ("count", Data::Integer(i64::from(l.count))),
                    ])
                })
                .collect(),
        )
    })
}

/// Runs `source` in `script`'s VM under its watchdog: as an expression first (`return <source>`),
/// else as a chunk. Call inside the host's execution context if bindings read host state.
pub fn eval_script(script: &Script, source: &str) -> Result<String, String> {
    let f = script.with_lua(|lua| compile(lua, source)).ok_or("the plugin has no VM")??;
    let values = script.eval_fn::<MultiValue>(&f, ()).map_err(|e| e.message)?;
    script.with_lua(|lua| show(lua, values)).ok_or_else(|| "the plugin has no VM".to_owned())
}

/// [`eval_script`] in the VM asking: the console's own plugin, already running.
pub fn eval_here(lua: &Lua, source: &str) -> Result<String, String> {
    let f = compile(lua, source)?;
    let values = f.call::<MultiValue>(()).map_err(|e| e.to_string())?;
    Ok(show(lua, values))
}

/// Compiles `source` as an expression (`return <source>`) first, else as a chunk.
pub fn compile(lua: &Lua, source: &str) -> Result<mlua::Function, String> {
    lua.load(format!("return {source}"))
        .set_name("=console")
        .into_function()
        .or_else(|_| lua.load(source).set_name("=console").into_function())
        .map_err(|e| e.to_string())
}

/// Values as the console shows them: plain data as JSON, anything else as Luau names it.
pub fn show(lua: &Lua, values: MultiValue) -> String {
    if values.is_empty() {
        return "nil".to_owned();
    }
    let one = |v: Value| match &v {
        Value::Nil => "nil".to_owned(),
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        Value::Table(_) => Data::from_lua(v.clone(), lua)
            .ok()
            .map_or_else(|| v.to_string().unwrap_or_default(), |d| serde_json::to_string_pretty(&d.to_json()).unwrap_or_default()),
        _ => v.to_string().unwrap_or_else(|_| v.type_name().to_owned()),
    };
    values.into_iter().map(one).collect::<Vec<_>>().join("\t")
}
