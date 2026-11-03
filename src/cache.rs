use crate::{config::Profile, results::Cells};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Entry {
    connection: String,
    fetched: u64,
    #[serde(default)]
    day: u32,
    rows: Vec<Cells>,
    #[serde(default)]
    previous: Option<Vec<Cells>>,
}

#[derive(Default, Deserialize, Serialize)]
pub struct MetadataCache {
    entries: BTreeMap<String, Entry>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn today() -> u32 {
    let mut time = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut time);
    }
    time.wYear as u32 * 10000 + time.wMonth as u32 * 100 + time.wDay as u32
}
fn path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("SqlDesk")
            .join("metadata-cache.json"),
    )
}
fn connection(profile: &Profile) -> String {
    serde_json::to_string(&(profile.redshift, &profile.host, profile.port, &profile.user)).unwrap()
}
fn key(profile: &Profile, sql: &str) -> String {
    let database = if sql.contains("FROM pg_catalog.pg_database") {
        ""
    } else {
        &profile.database
    };
    serde_json::to_string(&(connection(profile), database, sql)).unwrap()
}

impl MetadataCache {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        if fs::metadata(&path).map_or(true, |meta| meta.len() > MAX_BYTES as u64) {
            return Self::default();
        }
        fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }
    pub fn get(&self, profile: &Profile, sql: &str) -> Option<Vec<Cells>> {
        let entry = self.entries.get(&key(profile, sql))?;
        let columns = sql.contains("FROM pg_catalog.pg_attribute") || sql.contains("FROM svv_all_columns");
        (entry.day != 0 && (!columns || entry.day == today())).then(|| entry.rows.clone())
    }
    pub fn insert(&mut self, profile: &Profile, sql: &str, rows: &[Cells]) {
        let previous = self.entries.get(&key(profile, sql)).map(|entry| entry.rows.clone());
        self.entries.insert(
            key(profile, sql),
            Entry {
                connection: connection(profile),
                fetched: now(),
                day: today(),
                rows: rows.to_vec(),
                previous,
            },
        );
        self.save();
    }
    pub fn invalidate(&mut self, profile: &Profile) {
        let connection = connection(profile);
        for entry in self.entries.values_mut().filter(|entry| entry.connection == connection) {
            entry.day = 0;
        }
        self.save();
    }
    pub fn invalidate_scope(&mut self, profile: &Profile, schema: &str) -> Vec<String> {
        let identity = connection(profile);
        let literal = crate::db::literal(schema, profile.redshift);
        let mut queries = Vec::new();
        for (key, entry) in &mut self.entries {
            let Ok((saved_identity, database, sql)) = serde_json::from_str::<(String, String, String)>(key) else { continue; };
            if saved_identity == identity && database == profile.database
                && [format!("n.nspname = {literal} "), format!("n.nspname={literal} "), format!("schema_name = {literal} ")].iter().any(|part| sql.contains(part)) {
                entry.day = 0;
                queries.push(sql);
            }
        }
        self.save();
        queries
    }
    pub fn changed_columns(&self, profile: &Profile, sql: &str, rows: &[Cells]) -> Vec<bool> {
        let previous = self.entries.get(&key(profile, sql)).and_then(|entry| entry.previous.as_ref());
        rows.iter().map(|row| previous.is_some_and(|old| !old.iter().any(|r| r.get(..2) == row.get(..2)))).collect()
    }
    pub fn changed_tables(&self, profile: &Profile, schema: &str, objects: &[crate::DbObject]) -> Vec<String> {
        let identity = connection(profile);
        let schema = crate::db::literal(schema, profile.redshift);
        let mut changed = Vec::new();
        for (saved_key, entry) in &self.entries {
            let Ok((saved_identity, database, sql)) = serde_json::from_str::<(String, String, String)>(saved_key) else { continue; };
            if saved_identity != identity || database != profile.database { continue; }
            let Some(previous) = &entry.previous else { continue; };
            if entry.rows.iter().map(|row| row.get(..2)).eq(previous.iter().map(|row| row.get(..2))) { continue; }
            for object in objects {
                let table = crate::db::literal(&object.name, profile.redshift);
                if sql.contains(&format!("n.nspname={schema} AND c.relname={table} AND"))
                    || sql.contains(&format!("schema_name = {schema} AND table_name = {table} ORDER BY")) {
                    changed.push(object.name.to_string());
                }
            }
        }
        changed
    }
    fn save(&mut self) {
        loop {
            let Ok(data) = serde_json::to_vec(self) else {
                return;
            };
            if data.len() <= MAX_BYTES && self.entries.len() <= 128 {
                if let Some(path) = path() {
                    let dir = path.parent().unwrap();
                    if fs::create_dir_all(dir).is_err() {
                        return;
                    }
                    if let Ok(mut temp) = tempfile::NamedTempFile::new_in(dir) {
                        if temp.write_all(&data).is_ok() && temp.as_file().sync_all().is_ok() {
                            let _ = temp.persist(path);
                        }
                    }
                }
                return;
            }
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.fetched)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn column_changes_include_types_and_clear_on_the_next_snapshot() {
        let profile = Profile { database: "dev".into(), ..Profile::default() };
        let sql = "SELECT a.attname FROM pg_catalog.pg_attribute a WHERE n.nspname='public' AND c.relname='orders' AND a.attnum>0";
        let mut cache = MetadataCache::default();
        let rows = vec![vec![Some("id".into()), Some("bigint".into())]];
        cache.entries.insert(key(&profile, sql), Entry {
            connection: connection(&profile), fetched: now(), day: today(), rows: rows.clone(),
            previous: Some(vec![vec![Some("id".into()), Some("integer".into())]]),
        });
        let objects = vec![crate::DbObject { name: "orders".into(), kind: 0 }];
        assert_eq!(cache.changed_columns(&profile, sql, &rows), vec![true]);
        assert_eq!(cache.changed_tables(&profile, "public", &objects), vec!["orders"]);
        cache.entries.get_mut(&key(&profile, sql)).unwrap().previous = Some(rows.clone());
        assert_eq!(cache.changed_columns(&profile, sql, &rows), vec![false]);
        assert!(cache.changed_tables(&profile, "public", &objects).is_empty());
    }
    #[test]
    fn metadata_is_isolated_by_identity_database_and_age() {
        let mut profile = Profile {
            host: "host".into(),
            user: "user".into(),
            database: "dev".into(),
            ..Profile::default()
        };
        let sql = "SELECT schema_name FROM information_schema.schemata";
        let mut cache = MetadataCache::default();
        cache.entries.insert(
            key(&profile, sql),
            Entry {
                connection: connection(&profile),
                fetched: now(),
                day: today(),
                rows: vec![vec![Some("public".into())]],
                previous: None,
            },
        );
        assert!(cache.get(&profile, sql).is_some());
        profile.database = "prod".into();
        assert!(cache.get(&profile, sql).is_none());
        profile.database = "dev".into();
        profile.user = "other".into();
        assert!(cache.get(&profile, sql).is_none());
        profile.user = "user".into();
        cache.entries.get_mut(&key(&profile, sql)).unwrap().day = 0;
        assert!(cache.get(&profile, sql).is_none());
    }
}
