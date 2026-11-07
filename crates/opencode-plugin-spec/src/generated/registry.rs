//! Contribution kinds, services and context events, with the halves that offer them.
//! Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.

use crate::{Half, string_enum};

/// The contract version this crate describes.
pub const CONTRACT: &str = "1.0";
pub const CONTRACT_MAJOR: u16 = 1;
pub const CONTRACT_MINOR: u16 = 0;

string_enum! {
    /// A contribution kind, by its contract name.
    pub enum ContributionKind {
        /// Adds a command to the palette, its keybind and its slash command.
        Command = "command",
        /// Adds a panel: tabs in the session's side region.
        Panel = "panel",
        /// Adds a row to Settings › Plugins.
        Setting = "setting",
        /// Adds a settings page, or a section on a host page.
        SettingsPage = "settingsPage",
        /// Adds an item to a host menu.
        MenuItem = "menuItem",
        /// Adds an item to the app menu bar.
        MenubarItem = "menubarItem",
        /// Opens local links it matches.
        LinkHandler = "linkHandler",
        /// Adds a pill at the titlebar's end.
        TitlebarItem = "titlebarItem",
        /// Adds a pill above the session composer.
        ComposerPill = "composerPill",
        /// Renders fenced code blocks of some languages.
        CodeBlock = "codeBlock",
        /// Adds a button beside code blocks' Copy button.
        CodeBlockAction = "codeBlockAction",
        /// Fills one of the shell's slots.
        Slot = "slot",
        /// Adds a color theme.
        Theme = "theme",
        /// Shows files with some extensions.
        FileViewer = "fileViewer",
        /// Adds a source of servers. Add it in the app half so every window shares one list.
        ServerSource = "serverSource",
        /// Adds a modal dialog, opened with `Dialog:Open`.
        Dialog = "dialog",
        /// Adds popover content, opened with `Popover:Open` or `ui.flyoutRow`.
        Popover = "popover",
        /// Adds a tool to the app's client MCP server.
        McpTool = "mcpTool",
    }
}

impl ContributionKind {
    /// The context method that adds it, such as `AddCommand`.
    pub fn method(self) -> &'static str {
        match self {
            Self::Command => "AddCommand",
            Self::Panel => "AddPanel",
            Self::Setting => "AddSetting",
            Self::SettingsPage => "AddSettingsPage",
            Self::MenuItem => "AddMenuItem",
            Self::MenubarItem => "AddMenubarItem",
            Self::LinkHandler => "AddLinkHandler",
            Self::TitlebarItem => "AddTitlebarItem",
            Self::ComposerPill => "AddComposerPill",
            Self::CodeBlock => "AddCodeBlock",
            Self::CodeBlockAction => "AddCodeBlockAction",
            Self::Slot => "AddSlot",
            Self::Theme => "AddTheme",
            Self::FileViewer => "AddFileViewer",
            Self::ServerSource => "AddServerSource",
            Self::Dialog => "AddDialog",
            Self::Popover => "AddPopover",
            Self::McpTool => "AddMcpTool",
        }
    }

    /// The spec record it takes.
    pub fn spec(self) -> &'static str {
        match self {
            Self::Command => "CommandSpec",
            Self::Panel => "PanelSpec",
            Self::Setting => "SettingSpec",
            Self::SettingsPage => "SettingsPageSpec",
            Self::MenuItem => "MenuItemSpec",
            Self::MenubarItem => "MenubarItemSpec",
            Self::LinkHandler => "LinkHandlerSpec",
            Self::TitlebarItem => "TitlebarItemSpec",
            Self::ComposerPill => "ComposerPillSpec",
            Self::CodeBlock => "CodeBlockSpec",
            Self::CodeBlockAction => "CodeBlockActionSpec",
            Self::Slot => "SlotSpec",
            Self::Theme => "ThemeSpec",
            Self::FileViewer => "FileViewerSpec",
            Self::ServerSource => "ServerSourceSpec",
            Self::Dialog => "DialogSpec",
            Self::Popover => "PopoverSpec",
            Self::McpTool => "McpToolSpec",
        }
    }

    /// The halves that may add it.
    pub fn halves(self) -> &'static [Half] {
        match self {
            Self::Command => &[Half::Window],
            Self::Panel => &[Half::Window],
            Self::Setting => &[Half::Window],
            Self::SettingsPage => &[Half::Window],
            Self::MenuItem => &[Half::Window],
            Self::MenubarItem => &[Half::App],
            Self::LinkHandler => &[Half::Window],
            Self::TitlebarItem => &[Half::Window],
            Self::ComposerPill => &[Half::Window],
            Self::CodeBlock => &[Half::Window],
            Self::CodeBlockAction => &[Half::Window],
            Self::Slot => &[Half::Window],
            Self::Theme => &[Half::Window],
            Self::FileViewer => &[Half::Window],
            Self::ServerSource => &[Half::Window, Half::App],
            Self::Dialog => &[Half::Window],
            Self::Popover => &[Half::Window],
            Self::McpTool => &[Half::Window, Half::App],
        }
    }
}

