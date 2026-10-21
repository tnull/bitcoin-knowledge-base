/// SQL statements for creating the BKB database schema.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id              TEXT PRIMARY KEY,
    source_type     TEXT NOT NULL,
    source_repo     TEXT,
    source_id       TEXT NOT NULL,
    title           TEXT,
    body            TEXT,
    author          TEXT,
    author_id       TEXT,
    created_at      TIMESTAMP NOT NULL,
    updated_at      TIMESTAMP,
    parent_id       TEXT,
    metadata        TEXT,
    seq             INTEGER,
    UNIQUE(source_type, source_repo, source_id)
);

CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source_type, source_repo);
CREATE INDEX IF NOT EXISTS idx_documents_parent ON documents(parent_id);
CREATE INDEX IF NOT EXISTS idx_documents_author ON documents(author);
CREATE INDEX IF NOT EXISTS idx_documents_created ON documents(created_at);
CREATE INDEX IF NOT EXISTS idx_documents_seq ON documents(seq);

CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
    title,
    body,
    content=documents,
    content_rowid=rowid
);

CREATE TRIGGER IF NOT EXISTS documents_fts_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid, title, body)
    VALUES (NEW.rowid, NEW.title, NEW.body);
END;

CREATE TRIGGER IF NOT EXISTS documents_fts_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.body);
END;

CREATE TRIGGER IF NOT EXISTS documents_fts_update AFTER UPDATE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.body);
    INSERT INTO documents_fts(rowid, title, body)
    VALUES (NEW.rowid, NEW.title, NEW.body);
END;

CREATE TABLE IF NOT EXISTS refs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_doc_id TEXT NOT NULL,
    to_doc_id   TEXT,
    ref_type    TEXT NOT NULL,
    to_external TEXT,
    context     TEXT,
    FOREIGN KEY (from_doc_id) REFERENCES documents(id)
);

CREATE INDEX IF NOT EXISTS idx_refs_from ON refs(from_doc_id);
CREATE INDEX IF NOT EXISTS idx_refs_to ON refs(to_doc_id);
CREATE INDEX IF NOT EXISTS idx_refs_to_ext ON refs(to_external);
CREATE INDEX IF NOT EXISTS idx_refs_type ON refs(ref_type);

CREATE TABLE IF NOT EXISTS concepts (
    slug        TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    category    TEXT,
    aliases     TEXT
);

CREATE TABLE IF NOT EXISTS concept_mentions (
    doc_id       TEXT NOT NULL,
    concept_slug TEXT NOT NULL,
    confidence   REAL DEFAULT 1.0,
    PRIMARY KEY (doc_id, concept_slug),
    FOREIGN KEY (doc_id) REFERENCES documents(id),
    FOREIGN KEY (concept_slug) REFERENCES concepts(slug)
);

CREATE INDEX IF NOT EXISTS idx_concept_mentions_concept ON concept_mentions(concept_slug);

CREATE TABLE IF NOT EXISTS change_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id      TEXT NOT NULL,
    change_type TEXT NOT NULL,
    changed_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sync_state (
    source_id      TEXT PRIMARY KEY,
    source_type    TEXT NOT NULL,
    source_repo    TEXT,
    last_cursor    TEXT,
    last_synced_at TIMESTAMP,
    next_run_at    TIMESTAMP,
    status         TEXT DEFAULT 'pending',
    error_message  TEXT,
    retry_count    INTEGER DEFAULT 0,
    items_found    INTEGER DEFAULT 0
);
"#;
