//! Every contribution point beyond commands, panels and settings rows, as Desktop's plugin API
//! names them: menu items, app menu items, settings pages, link handlers, titlebar items,
//! composer pills, code block actions and renderers, shell slots, themes, file viewers, server
//! sources and MCP tools. One [`Contribution`] spec describes any of them; the setup methods
//! below attach the callbacks each kind needs.

use std::borrow::Cow;

use crate::abi::{AbiStatus, AbiStr, Callback, ContributionAbi, QueryCallback, RenderCallback, contrib};
use crate::app::AppSetupCx;
use crate::boundary::{ctx, drop_packed, host};
use crate::cx::{ActionCx, Props, ViewCx, WindowSetupCx, action_callback, query_callback, render_callback};
use crate::el::El;

type Text = Cow<'static, str>;

/// Where a [`Contribution::menu_item`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// The side panel's `+` menu and its New tab cards.
    SessionPanel,
    /// Settings › Servers' Add server menu.
    ServerAdd,
    /// Each server row's menu in Settings › Servers; `run`'s input is the server's id.
    ServerRow,
}

/// The app menu's submenus (`MenubarItem.menu`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menubar {
    App,
    File,
    Edit,
    View,
    Go,
    Window,
    Help,
}

/// The shell's fixed slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotName {
    /// Across the window's bottom (the debug bar).
    WindowBottom,
    /// The session header's trailing end, before the usage numbers.
    SessionHeader,
    /// The session location menu.
    SessionLocationMenu,
    /// After the side panel's tabs.
    SessionPanelEnd,
}

/// A host settings page a [`Contribution::settings_page`] adds a section to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPage {
    General,
    Servers,
}

/// A server's state in a server source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerState {
    Stopped = 0,
    Starting = 1,
    Auth = 2,
    Ready = 3,
    Failed = 4,
    /// It answers, but runs a version this app can't use.
    Incompatible = 5,
}

/// One server a server source lists (`ActionCx::set_servers`), keyed `<plugin>:<id>` in the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    /// Its kind's badge (`WSL`, `SSH`).
    pub label: String,
    pub state: ServerState,
    /// Its HTTP address and password once ready.
    pub url: String,
    pub password: String,
    /// It joins the app's servers (the window can connect to it) rather than only showing in
    /// Settings.
    pub listed: bool,
}

/// One contribution: a kind and the fields it uses.
#[derive(Clone, Debug)]
pub struct Contribution {
    kind: u32,
    id: Text,
    title: Text,
    description: Text,
    keywords: Text,
    icon: Text,
    target: Text,
    order: i32,
    flags: u32,
}

impl Contribution {
    /// A popover the host places on `side` of its anchor (`abi::SIDE_BOTTOM` …), aligned along
    /// it (`abi::ALIGN_END` …), `gap` points away; opened with `ActionCx::toggle_popover`.
    pub fn popover(id: impl Into<Text>, label: impl Into<Text>, side: u32, align: u32, gap: i32) -> Self {
        Contribution { order: gap, flags: crate::abi::popover_flags(side, align), ..Self::new(contrib::POPOVER, id, label) }
    }

    fn new(kind: u32, id: impl Into<Text>, title: impl Into<Text>) -> Self {
        Contribution {
            kind,
            id: id.into(),
            title: title.into(),
            description: Text::Borrowed(""),
            keywords: Text::Borrowed(""),
            icon: Text::Borrowed(""),
            target: Text::Borrowed(""),
            order: 0,
            flags: 0,
        }
    }

    /// A menu item (`MenuItem`).
    pub fn menu_item(menu: Menu, id: impl Into<Text>, title: impl Into<Text>) -> Self {
        let target = match menu {
            Menu::SessionPanel => "session.panel",
            Menu::ServerAdd => "server.add",
            Menu::ServerRow => "server.row",
        };
        Contribution { target: Text::Borrowed(target), ..Self::new(contrib::MENU_ITEM, id, title) }
    }

    /// An app menu item (`MenubarItem`); belongs to the app half.
    pub fn menubar_item(menu: Menubar, id: impl Into<Text>, label: impl Into<Text>) -> Self {
        let target = match menu {
            Menubar::App => "app",
            Menubar::File => "file",
            Menubar::Edit => "edit",
            Menubar::View => "view",
            Menubar::Go => "go",
            Menubar::Window => "window",
            Menubar::Help => "help",
        };
        Contribution { target: Text::Borrowed(target), ..Self::new(contrib::MENUBAR_ITEM, id, label) }
    }

    /// A Settings page of its own (`SettingsPage`); [`Self::on_page`] makes it a section of a
    /// host page instead. Its [`Self::entry`] rows are what settings search finds.
    pub fn settings_page(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        Self::new(contrib::SETTINGS_PAGE, id, title)
    }

