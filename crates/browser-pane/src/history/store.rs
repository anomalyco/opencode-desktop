//! Persistent SQLite browsing history store (`history.sqlite`) matching upstream
//! `packages/gui-plugins/src/browser/history/store.ts`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use super::page::{FoundPage, MAX_PAGES, PageVisit, page_key, page_title};
use super::search::PageIndex;

const FILE_NAME: &str = "history.sqlite";
const PRUNE_ROOM: usize = 500;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Persistent SQLite history store holding up to [`MAX_PAGES`] web pages with lazy in-memory
/// frecency indexing and `PRAGMA secure_delete = ON`.
pub struct HistoryStore {
    conn: Connection,
    path: PathBuf,
    index: Option<PageIndex>,
}

impl HistoryStore {
    /// Opens or initializes `<directory>/history.sqlite`.
    pub fn open(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|e| format!("create history dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
        }
        let path = directory.join(FILE_NAME);
        let conn = Connection::open(&path).map_err(|e| format!("open history db: {e}"))?;
        Self::init(conn, path)
    }

    /// Opens an in-memory history database (used by unattended fixture runs).
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("open memory history db: {e}"))?;
        Self::init(conn, PathBuf::new())
    }

    fn init(conn: Connection, path: PathBuf) -> Result<Self, String> {
        let schema = "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA secure_delete = ON;
            CREATE TABLE IF NOT EXISTS pages (
                id INTEGER PRIMARY KEY,
                key TEXT NOT NULL UNIQUE,
                url TEXT NOT NULL,
                title TEXT NOT NULL,
                visits INTEGER NOT NULL,
                typed INTEGER NOT NULL,
                visited INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) WITHOUT ROWID;
        ";
        conn.execute_batch(schema).map_err(|e| format!("init history schema: {e}"))?;
        Ok(Self { conn, path, index: None })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the in-memory index ahead of the first search and prunes if above [`MAX_PAGES`].
    pub fn prepare(&mut self) {
        if self.index.is_none() {
            let idx = self.ensure_index();
            if idx.size() > MAX_PAGES {
                self.prune(PRUNE_ROOM);
            }
        }
    }

    /// Returns how many web pages are currently stored.
    pub fn count(&self) -> usize {
        if let Some(idx) = &self.index {
            return idx.size();
        }
        self.conn
            .query_row("SELECT count(*) FROM pages", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as usize)
            .unwrap_or(0)
    }

    /// Searches stored history for `text`, returning up to `limit` frecency-ranked pages.
    pub fn search(&mut self, text: &str, limit: usize) -> Vec<FoundPage> {
        self.prepare();
        let now = now_ms();
        let Some(idx) = self.index.as_ref() else {
            return Vec::new();
        };
        let conn = &self.conn;
        idx.find_pages(
            text,
            limit,
            now,
            |id| {
                conn.query_row("SELECT url, title FROM pages WHERE id = ?1", [id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .optional()
                .ok()
                .flatten()
            },
            |host| {
                conn.query_row("SELECT url, title FROM pages WHERE key = ?1", [host], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .optional()
                .ok()
                .flatten()
            },
        )
    }

    /// Records a visit to `url`. Non-web/preview/credentialed URLs are silently ignored.
    pub fn visit(&mut self, url: &str, title: &str, typed: bool) {
        let Some(key) = page_key(url) else { return };
        let clean_title = page_title(title);
        let typed_inc: i64 = if typed { 1 } else { 0 };
        let now = now_ms();
        let row = self
            .conn
            .query_row(
                "INSERT INTO pages (key, url, title, visits, typed, visited) VALUES (?1, ?2, ?3, 1, ?4, ?5)
                 ON CONFLICT (key) DO UPDATE SET
                   url = excluded.url,
                   title = CASE WHEN excluded.title = '' THEN title ELSE excluded.title END,
                   visits = visits + 1,
                   typed = typed + excluded.typed,
                   visited = excluded.visited
                 RETURNING id, url, title, visits, typed, visited",
                params![key, url, clean_title, typed_inc, now],
                row_to_visit,
            )
            .ok();
        if let Some((id, visit)) = row
            && let Some(idx) = self.index.as_mut()
        {
            idx.set(id, &visit);
            if idx.size() > MAX_PAGES {
                self.prune(PRUNE_ROOM);
            }
        }
    }

    /// Updates a stored page's title without incrementing visit counts.
    pub fn retitle(&mut self, url: &str, title: &str) {
        let Some(key) = page_key(url) else { return };
        let next = page_title(title);
        if next.is_empty() {
            return;
        }
        let existing = self
            .conn
            .query_row(
                "SELECT id, url, title, visits, typed, visited FROM pages WHERE key = ?1",
                [&key],
                row_to_visit,
            )
            .optional()
            .ok()
            .flatten();
        let Some((id, mut visit)) = existing else { return };
        if visit.title == next {
            return;
        }
        if self.conn.execute("UPDATE pages SET title = ?1 WHERE id = ?2", params![&next, id]).is_ok() {
            visit.title = next;
            if let Some(idx) = self.index.as_mut() {
                idx.set(id, &visit);
            }
        }
    }

    /// Merges imported pages from another browser (idempotent: takes `max(visits)`, `max(typed)`,
    /// `max(visited)`). Returns how many distinct web pages were imported.
    pub fn add(&mut self, pages: &[PageVisit]) -> Result<usize, String> {
        let now = now_ms();
        let mut distinct: HashMap<String, PageVisit> = HashMap::new();
        for p in pages {
            let Some(key) = page_key(&p.url) else { continue };
            if p.visits < 1 {
                continue;
            }
            let next = PageVisit {
                url: p.url.clone(),
                title: page_title(&p.title),
                visits: p.visits,
                typed: p.typed.max(0).min(p.visits),
                visited: p.visited.clamp(0, now),
            };
            distinct
                .entry(key)
                .and_modify(|k| {
                    let use_next = next.visited >= k.visited;
                    if use_next {
                        k.url = next.url.clone();
                        if !next.title.is_empty() {
                            k.title = next.title.clone();
                        }
                    } else if k.title.is_empty() && !next.title.is_empty() {
                        k.title = next.title.clone();
                    }
                    k.visits = k.visits.saturating_add(next.visits);
                    k.typed = k.typed.saturating_add(next.typed);
                    k.visited = k.visited.max(next.visited);
                })
                .or_insert(next);
        }
        let imported = distinct.len();
        let tx = self.conn.transaction().map_err(|e| format!("begin import tx: {e}"))?;
        let mut updated: Vec<(i64, PageVisit)> = Vec::with_capacity(imported);
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO pages (key, url, title, visits, typed, visited) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (key) DO UPDATE SET
                       url = CASE WHEN excluded.visited >= visited THEN excluded.url ELSE url END,
                       title = CASE WHEN excluded.title != '' AND (excluded.visited >= visited OR title = '') THEN excluded.title ELSE title END,
                       visits = max(visits, excluded.visits),
                       typed = max(typed, excluded.typed),
                       visited = max(visited, excluded.visited)
                     RETURNING id, url, title, visits, typed, visited",
                )
                .map_err(|e| format!("prepare merge: {e}"))?;
            for (key, page) in distinct {
                if let Ok(row) = stmt.query_row(
                    params![key, page.url, page.title, page.visits, page.typed, page.visited],
                    row_to_visit,
                ) {
                    updated.push(row);
                }
            }
        }
        tx.commit().map_err(|e| format!("commit import tx: {e}"))?;
        if let Some(idx) = self.index.as_mut() {
            for (id, visit) in updated {
                idx.set(id, &visit);
            }
        }
        if self.count() > MAX_PAGES {
            self.prune(0);
        }
        Ok(imported)
    }

    /// One-time migration of legacy `plugin.browser/history` JSON visits into `history.sqlite`.
    pub fn adopt(&mut self, legacy: &[(String, String)]) {
        let done = self
            .conn
            .query_row("SELECT 1 FROM meta WHERE key = 'adopted'", [], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .is_some();
        if done {
            return;
        }
        let now = now_ms();
        let Ok(tx) = self.conn.transaction() else { return };
        let mut added: Vec<(i64, PageVisit)> = Vec::new();
        if let Ok(mut stmt) = tx.prepare(
            "INSERT INTO pages (key, url, title, visits, typed, visited) VALUES (?1, ?2, ?3, 1, 0, ?4)
             ON CONFLICT (key) DO NOTHING
             RETURNING id, url, title, visits, typed, visited",
        ) {
            for (pos, (url, title)) in legacy.iter().enumerate() {
                if let Some(key) = page_key(url)
                    && let Ok(Some(row)) = stmt
                        .query_row(params![key, url, page_title(title), now - pos as i64], row_to_visit)
                        .optional()
                {
                    added.push(row);
                }
            }
        }
        let _ = tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('adopted', '1')", []);
        if tx.commit().is_ok()
            && let Some(idx) = self.index.as_mut()
        {
            for (id, visit) in added {
                idx.set(id, &visit);
            }
        }
    }

    /// Removes one page from history (e.g. Shift+Delete in the address bar).
    pub fn remove(&mut self, url: &str) -> bool {
        let Some(key) = page_key(url) else { return false };
        let id = self
            .conn
            .query_row("SELECT id FROM pages WHERE key = ?1", [&key], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten();
        let Some(id) = id else { return false };
        if self.conn.execute("DELETE FROM pages WHERE id = ?1", [id]).is_ok() {
            if let Some(idx) = self.index.as_mut() {
                idx.remove(id);
            }
            return true;
        }
        false
    }

    /// Deletes every history row and reclaims freed pages (`PRAGMA secure_delete = ON` + `VACUUM`).
    pub fn clear(&mut self) -> Result<(), String> {
        self.conn.execute("DELETE FROM pages", []).map_err(|e| format!("clear history: {e}"))?;
        let _ = self.conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);");
        if let Some(idx) = self.index.as_mut() {
            idx.clear();
        }
        Ok(())
    }

    fn ensure_index(&mut self) -> &mut PageIndex {
        if self.index.is_none() {
            let mut idx = PageIndex::default();
            if let Ok(mut stmt) = self.conn.prepare("SELECT id, url, title, visits, typed, visited FROM pages")
                && let Ok(rows) = stmt.query_map([], row_to_visit)
            {
                for (id, visit) in rows.flatten() {
                    idx.set(id, &visit);
                }
            }
            self.index = Some(idx);
        }
        self.index.as_mut().expect("index initialized")
    }

    fn prune(&mut self, room: usize) {
        let keep = MAX_PAGES.saturating_sub(room);
        let now = now_ms();
        let drop_ids = self.ensure_index().least(keep, now);
        if drop_ids.is_empty() {
            return;
        }
        if let Ok(tx) = self.conn.transaction() {
            if let Ok(mut stmt) = tx.prepare("DELETE FROM pages WHERE id = ?1") {
                for id in &drop_ids {
                    let _ = stmt.execute([id]);
                }
            }
            if tx.commit().is_ok()
                && let Some(idx) = self.index.as_mut()
            {
                for id in drop_ids {
                    idx.remove(id);
                }
            }
        }
    }
}

fn row_to_visit(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, PageVisit)> {
    Ok((
        row.get(0)?,
        PageVisit {
            url: row.get(1)?,
            title: row.get(2)?,
            visits: row.get(3)?,
            typed: row.get(4)?,
            visited: row.get(5)?,
        },
    ))
}