string_enum! {
    /// A host service, by the name `ctx:GetService` takes.
    pub enum ServiceName {
        /// The sessions the app holds.
        Sessions = "Sessions",
        /// The server's model catalog.
        Models = "Models",
        /// The OpenCode server this window is connected to. In the app half, the app's own server.
        Server = "Server",
        /// The servers the app lists.
        Servers = "Servers",
        /// HTTP requests off the UI thread.
        Http = "Http",
        /// Child processes the host supervises.
        Process = "Process",
        /// Values the host saves for this plugin. Both halves and every window share one copy.
        Store = "Store",
        /// Typed values plugins share. A plugin may provide only ids under its own id.
        Contracts = "Contracts",
        /// The Studio console. `print` and `warn` write here too.
        Console = "Console",
        /// Short messages in a corner of the window. From the app half, in the focused window.
        Toast = "Toast",
        /// This plugin's popovers. The host places them, moves focus and closes them.
        Popover = "Popover",
        /// This plugin's modal dialogs.
        Dialog = "Dialog",
        /// The app's own menus.
        Menu = "Menu",
        /// Panels, settings and drafts in this window.
        Layout = "Layout",
        /// Routes local links to the plugins that handle them.
        Links = "Links",
        /// Published commands, as the palette runs them.
        Commands = "Commands",
        /// The theme and the user's appearance settings.
        Theme = "Theme",
        /// The running app.
        App = "App",
        /// The clipboard, the system browser and file manager, and the clock.
        System = "System",
        /// System notifications that follow the user's notification settings.
        Notifications = "Notifications",
        /// Files in the workspace and the apps that open them.
        Workspace = "Workspace",
        /// Session changes for `ui.diffView`.
        Diff = "Diff",
        /// Terminal sessions on the server, for `ui.terminalSurface`.
        Terminal = "Terminal",
        /// Pages in the app's embedded browser. Pages belong to the process and survive a window closing when the app half opened them.
        Browser = "Browser",
        /// Office documents laid out and drawn by the host.
        Office = "Office",
        /// The app's own updates.
        Updater = "Updater",
        /// Developer diagnostics. Local and beta builds only; elsewhere every method returns nil.
        Diagnostics = "Diagnostics",
        /// Display sleep.
        Power = "Power",
        /// QR codes.
        Qr = "Qr",
        /// Credential prompts from child processes such as `ssh`.
        Askpass = "Askpass",
        /// Images handed to the host for `ui.image`.
        Images = "Images",
    }
}

