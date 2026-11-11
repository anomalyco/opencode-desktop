//! The plugin tables in SQLite.

use rusqlite::{Connection, params};

use super::PluginRecord;

/// Plugin `id`'s record, both rows, inside the caller's transaction.
pub(crate) fn write(conn: &Connection, id: &str, record: &PluginRecord, now_ms: i64) -> rusqlite::Result<()> {
    match record.enabled {
        Some(on) => conn.prepare_cached("INSERT INTO plugin (id, enabled) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled")?.execute(params![id, on])?,
        None => conn.prepare_cached("DELETE FROM plugin WHERE id = ?1")?.execute([id])?,
    };
    match &record.trusted {
        Some(sha) => conn
            .prepare_cached("INSERT INTO plugin_trust (id, sha256, trusted_at) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET sha256 = excluded.sha256, trusted_at = excluded.trusted_at")?
            .execute(params![id, sha, now_ms])?,
        None => conn.prepare_cached("DELETE FROM plugin_trust WHERE id = ?1")?.execute([id])?,
    };
    Ok(())
}

/// The plugin values the boot record may carry: every global value and each shown session's,
/// read by key from the plugin namespaces (`plugin.` .. `plugin/`), none of the others' values.
pub(crate) fn slice_rows(conn: &Connection, shown: &[&str]) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut rows: Vec<(String, String, String)> = conn
        .prepare_cached("SELECT name, key, value FROM state WHERE name > 'plugin.' AND name < 'plugin/' AND instr(key, '@') = 0")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for session in shown {
        let mut stmt = conn.prepare_cached(
            "SELECT name, key, value FROM state WHERE name > 'plugin.' AND name < 'plugin/' AND instr(key, '@') > 0 \
             AND (substr(key, -length(?1) - 1) = '/' || ?1 OR substr(key, -length(?1) - 1) = '@' || ?1)",
        )?;
        let found = stmt.query_map([session], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for row in found {
            rows.push(row?);
        }
    }
    rows.retain(|(name, key, _)| super::wanted(name, key, shown));
    Ok(rows)
}
