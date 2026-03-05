use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::error::PikeError;
use crate::package::{PackageUpdate, SourceType};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self, PikeError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(Self { conn })
    }

    pub fn new_in_memory() -> Result<Self, PikeError> {
        let conn = Connection::open_in_memory()?;
        Ok(Self { conn })
    }

    pub fn default_path() -> Result<PathBuf, PikeError> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| PikeError::Config("could not determine data directory".into()))?;
        Ok(data_dir.join("pike").join("pike.db"))
    }

    pub fn migrate(&self) -> Result<(), PikeError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS update_cache (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                source            TEXT NOT NULL,
                package_name      TEXT NOT NULL,
                installed_version TEXT,
                available_version TEXT,
                checked_at        TEXT NOT NULL,
                UNIQUE(source, package_name)
            );
            CREATE TABLE IF NOT EXISTS metadata (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    pub fn upsert_updates(&self, updates: &[PackageUpdate]) -> Result<(), PikeError> {
        let tx = self.conn.unchecked_transaction()?;
        insert_updates_in_tx(&tx, updates, true)?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_cached_updates(&self) -> Result<Vec<PackageUpdate>, PikeError> {
        let mut stmt = self.conn.prepare(
            "SELECT source, package_name, installed_version, available_version
             FROM update_cache
             ORDER BY source, package_name",
        )?;
        let rows = stmt.query_map([], |row| {
            let source_str: String = row.get(0)?;
            Ok((
                source_str,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut updates = Vec::new();
        for row in rows {
            let (source_str, name, installed, available) = row?;
            updates.push(row_to_update(source_str, name, installed, available));
        }
        Ok(updates)
    }

    pub fn replace_cache(&self, updates: &[PackageUpdate]) -> Result<(), PikeError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM update_cache", [])?;
        insert_updates_in_tx(&tx, updates, false)?;
        tx.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_checked', ?1)",
            params![chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_last_checked(&self) -> Result<Option<String>, PikeError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM metadata WHERE key = 'last_checked'")?;
        match stmt.query_row([], |row| row.get::<_, String>(0)) {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn clear_cache(&self) -> Result<(), PikeError> {
        self.conn.execute("DELETE FROM update_cache", [])?;
        Ok(())
    }
}

fn insert_updates_in_tx(
    tx: &rusqlite::Transaction,
    updates: &[PackageUpdate],
    or_replace: bool,
) -> Result<(), PikeError> {
    let now = chrono::Utc::now().to_rfc3339();
    let sql = if or_replace {
        "INSERT OR REPLACE INTO update_cache
            (source, package_name, installed_version, available_version, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5)"
    } else {
        "INSERT INTO update_cache
            (source, package_name, installed_version, available_version, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5)"
    };
    let mut stmt = tx.prepare(sql)?;
    for u in updates {
        stmt.execute(params![
            u.source.to_string(),
            u.name,
            u.installed_version,
            u.available_version,
            now,
        ])?;
    }
    Ok(())
}

fn row_to_update(
    source_str: String,
    name: String,
    installed: String,
    available: String,
) -> PackageUpdate {
    PackageUpdate {
        name,
        source: source_str.parse().unwrap_or_else(|_| {
            tracing::warn!(
                "unknown source '{}' in cache, defaulting to dnf",
                source_str
            );
            SourceType::Dnf
        }),
        arch: None,
        installed_version: installed,
        available_version: available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        let db = Database::new_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    #[test]
    fn test_migrate_creates_table() {
        let db = setup_db();
        db.migrate().unwrap();
    }

    #[test]
    fn test_upsert_and_get_updates() {
        let db = setup_db();
        let updates = vec![
            PackageUpdate {
                name: "bash".into(),
                source: SourceType::Dnf,
                arch: None,
                installed_version: "5.2.37".into(),
                available_version: "5.2.38".into(),
            },
            PackageUpdate {
                name: "org.mozilla.firefox".into(),
                source: SourceType::Flatpak,
                arch: None,
                installed_version: "136.0".into(),
                available_version: "137.0".into(),
            },
        ];
        db.upsert_updates(&updates).unwrap();

        let cached = db.get_cached_updates().unwrap();
        assert_eq!(cached.len(), 2);
        assert_eq!(cached[0].name, "bash");
        assert_eq!(cached[0].source, SourceType::Dnf);
        assert_eq!(cached[1].name, "org.mozilla.firefox");
        assert_eq!(cached[1].source, SourceType::Flatpak);
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let db = setup_db();
        let v1 = vec![PackageUpdate {
            name: "bash".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "5.2.37".into(),
            available_version: "5.2.38".into(),
        }];
        db.upsert_updates(&v1).unwrap();

        let v2 = vec![PackageUpdate {
            name: "bash".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "5.2.38".into(),
            available_version: "5.2.39".into(),
        }];
        db.upsert_updates(&v2).unwrap();

        let cached = db.get_cached_updates().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].available_version, "5.2.39");
    }

    #[test]
    fn test_clear_cache() {
        let db = setup_db();
        let updates = vec![PackageUpdate {
            name: "bash".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "5.2.37".into(),
            available_version: "5.2.38".into(),
        }];
        db.upsert_updates(&updates).unwrap();
        db.clear_cache().unwrap();
        let cached = db.get_cached_updates().unwrap();
        assert!(cached.is_empty());
    }

    #[test]
    fn test_replace_cache_is_atomic() {
        let db = setup_db();
        let initial = vec![PackageUpdate {
            name: "bash".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "5.2.37".into(),
            available_version: "5.2.38".into(),
        }];
        db.upsert_updates(&initial).unwrap();

        let replacement = vec![PackageUpdate {
            name: "vim".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "9.0".into(),
            available_version: "9.1".into(),
        }];
        db.replace_cache(&replacement).unwrap();

        let cached = db.get_cached_updates().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].name, "vim");
    }

    #[test]
    fn test_empty_get() {
        let db = setup_db();
        let cached = db.get_cached_updates().unwrap();
        assert!(cached.is_empty());
    }

    #[test]
    fn test_last_checked_empty() {
        let db = setup_db();
        assert!(db.get_last_checked().unwrap().is_none());
    }

    #[test]
    fn test_last_checked_survives_empty_replace() {
        let db = setup_db();
        let updates = vec![PackageUpdate {
            name: "bash".into(),
            source: SourceType::Dnf,
            arch: None,
            installed_version: "5.2.37".into(),
            available_version: "5.2.38".into(),
        }];
        db.replace_cache(&updates).unwrap();
        assert!(db.get_last_checked().unwrap().is_some());

        db.replace_cache(&[]).unwrap();
        assert!(db.get_cached_updates().unwrap().is_empty());
        assert!(db.get_last_checked().unwrap().is_some());
    }
}
