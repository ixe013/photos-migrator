use std::path::Path;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbFileRecord {
    pub sha256: String,
    pub file_path: String,
    pub file_size: u64,
    pub is_edited_version: bool,
    pub metadata_fixed: bool,
    pub upload_token: Option<String>,
    pub google_media_id: Option<String>,
    pub uploaded_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbAlbumRecord {
    pub album_name: String,
    pub source_collaborators: Vec<String>,
    pub google_album_id: Option<String>,
    pub share_token: Option<String>,
    pub shareable_url: Option<String>,
    pub created_at: Option<String>,
}

pub struct MigrationDb {
    conn: Connection,
}

impl MigrationDb {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS files (
                sha256 TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                is_edited_version INTEGER NOT NULL DEFAULT 0,
                metadata_fixed INTEGER NOT NULL DEFAULT 0,
                upload_token TEXT,
                google_media_id TEXT,
                uploaded_at TEXT,
                error_message TEXT
            );

            CREATE TABLE IF NOT EXISTS albums (
                album_name TEXT PRIMARY KEY,
                source_collaborators TEXT NOT NULL DEFAULT '[]',
                google_album_id TEXT,
                share_token TEXT,
                shareable_url TEXT,
                created_at TEXT
            );

            CREATE TABLE IF NOT EXISTS album_items (
                album_name TEXT NOT NULL,
                file_sha256 TEXT NOT NULL,
                added_to_album INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (album_name, file_sha256),
                FOREIGN KEY(album_name) REFERENCES albums(album_name) ON DELETE CASCADE,
                FOREIGN KEY(file_sha256) REFERENCES files(sha256) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_files_uploaded ON files(uploaded_at);
            CREATE INDEX IF NOT EXISTS idx_album_items_added ON album_items(added_to_album);
            "#,
        )?;
        Ok(())
    }

    pub fn insert_or_update_file(&self, record: &DbFileRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO files (
                sha256, file_path, file_size, is_edited_version,
                metadata_fixed, upload_token, google_media_id, uploaded_at, error_message
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(sha256) DO UPDATE SET
                file_path = excluded.file_path,
                is_edited_version = excluded.is_edited_version,
                metadata_fixed = excluded.metadata_fixed
            "#,
            params![
                record.sha256,
                record.file_path,
                record.file_size as i64,
                if record.is_edited_version { 1 } else { 0 },
                if record.metadata_fixed { 1 } else { 0 },
                record.upload_token,
                record.google_media_id,
                record.uploaded_at,
                record.error_message,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_file_by_hash(&self, sha256: &str) -> Result<Option<DbFileRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT sha256, file_path, file_size, is_edited_version,
                   metadata_fixed, upload_token, google_media_id, uploaded_at, error_message
            FROM files WHERE sha256 = ?1
            "#,
        )?;

        let mut rows = stmt.query(params![sha256])?;
        if let Some(row) = rows.next()? {
            Ok(Some(DbFileRecord {
                sha256: row.get(0)?,
                file_path: row.get(1)?,
                file_size: row.get::<_, i64>(2)? as u64,
                is_edited_version: row.get::<_, i32>(3)? != 0,
                metadata_fixed: row.get::<_, i32>(4)? != 0,
                upload_token: row.get(5)?,
                google_media_id: row.get(6)?,
                uploaded_at: row.get(7)?,
                error_message: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn mark_file_uploaded(&self, sha256: &str, google_media_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            r#"
            UPDATE files
            SET google_media_id = ?1, uploaded_at = ?2, error_message = NULL
            WHERE sha256 = ?3
            "#,
            params![google_media_id, now, sha256],
        )?;
        Ok(())
    }

    pub fn mark_file_error(&self, sha256: &str, error: &str) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE files
            SET error_message = ?1
            WHERE sha256 = ?2
            "#,
            params![error, sha256],
        )?;
        Ok(())
    }

    pub fn get_pending_uploads(&self) -> Result<Vec<DbFileRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT sha256, file_path, file_size, is_edited_version,
                   metadata_fixed, upload_token, google_media_id, uploaded_at, error_message
            FROM files
            WHERE google_media_id IS NULL
            ORDER BY file_size ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(DbFileRecord {
                sha256: row.get(0)?,
                file_path: row.get(1)?,
                file_size: row.get::<_, i64>(2)? as u64,
                is_edited_version: row.get::<_, i32>(3)? != 0,
                metadata_fixed: row.get::<_, i32>(4)? != 0,
                upload_token: row.get(5)?,
                google_media_id: row.get(6)?,
                uploaded_at: row.get(7)?,
                error_message: row.get(8)?,
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn insert_or_update_album(&self, album: &DbAlbumRecord) -> Result<()> {
        let collab_json = serde_json::to_string(&album.source_collaborators).unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            r#"
            INSERT INTO albums (
                album_name, source_collaborators, google_album_id,
                share_token, shareable_url, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(album_name) DO UPDATE SET
                source_collaborators = excluded.source_collaborators,
                google_album_id = COALESCE(excluded.google_album_id, albums.google_album_id),
                share_token = COALESCE(excluded.share_token, albums.share_token),
                shareable_url = COALESCE(excluded.shareable_url, albums.shareable_url)
            "#,
            params![
                album.album_name,
                collab_json,
                album.google_album_id,
                album.share_token,
                album.shareable_url,
                album.created_at,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_album(&self, album_name: &str) -> Result<Option<DbAlbumRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT album_name, source_collaborators, google_album_id, share_token, shareable_url, created_at
            FROM albums WHERE album_name = ?1
            "#,
        )?;

        let mut rows = stmt.query(params![album_name])?;
        if let Some(row) = rows.next()? {
            let collab_str: String = row.get(1)?;
            let collabs: Vec<String> = serde_json::from_str(&collab_str).unwrap_or_default();
            Ok(Some(DbAlbumRecord {
                album_name: row.get(0)?,
                source_collaborators: collabs,
                google_album_id: row.get(2)?,
                share_token: row.get(3)?,
                shareable_url: row.get(4)?,
                created_at: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_all_albums(&self) -> Result<Vec<DbAlbumRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT album_name, source_collaborators, google_album_id, share_token, shareable_url, created_at
            FROM albums
            ORDER BY album_name ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let collab_str: String = row.get(1)?;
            let collabs: Vec<String> = serde_json::from_str(&collab_str).unwrap_or_default();
            Ok(DbAlbumRecord {
                album_name: row.get(0)?,
                source_collaborators: collabs,
                google_album_id: row.get(2)?,
                share_token: row.get(3)?,
                shareable_url: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn link_album_item(&self, album_name: &str, file_sha256: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO album_items (album_name, file_sha256, added_to_album)
            VALUES (?1, ?2, 0)
            "#,
            params![album_name, file_sha256],
        )?;
        Ok(())
    }

    pub fn mark_album_item_added(&self, album_name: &str, file_sha256: &str) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE album_items
            SET added_to_album = 1
            WHERE album_name = ?1 AND file_sha256 = ?2
            "#,
            params![album_name, file_sha256],
        )?;
        Ok(())
    }

    pub fn get_unadded_album_media_ids(&self, album_name: &str) -> Result<Vec<(String, String)>> {
        // Returns (file_sha256, google_media_id) for items whose file is uploaded but not yet added to album
        let mut stmt = self.conn.prepare(
            r#"
            SELECT ai.file_sha256, f.google_media_id
            FROM album_items ai
            JOIN files f ON ai.file_sha256 = f.sha256
            WHERE ai.album_name = ?1
              AND ai.added_to_album = 0
              AND f.google_media_id IS NOT NULL
            "#,
        )?;

        let rows = stmt.query_map(params![album_name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn get_stats(&self) -> Result<MigrationStats> {
        let total_files: i64 = self.conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let uploaded_files: i64 = self.conn.query_row("SELECT COUNT(*) FROM files WHERE google_media_id IS NOT NULL", [], |r| r.get(0))?;
        let total_albums: i64 = self.conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
        let created_albums: i64 = self.conn.query_row("SELECT COUNT(*) FROM albums WHERE google_album_id IS NOT NULL", [], |r| r.get(0))?;
        let shared_albums: i64 = self.conn.query_row("SELECT COUNT(*) FROM albums WHERE shareable_url IS NOT NULL", [], |r| r.get(0))?;
        let total_bytes: i64 = self.conn.query_row("SELECT COALESCE(SUM(file_size), 0) FROM files", [], |r| r.get(0))?;
        let uploaded_bytes: i64 = self.conn.query_row("SELECT COALESCE(SUM(file_size), 0) FROM files WHERE google_media_id IS NOT NULL", [], |r| r.get(0))?;

        Ok(MigrationStats {
            total_files: total_files as u64,
            uploaded_files: uploaded_files as u64,
            total_albums: total_albums as u64,
            created_albums: created_albums as u64,
            shared_albums: shared_albums as u64,
            total_bytes: total_bytes as u64,
            uploaded_bytes: uploaded_bytes as u64,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    pub total_files: u64,
    pub uploaded_files: u64,
    pub total_albums: u64,
    pub created_albums: u64,
    pub shared_albums: u64,
    pub total_bytes: u64,
    pub uploaded_bytes: u64,
}
