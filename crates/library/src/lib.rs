//! `snapwright-library` — every capture, kept and searchable.
//!
//! * Images live in a content-addressed store: `store/ab/abcdef….png`, named by the BLAKE3
//!   hash of the PNG bytes, so the same image is stored once.
//! * Thumbnails (at most [`THUMB_MAX`] pixels on the long side) live in `thumbs/`.
//! * Metadata, tags and a full-text index (SQLite FTS5) live in `library.sqlite3`.
//!
//! Deleting moves a capture to the trash; [`Library::purge_trash`] removes it for good, along
//! with its files once no other capture shares them.

mod search;
mod store;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension as _, Row, params};

pub use search::fts_query;

/// Longest side of a thumbnail, in pixels.
pub const THUMB_MAX: u32 = 320;

/// Errors from the library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("image: {0}")]
    Image(String),
    #[error("no capture with id {0}")]
    NotFound(i64),
    #[error("library schema version {0} is newer than this build understands")]
    TooNew(i64),
}

/// Shorthand for library results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How a capture was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Region,
    Window,
    Display,
    Import,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Window => "window",
            Self::Display => "display",
            Self::Import => "import",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "window" => Self::Window,
            "display" => Self::Display,
            "import" => Self::Import,
            _ => Self::Region,
        }
    }
}

/// What was on screen when a capture was taken. Every field is optional context for search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub source: Source,
    /// Application that owned the captured window (or the window under the region).
    pub app: String,
    pub window_title: String,
    /// Page address, when the window was a browser that reported one.
    pub url: String,
    /// Display the capture came from.
    pub display_id: Option<u64>,
}

/// A capture to add: its PNG encoding and the RGBA pixels it was encoded from.
pub struct NewCapture<'a> {
    pub png: &'a [u8],
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub meta: Metadata,
}

/// A stored capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub id: i64,
    /// BLAKE3 of the PNG bytes, hex.
    pub hash: String,
    /// Milliseconds since the Unix epoch.
    pub created_ms: i64,
    pub width: u32,
    pub height: u32,
    /// PNG size in bytes.
    pub bytes: u64,
    pub meta: Metadata,
    pub note: String,
    pub tags: Vec<String>,
    /// Text recognised in the image (filled by OCR later).
    pub text: String,
    pub trashed: bool,
}

/// Which captures to list.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Free text matched against app, title, URL, note, tags and recognised text. Each word
    /// must match the start of a word, in any field.
    pub text: Option<String>,
    /// Only captures with this tag.
    pub tag: Option<String>,
    /// Only captures from this app.
    pub app: Option<String>,
    /// Only captures taken at or after this time (ms since epoch).
    pub since_ms: Option<i64>,
    /// List the trash instead of the library.
    pub trash: bool,
    /// Page size; 0 means no limit.
    pub limit: usize,
    pub offset: usize,
}

/// An open capture library.
pub struct Library {
    root: PathBuf,
    db: Connection,
}

const SCHEMA_VERSION: i64 = 1;

