//! The engine-agnostic OpenCode plugin contract.
//!
//! `contract.json` is the single source of truth. `cargo run -p opencode-plugin-spec -- --write`
//! generates from it:
//!
//! - `plugins/opencode.d.luau`, `plugins/types/*.luau` and `plugins/.luaurc` for `luau-lsp`,
//! - `plugins/plugin.schema.json` for plugin.toml,
//! - `plugins/opencode-selene.yml` for `selene`,
//! - `docs/api.md`,
//! - this crate's generated types: semantic tokens, enums, records, the `ui.*` node
//!   vocabulary and the registry of contributions, services and events.
//!
//! Nothing here knows Rune, a Luau runtime or the app: a host decodes plugin values with serde
//! and the node tables, and maps tokens and nodes to its own engine.

mod color;
mod generated;
mod macros;
mod props;
mod version;

pub use color::ColorSpec;
pub use generated::*;
pub use macros::Record;
pub use props::{PropSpec, PropType, PropsSpec};
pub use version::{Compat, ContractVersion};

/// The files an author's editor needs, by path relative to a plugins folder, for
/// `opencode-desktop --write-plugin-types <dir>`.
pub const AUTHOR_FILES: &[(&str, &str)] = &[
    (".luaurc", include_str!("../../../plugins/.luaurc")),
    ("opencode.d.luau", include_str!("../../../plugins/opencode.d.luau")),
    ("types/ui.luau", include_str!("../../../plugins/types/ui.luau")),
    ("types/plugin.luau", include_str!("../../../plugins/types/plugin.luau")),
    ("plugin.schema.json", include_str!("../../../plugins/plugin.schema.json")),
    ("opencode-selene.yml", include_str!("../../../plugins/opencode-selene.yml")),
];
