use gc_core::metadata::{Bookmark, MediaKind};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: i64,
    pub path: PathBuf,
    pub kind: MediaKind,
    pub game_title: Option<String>,
    pub executable: Option<String>,
    /// Capture time, unix epoch milliseconds.
    pub created_ms: i64,
    pub duration_ms: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: u64,
    /// File modification time (unix ms) used to detect changes.
    pub mtime_ms: i64,
    pub bookmarks: Vec<Bookmark>,
    pub audio_tracks: Vec<String>,
    pub video_codec: Option<String>,
    pub thumbnail: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    #[default]
    Newest,
    Oldest,
    Longest,
    Shortest,
    Largest,
    Game,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MediaFilter {
    pub game: Option<String>,
    pub kind: Option<MediaKind>,
    /// Case-insensitive substring of game title or file name.
    pub search: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub min_duration_ms: Option<u64>,
    pub max_duration_ms: Option<u64>,
    pub sort: SortOrder,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    pub title: String,
    pub count: u64,
    pub total_bytes: u64,
    pub latest_ms: i64,
    pub has_icon: bool,
}

pub struct Library {
    conn: Connection,
}

/// What the scanner needs to know about an indexed file:
/// `(id, size_bytes, mtime_ms, thumbnail)`.
pub type KnownFile = (i64, u64, i64, Option<PathBuf>);

