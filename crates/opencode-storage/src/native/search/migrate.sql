-- Executed with the migration record and user_version in one writer transaction.
-- Preserve the logical NOT NULL constraint of the old WITHOUT ROWID primary key.
CREATE TABLE document_v2 (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
INSERT INTO document_v2(key,value) SELECT key,value FROM document;
DROP TABLE document;
ALTER TABLE document_v2 RENAME TO document;
CREATE INDEX blob_by_touch ON blob(touched_at);

CREATE TABLE search_meta (
    id INTEGER PRIMARY KEY,
    server TEXT NOT NULL, session_id TEXT NOT NULL, parent_id TEXT, project_dir TEXT NOT NULL,
    message_id TEXT NOT NULL, kind TEXT NOT NULL, archived INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL, title TEXT NOT NULL, snippet_zstd BLOB NOT NULL,
    UNIQUE (server, session_id, message_id, kind)
);
CREATE VIRTUAL TABLE search_fts USING fts5(
    title, body, content='', contentless_delete=1, tokenize='opencode_tok', prefix='2 3'
);

-- Queue keys, not bodies. The migration does not tokenize history or decompress copies.
INSERT OR IGNORE INTO state(name,key,value,updated_at)
    SELECT 'rune.search.dirty','d'||char(10)||key,'1',0 FROM document WHERE key GLOB 'opencode.window.*.dat:draft:*';
INSERT OR IGNORE INTO state(name,key,value,updated_at)
    SELECT 'rune.search.dirty','o'||char(10)||server||char(10)||id,'1',updated_at FROM outbox;
INSERT OR IGNORE INTO state(name,key,value,updated_at)
    SELECT 'rune.search.dirty',key,'1',touched_at FROM cache WHERE key GLOB ('rune.copy'||char(10)||'*');