impl Library {
    /// Opens (creating if needed) the library in `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        for dir in [root.clone(), root.join("store"), root.join("thumbs")] {
            std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        }
        let db = Connection::open(root.join("library.sqlite3"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&db)?;
        Ok(Self { root, db })
    }

    /// The folder this library lives in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stores a capture: writes the image (once per distinct image) and its thumbnail, then
    /// records it. Returns the new capture's id.
    pub fn add(&mut self, new: &NewCapture<'_>) -> Result<i64> {
        self.add_at(new, now_ms())
    }

    fn add_at(&mut self, new: &NewCapture<'_>, created_ms: i64) -> Result<i64> {
        let hash = blake3::hash(new.png).to_hex().to_string();
        store::write_once(&self.image_path(&hash), new.png)?;
        let thumb_path = self.thumb_path(&hash);
        if !thumb_path.exists() {
            let thumb = store::thumbnail(new.rgba, new.width, new.height, THUMB_MAX)?;
            store::write_once(&thumb_path, &thumb)?;
        }
        let m = &new.meta;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO captures
                (hash, created_ms, width, height, bytes, source, app, window_title, url, display_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                hash,
                created_ms,
                new.width,
                new.height,
                new.png.len() as i64,
                m.source.as_str(),
                m.app,
                m.window_title,
                m.url,
                m.display_id.map(|d| d as i64),
            ],
        )?;
        let id = tx.last_insert_rowid();
        reindex(&tx, id)?;
        tx.commit()?;
        Ok(id)
    }

    /// One capture by id, trashed or not.
    pub fn get(&self, id: i64) -> Result<Capture> {
        let capture = self
            .db
            .query_row(
                &format!("SELECT {COLUMNS} FROM captures c WHERE c.id = ?1"),
                [id],
                capture_from_row,
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;
        self.with_tags(capture)
    }

    /// Captures matching `query`, newest first.
    pub fn list(&self, query: &Query) -> Result<Vec<Capture>> {
        let mut sql = format!("SELECT {COLUMNS} FROM captures c");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut filters = vec![if query.trash {
            "c.deleted_ms IS NOT NULL".to_owned()
        } else {
            "c.deleted_ms IS NULL".to_owned()
        }];
        if let Some(fts) = query.text.as_deref().and_then(fts_query) {
            sql.push_str(" JOIN captures_fts f ON f.rowid = c.id");
            filters.push("captures_fts MATCH ?".into());
            args.push(fts.into());
        }
        if let Some(tag) = &query.tag {
            filters.push(
                "EXISTS (SELECT 1 FROM capture_tags ct JOIN tags t ON t.id = ct.tag_id
                         WHERE ct.capture_id = c.id AND t.name = ? COLLATE NOCASE)"
                    .into(),
            );
            args.push(tag.clone().into());
        }
        if let Some(app) = &query.app {
            filters.push("c.app = ? COLLATE NOCASE".into());
            args.push(app.clone().into());
        }
        if let Some(since) = query.since_ms {
            filters.push("c.created_ms >= ?".into());
            args.push(since.into());
        }
        sql.push_str(" WHERE ");
        sql.push_str(&filters.join(" AND "));
        sql.push_str(" ORDER BY c.created_ms DESC, c.id DESC");
        if query.limit > 0 {
            sql.push_str(&format!(" LIMIT {} OFFSET {}", query.limit, query.offset));
        }
        let mut stmt = self.db.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), capture_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(self.with_tags(row?)?);
        }
        Ok(out)
    }

    /// How many captures match `query` (ignoring its paging).
    pub fn count(&self, query: &Query) -> Result<usize> {
        let all = Query {
            limit: 0,
            offset: 0,
            ..query.clone()
        };
        // Counting through `list` keeps one filter implementation; libraries hold thousands of
        // rows, not millions, and the UI asks once per search.
        Ok(self.list(&all)?.len())
    }

    /// Adds a tag (created if new). Tag names are matched case-insensitively.
    pub fn tag(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        self.get(id)?;
        let tx = self.db.transaction()?;
        tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [name])?;
        tx.execute(
            "INSERT OR IGNORE INTO capture_tags (capture_id, tag_id)
             SELECT ?1, id FROM tags WHERE name = ?2 COLLATE NOCASE",
            params![id, name],
        )?;
        reindex(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a tag from a capture.
    pub fn untag(&mut self, id: i64, name: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "DELETE FROM capture_tags WHERE capture_id = ?1
               AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
            params![id, name.trim()],
        )?;
        reindex(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Every tag in use, with how many captures carry it, most used first.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.db.prepare(
            "SELECT t.name, COUNT(*) FROM tags t
               JOIN capture_tags ct ON ct.tag_id = t.id
               JOIN captures c ON c.id = ct.capture_id AND c.deleted_ms IS NULL
             GROUP BY t.id ORDER BY COUNT(*) DESC, t.name",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Sets the free-text note.
    pub fn set_note(&mut self, id: i64, note: &str) -> Result<()> {
        self.update(id, "UPDATE captures SET note = ?2 WHERE id = ?1", note)
    }

    /// Sets the recognised text (from OCR).
    pub fn set_text(&mut self, id: i64, text: &str) -> Result<()> {
        self.update(id, "UPDATE captures SET text = ?2 WHERE id = ?1", text)
    }

    fn update(&mut self, id: i64, sql: &str, value: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        if tx.execute(sql, params![id, value])? == 0 {
            return Err(Error::NotFound(id));
        }
        reindex(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Moves a capture to the trash.
    pub fn trash(&mut self, id: i64) -> Result<()> {
        let n = self.db.execute(
            "UPDATE captures SET deleted_ms = ?2 WHERE id = ?1 AND deleted_ms IS NULL",
            params![id, now_ms()],
        )?;
        if n == 0 {
            self.get(id)?;
        }
        Ok(())
    }

    /// Takes a capture back out of the trash.
    pub fn restore(&mut self, id: i64) -> Result<()> {
        if self
            .db
            .execute("UPDATE captures SET deleted_ms = NULL WHERE id = ?1", [id])?
            == 0
        {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }

    /// Deletes trashed captures for good, and their files once nothing else uses them.
    /// Returns how many captures were removed.
    pub fn purge_trash(&mut self) -> Result<usize> {
        let tx = self.db.transaction()?;
        let hashes: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT DISTINCT hash FROM captures WHERE deleted_ms IS NOT NULL")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        tx.execute(
            "DELETE FROM captures_fts WHERE rowid IN
               (SELECT id FROM captures WHERE deleted_ms IS NOT NULL)",
            [],
        )?;
        let n = tx.execute("DELETE FROM captures WHERE deleted_ms IS NOT NULL", [])?;
        let mut orphans = Vec::new();
        for hash in hashes {
            let used: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM captures WHERE hash = ?1)",
                [&hash],
                |r| r.get(0),
            )?;
            if !used {
                orphans.push(hash);
            }
        }
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM capture_tags)",
            [],
        )?;
        tx.commit()?;
        for hash in orphans {
            store::remove(&self.image_path(&hash));
            store::remove(&self.thumb_path(&hash));
        }
        Ok(n)
    }

    /// Where a capture's PNG lives.
    pub fn image_path_of(&self, capture: &Capture) -> PathBuf {
        self.image_path(&capture.hash)
    }

    /// Where a capture's thumbnail lives.
    pub fn thumb_path_of(&self, capture: &Capture) -> PathBuf {
        self.thumb_path(&capture.hash)
    }

    fn image_path(&self, hash: &str) -> PathBuf {
        self.root
            .join("store")
            .join(&hash[..2.min(hash.len())])
            .join(format!("{hash}.png"))
    }

    fn thumb_path(&self, hash: &str) -> PathBuf {
        self.root.join("thumbs").join(format!("{hash}.png"))
    }

    fn with_tags(&self, mut capture: Capture) -> Result<Capture> {
        let mut stmt = self.db.prepare_cached(
            "SELECT t.name FROM tags t JOIN capture_tags ct ON ct.tag_id = t.id
             WHERE ct.capture_id = ?1 ORDER BY t.name COLLATE NOCASE",
        )?;
        capture.tags = stmt
            .query_map([capture.id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(capture)
    }
}

const COLUMNS: &str = "c.id, c.hash, c.created_ms, c.width, c.height, c.bytes, c.source, c.app,
    c.window_title, c.url, c.display_id, c.note, c.text, c.deleted_ms";

fn capture_from_row(r: &Row<'_>) -> rusqlite::Result<Capture> {
    Ok(Capture {
        id: r.get(0)?,
        hash: r.get(1)?,
        created_ms: r.get(2)?,
        width: r.get(3)?,
        height: r.get(4)?,
        bytes: r.get::<_, i64>(5)? as u64,
        meta: Metadata {
            source: Source::parse(&r.get::<_, String>(6)?),
            app: r.get(7)?,
            window_title: r.get(8)?,
            url: r.get(9)?,
            display_id: r.get::<_, Option<i64>>(10)?.map(|d| d as u64),
        },
        note: r.get(11)?,
        text: r.get(12)?,
        tags: Vec::new(),
        trashed: r.get::<_, Option<i64>>(13)?.is_some(),
    })
}

/// Rewrites a capture's full-text row from its current fields and tags.
fn reindex(db: &Connection, id: i64) -> Result<()> {
    db.execute("DELETE FROM captures_fts WHERE rowid = ?1", [id])?;
    db.execute(
        "INSERT INTO captures_fts (rowid, app, title, url, note, text, tags)
         SELECT c.id, c.app, c.window_title, c.url, c.note, c.text,
                COALESCE((SELECT group_concat(t.name, ' ') FROM capture_tags ct
                          JOIN tags t ON t.id = ct.tag_id WHERE ct.capture_id = c.id), '')
         FROM captures c WHERE c.id = ?1",
        [id],
    )?;
    Ok(())
}

fn migrate(db: &Connection) -> Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(Error::TooNew(version));
    }
    if version < 1 {
        db.execute_batch(
            "BEGIN;
             CREATE TABLE captures (
                 id           INTEGER PRIMARY KEY,
                 hash         TEXT    NOT NULL,
                 created_ms   INTEGER NOT NULL,
                 width        INTEGER NOT NULL,
                 height       INTEGER NOT NULL,
                 bytes        INTEGER NOT NULL,
                 source       TEXT    NOT NULL DEFAULT 'region',
                 app          TEXT    NOT NULL DEFAULT '',
                 window_title TEXT    NOT NULL DEFAULT '',
                 url          TEXT    NOT NULL DEFAULT '',
                 display_id   INTEGER,
                 note         TEXT    NOT NULL DEFAULT '',
                 text         TEXT    NOT NULL DEFAULT '',
                 deleted_ms   INTEGER
             );
             CREATE INDEX captures_created ON captures (deleted_ms, created_ms DESC);
             CREATE INDEX captures_hash ON captures (hash);
             CREATE TABLE tags (
                 id   INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE COLLATE NOCASE
             );
             CREATE TABLE capture_tags (
                 capture_id INTEGER NOT NULL REFERENCES captures (id) ON DELETE CASCADE,
                 tag_id     INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
                 PRIMARY KEY (capture_id, tag_id)
             ) WITHOUT ROWID;
             CREATE VIRTUAL TABLE captures_fts USING fts5 (
                 app, title, url, note, text, tags,
                 tokenize = 'unicode61 remove_diacritics 2'
             );
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