impl Library {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS media (
                    id            INTEGER PRIMARY KEY,
                    path          TEXT NOT NULL UNIQUE,
                    kind          TEXT NOT NULL,
                    game_title    TEXT,
                    executable    TEXT,
                    created_ms    INTEGER NOT NULL,
                    duration_ms   INTEGER NOT NULL DEFAULT 0,
                    width         INTEGER,
                    height        INTEGER,
                    size_bytes    INTEGER NOT NULL DEFAULT 0,
                    mtime_ms      INTEGER NOT NULL DEFAULT 0,
                    bookmarks     TEXT NOT NULL DEFAULT '[]',
                    audio_tracks  TEXT NOT NULL DEFAULT '[]',
                    video_codec   TEXT,
                    thumbnail     TEXT
                );
                CREATE INDEX IF NOT EXISTS media_created ON media(created_ms);
                CREATE INDEX IF NOT EXISTS media_game ON media(game_title);
                CREATE TABLE IF NOT EXISTS game_icons (
                    title   TEXT PRIMARY KEY COLLATE NOCASE,
                    png     BLOB NOT NULL
                );
                "#,
            )?;
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { conn })
    }

    fn row_to_item(r: &Row<'_>) -> rusqlite::Result<MediaItem> {
        let kind: String = r.get("kind")?;
        let bookmarks: String = r.get("bookmarks")?;
        let tracks: String = r.get("audio_tracks")?;
        Ok(MediaItem {
            id: r.get("id")?,
            path: PathBuf::from(r.get::<_, String>("path")?),
            kind: MediaKind::from_label(&kind).unwrap_or(MediaKind::Clip),
            game_title: r.get("game_title")?,
            executable: r.get("executable")?,
            created_ms: r.get("created_ms")?,
            duration_ms: r.get::<_, i64>("duration_ms")?.max(0) as u64,
            width: r.get("width")?,
            height: r.get("height")?,
            size_bytes: r.get::<_, i64>("size_bytes")?.max(0) as u64,
            mtime_ms: r.get("mtime_ms")?,
            bookmarks: serde_json::from_str(&bookmarks).unwrap_or_default(),
            audio_tracks: serde_json::from_str(&tracks).unwrap_or_default(),
            video_codec: r.get("video_codec")?,
            thumbnail: r.get::<_, Option<String>>("thumbnail")?.map(PathBuf::from),
        })
    }

    /// Inserts or replaces by path; returns the row id.
    pub fn upsert(&self, item: &MediaItem) -> anyhow::Result<i64> {
        let id = self.conn.query_row(
            r#"INSERT INTO media (path, kind, game_title, executable, created_ms, duration_ms, width, height,
                                  size_bytes, mtime_ms, bookmarks, audio_tracks, video_codec, thumbnail)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
               ON CONFLICT(path) DO UPDATE SET
                   kind=excluded.kind, game_title=excluded.game_title, executable=excluded.executable,
                   created_ms=excluded.created_ms, duration_ms=excluded.duration_ms, width=excluded.width,
                   height=excluded.height, size_bytes=excluded.size_bytes, mtime_ms=excluded.mtime_ms,
                   bookmarks=excluded.bookmarks, audio_tracks=excluded.audio_tracks,
                   video_codec=excluded.video_codec, thumbnail=excluded.thumbnail
               RETURNING id"#,
            params![
                item.path.to_string_lossy(),
                item.kind.as_str(),
                item.game_title,
                item.executable,
                item.created_ms,
                item.duration_ms as i64,
                item.width,
                item.height,
                item.size_bytes as i64,
                item.mtime_ms,
                serde_json::to_string(&item.bookmarks)?,
                serde_json::to_string(&item.audio_tracks)?,
                item.video_codec,
                item.thumbnail.as_ref().map(|p| p.to_string_lossy().into_owned()),
            ],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn get(&self, id: i64) -> anyhow::Result<Option<MediaItem>> {
        Ok(self
            .conn
            .query_row("SELECT * FROM media WHERE id = ?1", [id], Self::row_to_item)
            .optional()?)
    }

    pub fn get_by_path(&self, path: &Path) -> anyhow::Result<Option<MediaItem>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM media WHERE path = ?1",
                [path.to_string_lossy()],
                Self::row_to_item,
            )
            .optional()?)
    }

    pub fn remove(&self, id: i64) -> anyhow::Result<Option<MediaItem>> {
        let item = self.get(id)?;
        self.conn.execute("DELETE FROM media WHERE id = ?1", [id])?;
        Ok(item)
    }

    /// `path → (id, size, mtime, thumbnail)` for every indexed file.
    pub fn known_files(&self) -> anyhow::Result<HashMap<PathBuf, KnownFile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, size_bytes, mtime_ms, thumbnail FROM media")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(1)?),
                (
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(2)?.max(0) as u64,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?.map(PathBuf::from),
                ),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn list(&self, f: &MediaFilter) -> anyhow::Result<Vec<MediaItem>> {
        let mut sql = String::from("SELECT * FROM media WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        // Every `?` in a clause refers to the value pushed with it.
        let mut arg = |sql: &mut String, clause: &str, v: rusqlite::types::Value| {
            args.push(v);
            sql.push_str(&clause.replace('?', &format!("?{}", args.len())));
        };
        if let Some(g) = &f.game {
            if g == gc_core::naming::UNCATEGORIZED {
                sql.push_str(" AND game_title IS NULL");
            } else {
                arg(
                    &mut sql,
                    " AND game_title = ? COLLATE NOCASE",
                    g.clone().into(),
                );
            }
        }
        if let Some(k) = f.kind {
            arg(&mut sql, " AND kind = ?", k.as_str().to_string().into());
        }
        if let Some(s) = f
            .search
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            let pat = format!(
                "%{}%",
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            arg(
                &mut sql,
                " AND (game_title LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\')",
                pat.into(),
            );
        }
        if let Some(v) = f.from_ms {
            arg(&mut sql, " AND created_ms >= ?", v.into());
        }
        if let Some(v) = f.to_ms {
            arg(&mut sql, " AND created_ms <= ?", v.into());
        }
        if let Some(v) = f.min_duration_ms {
            arg(&mut sql, " AND duration_ms >= ?", (v as i64).into());
        }
        if let Some(v) = f.max_duration_ms {
            arg(&mut sql, " AND duration_ms <= ?", (v as i64).into());
        }
        sql.push_str(match f.sort {
            SortOrder::Newest => " ORDER BY created_ms DESC, id DESC",
            SortOrder::Oldest => " ORDER BY created_ms ASC, id ASC",
            SortOrder::Longest => " ORDER BY duration_ms DESC, created_ms DESC",
            SortOrder::Shortest => " ORDER BY duration_ms ASC, created_ms DESC",
            SortOrder::Largest => " ORDER BY size_bytes DESC",
            SortOrder::Game => {
                " ORDER BY game_title IS NULL, game_title COLLATE NOCASE, created_ms DESC"
            }
        });
        sql.push_str(&format!(
            " LIMIT {} OFFSET {}",
            f.limit.map(i64::from).unwrap_or(-1),
            f.offset.unwrap_or(0)
        ));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), Self::row_to_item)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn games(&self) -> anyhow::Result<Vec<GameSummary>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT COALESCE(m.game_title, ?1) AS title, COUNT(*), SUM(m.size_bytes), MAX(m.created_ms),
                      EXISTS(SELECT 1 FROM game_icons i WHERE i.title = m.game_title)
               FROM media m GROUP BY m.game_title COLLATE NOCASE ORDER BY MAX(m.created_ms) DESC"#,
        )?;
        let rows = stmt.query_map([gc_core::naming::UNCATEGORIZED], |r| {
            Ok(GameSummary {
                title: r.get(0)?,
                count: r.get::<_, i64>(1)? as u64,
                total_bytes: r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
                latest_ms: r.get(3)?,
                has_icon: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_game_icon(&self, title: &str, png: &[u8]) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO game_icons (title, png) VALUES (?1, ?2) ON CONFLICT(title) DO UPDATE SET png = excluded.png",
            params![title, png],
        )?;
        Ok(())
    }

    pub fn game_icon(&self, title: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT png FROM game_icons WHERE title = ?1",
                [title],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn update_bookmarks(&self, id: i64, bookmarks: &[Bookmark]) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE media SET bookmarks = ?1 WHERE id = ?2",
            params![serde_json::to_string(bookmarks)?, id],
        )?;
        Ok(())
    }

    pub fn count(&self) -> anyhow::Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get::<_, i64>(0))? as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn item(path: &str, game: Option<&str>, created: i64, dur: u64) -> MediaItem {
        MediaItem {
            id: 0,
            path: PathBuf::from(path),
            kind: MediaKind::Clip,
            game_title: game.map(String::from),
            executable: None,
            created_ms: created,
            duration_ms: dur,
            width: Some(1920),
            height: Some(1080),
            size_bytes: dur * 10,
            mtime_ms: created,
            bookmarks: vec![Bookmark {
                offset_ms: 1,
                label: None,
            }],
            audio_tracks: vec!["Master".into()],
            video_codec: Some("h264".into()),
            thumbnail: None,
        }
    }

    #[test]
    fn upsert_list_filter_sort() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib
            .upsert(&item(
                "/l/Fortnite/a_Clip.mp4",
                Some("Fortnite"),
                100,
                30_000,
            ))
            .unwrap();
        lib.upsert(&item(
            "/l/Elden Ring/b_Clip.mp4",
            Some("Elden Ring"),
            200,
            60_000,
        ))
        .unwrap();
        lib.upsert(&item("/l/Uncategorized/c_Clip.mp4", None, 300, 10_000))
            .unwrap();
        // Upsert of same path keeps the id.
        let again = lib
            .upsert(&item(
                "/l/Fortnite/a_Clip.mp4",
                Some("Fortnite"),
                100,
                31_000,
            ))
            .unwrap();
        assert_eq!(a, again);
        assert_eq!(lib.count().unwrap(), 3);

        let all = lib.list(&MediaFilter::default()).unwrap();
        assert_eq!(
            all.iter().map(|i| i.created_ms).collect::<Vec<_>>(),
            [300, 200, 100]
        );
        assert_eq!(all[2].bookmarks.len(), 1);

        let f = MediaFilter {
            game: Some("fortnite".into()),
            ..Default::default()
        };
        assert_eq!(lib.list(&f).unwrap().len(), 1);
        let f = MediaFilter {
            game: Some(gc_core::naming::UNCATEGORIZED.into()),
            ..Default::default()
        };
        assert_eq!(lib.list(&f).unwrap()[0].created_ms, 300);
        let f = MediaFilter {
            search: Some("elden".into()),
            ..Default::default()
        };
        assert_eq!(lib.list(&f).unwrap().len(), 1);
        let f = MediaFilter {
            search: Some("100%".into()),
            ..Default::default()
        };
        assert!(lib.list(&f).unwrap().is_empty());
        let f = MediaFilter {
            min_duration_ms: Some(20_000),
            sort: SortOrder::Shortest,
            ..Default::default()
        };
        let r = lib.list(&f).unwrap();
        assert_eq!(
            r.iter().map(|i| i.duration_ms).collect::<Vec<_>>(),
            [31_000, 60_000]
        );
        let f = MediaFilter {
            from_ms: Some(150),
            to_ms: Some(250),
            ..Default::default()
        };
        assert_eq!(lib.list(&f).unwrap().len(), 1);
        let f = MediaFilter {
            limit: Some(1),
            offset: Some(1),
            ..Default::default()
        };
        assert_eq!(lib.list(&f).unwrap()[0].created_ms, 200);

        let games = lib.games().unwrap();
        assert_eq!(
            games.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            ["Uncategorized", "Elden Ring", "Fortnite"]
        );
        lib.set_game_icon("Fortnite", b"png").unwrap();
        assert!(lib
            .games()
            .unwrap()
            .iter()
            .any(|g| g.title == "Fortnite" && g.has_icon));
        assert_eq!(
            lib.game_icon("FORTNITE").unwrap().as_deref(),
            Some(&b"png"[..])
        );

        let removed = lib.remove(a).unwrap().unwrap();
        assert_eq!(removed.game_title.as_deref(), Some("Fortnite"));
        assert_eq!(lib.count().unwrap(), 2);
    }
}