impl ServiceName {
    /// The halves that may get it.
    pub fn halves(self) -> &'static [Half] {
        match self {
            Self::Sessions => &[Half::Window, Half::App],
            Self::Models => &[Half::Window, Half::App],
            Self::Server => &[Half::Window, Half::App],
            Self::Servers => &[Half::Window, Half::App],
            Self::Http => &[Half::Window, Half::App],
            Self::Process => &[Half::Window, Half::App],
            Self::Store => &[Half::Window, Half::App],
            Self::Contracts => &[Half::Window, Half::App],
            Self::Console => &[Half::Window, Half::App],
            Self::Toast => &[Half::Window, Half::App],
            Self::Popover => &[Half::Window],
            Self::Dialog => &[Half::Window],
            Self::Menu => &[Half::Window],
            Self::Layout => &[Half::Window],
            Self::Links => &[Half::Window],
            Self::Commands => &[Half::Window],
            Self::Theme => &[Half::Window, Half::App],
            Self::App => &[Half::Window, Half::App],
            Self::System => &[Half::Window, Half::App],
            Self::Notifications => &[Half::Window, Half::App],
            Self::Workspace => &[Half::Window, Half::App],
            Self::Diff => &[Half::Window],
            Self::Terminal => &[Half::Window, Half::App],
            Self::Browser => &[Half::Window, Half::App],
            Self::Office => &[Half::Window, Half::App],
            Self::Updater => &[Half::Window, Half::App],
            Self::Diagnostics => &[Half::Window, Half::App],
            Self::Power => &[Half::Window, Half::App],
            Self::Qr => &[Half::Window, Half::App],
            Self::Askpass => &[Half::App],
            Self::Images => &[Half::Window, Half::App],
        }
    }

    /// Its methods, signals and fields, by name.
    pub fn members(self) -> &'static [&'static str] {
        match self {
            Self::Sessions => &["Get", "Routed", "Open", "Changed", "RoutedChanged"],
            Self::Models => &["Get", "Changed"],
            Self::Server => &["RequestAsync", "Resource", "Invalidate", "OnEvent", "EncodeUri", "Version", "IsLocal", "CanWrite", "Connected"],
            Self::Servers => &["List", "Current", "Health", "Connect", "Changed"],
            Self::Http => &["FetchAsync"],
            Self::Process => &["Spawn", "RunAsync"],
            Self::Store => &["Value", "SessionValue"],
            Self::Contracts => &["Provide", "Use", "Summary"],
            Self::Console => &["Log", "Warn", "Error"],
            Self::Toast => &["Show", "Info", "Success", "Error"],
            Self::Popover => &["Open", "Toggle", "Close", "IsOpen"],
            Self::Dialog => &["Open", "Close", "IsOpen"],
            Self::Menu => &["OpenAsync"],
            Self::Layout => &["OpenPanel", "TogglePanel", "ClosePanel", "PanelState", "OpenSettings", "NewDraft", "Narrow"],
            Self::Links => &["Open", "Exists"],
            Self::Commands => &["Run", "Keybind"],
            Self::Theme => &["Current", "IsDark", "Resolve", "Font", "Changed"],
            Self::App => &["Version", "Channel", "Platform", "Locale", "Arg", "LogsDir", "DownloadsDir", "LocaleChanged"],
            Self::System => &["CopyText", "OpenExternal", "Reveal", "Now", "UtcOffset"],
            Self::Notifications => &["Notify"],
            Self::Workspace => &["ReadFileAsync", "SaveDownloadAsync", "OpenDocument", "OpenTree", "OpenInApps", "OpenIn", "FileChanged"],
            Self::Diff => &["Open"],
            Self::Terminal => &["Create", "Attach"],
            Self::Browser => &["Open", "Tabs", "Available"],
            Self::Office => &["OpenAsync"],
            Self::Updater => &["State", "Check", "Download", "Install", "StateChanged"],
            Self::Diagnostics => &["Memory", "Metrics", "Get", "Set", "HeapSnapshotAsync"],
            Self::Power => &["KeepAwake"],
            Self::Qr => &["Encode"],
            Self::Askpass => &["Listen"],
            Self::Images => &["FromEncoded", "FromSvg", "FromRgba", "Release"],
        }
    }
}

string_enum! {
    /// A signal on a plugin context.
    pub enum ContextEvent {
        /// Fires as the half goes away: the plugin turns off, reloads, or its window closes. The host disconnects every connection afterwards.
        Disposing = "Disposing",
        /// Fires as the window gains or loses focus.
        FocusChanged = "FocusChanged",
    }
}

impl ContextEvent {
    /// The halves whose context has it.
    pub fn halves(self) -> &'static [Half] {
        match self {
            Self::Disposing => &[Half::Window, Half::App],
            Self::FocusChanged => &[Half::Window],
        }
    }
}
