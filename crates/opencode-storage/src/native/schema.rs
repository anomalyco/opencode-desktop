//! The schema: Desktop's tables where the data is the same, under Desktop's own migration ids
//! and statements (`packages/desktop/src/main/storage/migration.gen.ts:3-39`, `schema.ts:4-40`),
//! then Rune's own (`plugin_trust`, `window_state`, `outbox`).
//!
//! The journal (`migration`, `migration.ts:5-33`) records every applied id, as Desktop's does.
//! Whether anything is pending is answered without reading it: the header's `user_version`
//! holds [`SCHEMA_VERSION`] once the journal is complete, so a steady-state open reads one
//! header field (already in memory once the file is open) and parses no schema it doesn't use.

use rusqlite::Connection;

/// Bumped with every migration added below.
pub const SCHEMA_VERSION: u32 = 9;

/// `user_version` = `SCHEMA_VERSION << 8 | flags`.
pub const FLAG_LEGACY_SETTLED: u32 = 1;
pub const FLAG_SEED_SETTLED: u32 = 2;

/// `(id, statements)`, applied in order inside one transaction.
pub const MIGRATIONS: &[(&str, &[&str])] = &[
    ("20260906234153_drafts", &["CREATE TABLE `blob` (\n\t`id` text PRIMARY KEY,\n\t`data` blob NOT NULL\n);", "CREATE TABLE `document` (\n\t`key` text PRIMARY KEY,\n\t`value` text NOT NULL\n);"]),
    ("20260906234220_state", &["CREATE TABLE `state` (\n\t`name` text NOT NULL,\n\t`key` text NOT NULL,\n\t`value` text NOT NULL,\n\t`updated_at` integer NOT NULL,\n\tCONSTRAINT `state_pk` PRIMARY KEY(`name`, `key`)\n);"]),
    ("20260907031611_blob-touched", &["ALTER TABLE `blob` ADD `touched_at` integer DEFAULT 0 NOT NULL;"]),
    (
        "20260930041551_extension",
        &[
            "CREATE TABLE `extension` (\n\t`id` text PRIMARY KEY,\n\t`enabled` integer DEFAULT true NOT NULL,\n\t`manifest` text,\n\t`revision` text\n);",
            "CREATE TABLE `extension_file` (\n\t`extension_id` text NOT NULL,\n\t`path` text NOT NULL,\n\t`data` blob NOT NULL,\n\tCONSTRAINT `extension_file_pk` PRIMARY KEY(`extension_id`, `path`)\n);",
        ],
    ),
    ("20261013231314_drop-extension-files", &["DROP TABLE `extension_file`;"]),
    ("20261014210000_rename-extension-to-plugin", &["ALTER TABLE `extension` RENAME TO `plugin`;", "CREATE VIEW `extension` AS SELECT `id`, `enabled`, `manifest`, `revision` FROM `plugin`;"]),
    // Rune: third-party Luau plugin library trust (Tier 0), was `plugins-state.json` `trusted`.
    ("20261111000001_rune-plugin-trust", &["CREATE TABLE `plugin_trust` (\n\t`id` text PRIMARY KEY,\n\t`sha256` text NOT NULL,\n\t`trusted_at` integer NOT NULL\n);"]),
    // Rune: each window's frame, was `window-state-<id>.json`.
    (
        "20261111000002_rune-window-state",
        &["CREATE TABLE `window_state` (\n\t`id` text PRIMARY KEY,\n\t`x` integer,\n\t`y` integer,\n\t`width` integer NOT NULL,\n\t`height` integer NOT NULL,\n\t`maximized` integer NOT NULL DEFAULT 0,\n\t`full_screen` integer NOT NULL DEFAULT 0,\n\t`display_x` integer,\n\t`display_y` integer,\n\t`display_w` integer,\n\t`display_h` integer,\n\t`updated_at` integer NOT NULL\n);"],
    ),
    // Rune: the per-server prompt outbox (Tier 0), was `outbox-<sha256>.json`.
    (
        "20261111000003_rune-outbox",
        &[
            "CREATE TABLE `outbox` (\n\t`server` text NOT NULL,\n\t`id` text NOT NULL,\n\t`seq` integer NOT NULL,\n\t`session` text NOT NULL,\n\t`state` text NOT NULL,\n\t`operation_json` text NOT NULL,\n\t`draft_json` text NOT NULL,\n\t`prompt_json` text,\n\t`attachment_blobs_json` text NOT NULL DEFAULT '[]',\n\t`paths_json` text NOT NULL DEFAULT '[]',\n\t`inbox` integer NOT NULL DEFAULT 0,\n\t`why` text NOT NULL DEFAULT '',\n\t`after_check` text,\n\t`restored` integer NOT NULL DEFAULT 0,\n\t`durable` integer NOT NULL DEFAULT 1,\n\t`legacy_json` text,\n\t`updated_at` integer NOT NULL,\n\tCONSTRAINT `outbox_pk` PRIMARY KEY(`server`, `id`)\n);",
            "CREATE INDEX `outbox_server_seq` ON `outbox` (`server`, `seq`);",
        ],
    ),
];

/// Applies what the journal lacks, in one transaction (`migration.ts:5-33`). A database Desktop
/// made before its journal (a `document` table and no journal rows) keeps its first tables.
pub fn migrate(conn: &mut Connection, now_ms: i64) -> rusqlite::Result<Vec<&'static str>> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL)")?;
    let applied: Vec<String> = conn.prepare("SELECT id FROM migration")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let legacy = applied.is_empty() && conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'document'", [], |_| Ok(())).is_ok();
    let pending: Vec<&(&str, &[&str])> = MIGRATIONS.iter().filter(|(id, _)| !applied.iter().any(|a| a == id)).collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let tx = conn.transaction()?;
    for (index, (id, statements)) in pending.iter().enumerate() {
        if !(legacy && index == 0) {
            for s in statements.iter() {
                tx.execute_batch(s)?;
            }
        }
        tx.execute("INSERT INTO migration (id, time_completed) VALUES (?1, ?2)", (id, now_ms))?;
    }
    tx.commit()?;
    Ok(pending.iter().map(|(id, _)| *id).collect())
}

pub fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map(|v| u32::try_from(v).unwrap_or(0))
}

pub fn set_flags(conn: &Connection, flags: u32) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {}", (SCHEMA_VERSION << 8) | flags))
}