    /// Opens links it matches (`LinkHandler`); the highest `priority` wins.
    pub fn link_handler(id: impl Into<Text>, priority: i32) -> Self {
        Contribution { order: priority, ..Self::new(contrib::LINK_HANDLER, id, "") }
    }

    /// A pill at the titlebar's trailing end (`TitlebarItem`); `title` is its tooltip.
    pub fn titlebar_item(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        Self::new(contrib::TITLEBAR_ITEM, id, title)
    }

    /// A capsule above the root session's composer (`ComposerPill`).
    pub fn composer_pill(id: impl Into<Text>) -> Self {
        Self::new(contrib::COMPOSER_PILL, id, "")
    }

    /// A hover button beside code blocks' Copy (`CodeBlockAction`); `label` is its tooltip.
    pub fn code_block_action(id: impl Into<Text>, label: impl Into<Text>) -> Self {
        Self::new(contrib::CODE_BLOCK_ACTION, id, label)
    }

    /// Draws finished fenced code blocks in `languages` (`CodeBlock`).
    pub fn code_block(id: impl Into<Text>, languages: &[&str]) -> Self {
        Contribution { target: Text::Owned(languages.join(" ")), ..Self::new(contrib::CODE_BLOCK, id, "") }
    }

    /// Fills a shell slot (`Slot`).
    pub fn slot(slot: SlotName, id: impl Into<Text>) -> Self {
        let target = match slot {
            SlotName::WindowBottom => "window.bottom",
            SlotName::SessionHeader => "session.header",
            SlotName::SessionLocationMenu => "session.location.menu",
            SlotName::SessionPanelEnd => "session.panel.end",
        };
        Contribution { target: Text::Borrowed(target), ..Self::new(contrib::SLOT, id, "") }
    }

    /// A colour theme (`Theme`), listed as `<plugin>:<id>` in Appearance. `json` holds `light`
    /// and `dark` token maps, as Desktop's theme files do.
    pub fn theme(id: impl Into<Text>, name: impl Into<Text>, json: impl Into<Text>) -> Self {
        Contribution { target: json.into(), ..Self::new(contrib::THEME, id, name) }
    }

    /// Shows files with these extensions (`FileViewer`).
    pub fn file_viewer(id: impl Into<Text>, title: impl Into<Text>, extensions: &[&str]) -> Self {
        Contribution { target: Text::Owned(extensions.join(" ")), ..Self::new(contrib::FILE_VIEWER, id, title) }
    }

    /// A source of servers (`Server`); fill it with `ActionCx::set_servers`. [`Self::order`]
    /// places its servers among the app's.
    pub fn server_source(id: impl Into<Text>, title: impl Into<Text>) -> Self {
        Self::new(contrib::SERVER, id, title)
    }

    /// A modal dialog (`dialogs.open`), opened with `ActionCx::open_dialog`.
    pub fn dialog(id: impl Into<Text>, label: impl Into<Text>) -> Self {
        Self::new(contrib::DIALOG, id, label)
    }

    /// A tool on the app's client MCP (`McpTool`), named `<plugin>_<name>` there, with its
    /// JSON input schema.
    pub fn mcp_tool(name: impl Into<Text>, description: impl Into<Text>, schema: impl Into<Text>) -> Self {
        Contribution { description: description.into(), target: schema.into(), ..Self::new(contrib::MCP_TOOL, name, "") }
    }

    pub fn description(mut self, description: impl Into<Text>) -> Self {
        self.description = description.into();
        self
    }

