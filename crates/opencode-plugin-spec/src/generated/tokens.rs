//! The semantic token palette.
//! Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.

use crate::string_enum;

string_enum! {
    /// A theme color by meaning. The host maps each token to its theme once per theme change,
    /// and decodes one with a single `match` (`SemanticToken::parse`).
    pub enum SemanticToken {
        /// The window's base background.
        BgBase = "bg-base",
        /// A background one step quieter than `bg-base`, such as a sidebar.
        BgWeak = "bg-weak",
        /// A background one step stronger than `bg-base`, such as a title bar.
        BgStrong = "bg-strong",
        /// The first raised layer: cards and grouped lists.
        Surface1 = "surface-1",
        /// The second raised layer: controls on a card.
        Surface2 = "surface-2",
        /// The third raised layer: a control's track, such as an off switch.
        Surface3 = "surface-3",
        /// The fourth raised layer: the strongest fill a control uses.
        Surface4 = "surface-4",
        /// The fill of a row or button under the pointer.
        SurfaceHover = "surface-hover",
        /// The fill of a row or button while it is pressed.
        SurfacePress = "surface-press",
        /// The fill of a selected row or tab.
        SurfaceSelected = "surface-selected",
        /// Headings and the most important text.
        TextStrong = "text-strong",
        /// Body text.
        TextBase = "text-base",
        /// Secondary text: descriptions and labels.
        TextWeak = "text-weak",
        /// Tertiary text: hints, placeholders and timestamps.
        TextWeaker = "text-weaker",
        /// Text on an `accent` fill.
        TextOnAccent = "text-on-accent",
        /// Links and other interactive text.
        TextInteractive = "text-interactive",
        /// An icon that needs emphasis.
        IconStrong = "icon-strong",
        /// The default icon color.
        IconBase = "icon-base",
        /// A secondary icon.
        IconWeak = "icon-weak",
        /// A decorative or disabled icon.
        IconWeaker = "icon-weaker",
        /// An icon on an `accent` fill.
        IconOnAccent = "icon-on-accent",
        /// A strong border, such as a focused field's outline.
        BorderStrong = "border-strong",
        /// The default border.
        BorderBase = "border-base",
        /// A hairline divider or a card's border.
        BorderWeak = "border-weak",
        /// The keyboard focus ring.
        BorderFocus = "border-focus",
        /// The brand accent: primary buttons and switches that are on.
        Accent = "accent",
        /// An `accent` fill under the pointer.
        AccentHover = "accent-hover",
        /// A quiet accent tint, such as a selected chip's fill.
        AccentSubtle = "accent-subtle",
        /// Done or passing.
        StatusSuccess = "status-success",
        /// Needs attention.
        StatusWarning = "status-warning",
        /// Failed, blocked or destructive.
        StatusDanger = "status-danger",
        /// Neutral information.
        StatusInfo = "status-info",
    }
}

impl SemanticToken {
    /// The token's group, such as `surface` or `status`.
    pub fn group(self) -> &'static str {
        match self {
            Self::Accent | Self::AccentHover | Self::AccentSubtle => "accent",
            Self::BgBase | Self::BgWeak | Self::BgStrong => "background",
            Self::BorderStrong | Self::BorderBase | Self::BorderWeak | Self::BorderFocus => "border",
            Self::IconStrong | Self::IconBase | Self::IconWeak | Self::IconWeaker | Self::IconOnAccent => "icon",
            Self::StatusSuccess | Self::StatusWarning | Self::StatusDanger | Self::StatusInfo => "status",
            Self::Surface1 | Self::Surface2 | Self::Surface3 | Self::Surface4 | Self::SurfaceHover | Self::SurfacePress | Self::SurfaceSelected => "surface",
            Self::TextStrong | Self::TextBase | Self::TextWeak | Self::TextWeaker | Self::TextOnAccent | Self::TextInteractive => "text",
        }
    }
}