    /// An icon this plugin interned, or a host icon name.
    pub fn icon(mut self, icon: impl Into<Text>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Ascending display order (a link handler's priority).
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// A code block action only for these languages.
    pub fn languages(mut self, languages: &[&str]) -> Self {
        self.target = Text::Owned(languages.join(" "));
        self
    }

    /// A settings page as a section of a host page.
    pub fn on_page(mut self, page: HostPage) -> Self {
        self.target = Text::Borrowed(match page {
            HostPage::General => "general",
            HostPage::Servers => "servers",
        });
        self
    }

    /// A settings page listed beside Experimental and About rather than with the app's pages
    /// (`SettingsPage.group: "about"`), as What's new is.
    pub fn about_group(mut self) -> Self {
        self.flags |= crate::abi::SETTINGS_PAGE_ABOUT;
        self
    }

    /// A settings search entry on this page: found without drawing the page.
    pub fn entry(mut self, id: &str, title: &str, description: &str, keywords: &str) -> Self {
        let line = format!("{id}\t{title}\t{description}\t{keywords}").replace('\n', " ");
        let mut all = self.keywords.into_owned();
        if !all.is_empty() {
            all.push('\n');
        }
        all.push_str(&line);
        self.keywords = Text::Owned(all);
        self
    }

    fn add(&self, run: Callback, render: RenderCallback, query: QueryCallback) -> bool {
        let abi = ContributionAbi {
            kind: self.kind,
            flags: self.flags,
            order: self.order,
            _pad: 0,
            id: AbiStr::new(&self.id),
            title: AbiStr::new(&self.title),
            description: AbiStr::new(&self.description),
            keywords: AbiStr::new(&self.keywords),
            icon: AbiStr::new(&self.icon),
            target: AbiStr::new(&self.target),
            run,
            render,
            query,
        };
        // SAFETY: `abi` borrows `self` for the call; the host takes the callbacks when it accepts.
        let ok = unsafe { (host().add_contribution)(ctx(), &abi) } == AbiStatus::Ok;
        if !ok {
            drop_packed(run.data, run.drop_data);
            drop_packed(render.data, render.drop_data);
            drop_packed(query.data, query.drop_data);
        }
        ok
    }
}

impl<P: 'static> WindowSetupCx<P> {
    /// A [`Contribution::menu_item`].
    pub fn add_menu_item(&mut self, spec: Contribution, run: impl Fn(&mut P, &mut ActionCx) + 'static) {
        spec.add(action_callback::<P, _>(run), RenderCallback::NONE, QueryCallback::NONE);
    }

    /// A [`Contribution::settings_page`]; `render` draws its body.
    pub fn add_settings_page(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::link_handler`]: `matches` says whether it takes an href, `open` opens
    /// it (`cx.input()` is the href).
    pub fn add_link_handler(&mut self, spec: Contribution, matches: impl Fn(&P, &str) -> bool + 'static, open: impl Fn(&mut P, &mut ActionCx) + 'static) {
        let query = query_callback::<P, _>(move |s, _, href| Ok(matches(s, href).then(String::new)));
        spec.add(action_callback::<P, _>(open), RenderCallback::NONE, query);
    }

    /// A [`Contribution::titlebar_item`]: `render` draws the pill, `run` presses it.
    pub fn add_titlebar_item(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static, run: impl Fn(&mut P, &mut ActionCx) + 'static) {
        spec.add(action_callback::<P, _>(run), render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::composer_pill`]: `render` draws it for the session in `props`
    /// (`cx.nothing()` hides it), `run` presses it.
    pub fn add_composer_pill(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static, run: impl Fn(&mut P, &mut ActionCx) + 'static) {
        spec.add(action_callback::<P, _>(run), render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::code_block_action`]: `run` gets the code as `cx.input()` and its
    /// language as `cx.input_extra()`.
    pub fn add_code_block_action(&mut self, spec: Contribution, run: impl Fn(&mut P, &mut ActionCx) + 'static) {
        spec.add(action_callback::<P, _>(run), RenderCallback::NONE, QueryCallback::NONE);
    }

    /// A [`Contribution::code_block`]: `render` draws `props.text` in language `props.path`.
    pub fn add_code_block(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::popover`]: `render` draws its content for the session in `props`.
    pub fn add_popover(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::slot`].
    pub fn add_slot(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::theme`].
    pub fn add_theme(&mut self, spec: Contribution) {
        spec.add(Callback::NONE, RenderCallback::NONE, QueryCallback::NONE);
    }

    /// A [`Contribution::file_viewer`]: `render` draws the file at `props.path`.
    pub fn add_file_viewer(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::server_source`]: `row` draws a server's connection row in Settings ›
    /// Servers (`props.id` is its id).
    pub fn add_server_source(&mut self, spec: Contribution, row: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static) {
        spec.add(Callback::NONE, render_callback::<P, _>(row), QueryCallback::NONE);
    }

    /// A [`Contribution::dialog`]: `render` draws its card, `dismissed` hears Esc or a press
    /// outside closing it.
    pub fn add_dialog(&mut self, spec: Contribution, render: impl Fn(&P, &mut ViewCx<P>, &Props) -> El + 'static, dismissed: impl Fn(&mut P, &mut ActionCx) + 'static) {
        spec.add(action_callback::<P, _>(dismissed), render_callback::<P, _>(render), QueryCallback::NONE);
    }

    /// A [`Contribution::mcp_tool`]: `call` gets the arguments' JSON and answers JSON or text.
    pub fn add_mcp_tool(&mut self, spec: Contribution, call: impl Fn(&mut P, &mut ActionCx, &str) -> Result<String, String> + 'static) {
        let query = query_callback::<P, _>(move |s, cx, args| call(s, cx, args).map(Some));
        spec.add(Callback::NONE, RenderCallback::NONE, query);
    }
}

impl<A: 'static> AppSetupCx<A> {
    /// A [`Contribution::menubar_item`].
    pub fn add_menubar_item(&mut self, spec: Contribution, run: impl Fn(&mut A, &mut ActionCx) + 'static) {
        spec.add(action_callback::<A, _>(run), RenderCallback::NONE, QueryCallback::NONE);
    }
}
