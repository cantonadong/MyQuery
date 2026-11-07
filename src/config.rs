use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Profile {
    #[serde(default)]
    pub shared_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
    pub name: String,
    pub redshift: bool,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub tls: bool,
    #[serde(default)]
    pub trust_server_certificate: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(skip)]
    baseline: std::sync::Arc<std::sync::Mutex<serde_json::Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
    pub profiles: Vec<Profile>,
    pub language: Option<String>,
    #[serde(default)]
    pub dark_theme: bool,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub query_timeout: Option<u64>,
    #[serde(default)]
    pub last_selection: Option<BrowserSelection>,
    #[serde(default)]
    pub browser_scopes: Vec<BrowserSelection>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub table_prefixes: Vec<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_prefix: Option<String>,
    #[serde(default)]
    pub prefix_filters: BTreeMap<String, PrefixFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_objects: Vec<HiddenObject>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HiddenObject {
    pub scope: BrowserSelection,
    pub name: String,
    pub kind: i32,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct PrefixFilter {
    pub prefixes: Vec<String>,
    pub selected: Option<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrowserSelection {
    pub profile: String,
    pub database: String,
    pub schema: String,
}

impl Settings {
    pub fn visible_profiles(&self) -> Vec<String> {
        self.profiles.iter().map(|profile| profile.name.clone()).collect()
    }
}

fn path() -> Result<PathBuf, String> {
    let root = std::env::var_os("APPDATA").ok_or("APPDATA unavailable")?;
    Ok(PathBuf::from(root).join("MyDataTools").join("connections.json"))
}

pub fn load() -> Result<Settings, String> {
    let path = path()?;
    let _lock = config_lock()?;
    if !path.exists() {
        let legacy = PathBuf::from(std::env::var_os("APPDATA").ok_or("APPDATA unavailable")?).join("SqlDesk/connections.json");
        if legacy.exists() { fs::copy(legacy, &path).map_err(|e| e.to_string())?; }
    }
    if !path.exists() {
        return Ok(Settings::default());
    }
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("Connection configuration exceeds 1 MiB".into());
    }
    let mut settings: Settings = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if !settings.table_prefixes.is_empty() {
        let profile = settings.last_selection.as_ref().map(|last| last.profile.clone()).or_else(|| settings.profiles.first().map(|profile| profile.name.clone()));
        if let Some(profile) = profile {
            settings.prefix_filters.entry(profile).or_insert(PrefixFilter {
                prefixes: std::mem::take(&mut settings.table_prefixes),
                selected: settings.locked_prefix.take(),
            });
        }
    }
    *settings.baseline.lock().unwrap() = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

fn config_lock() -> Result<fs::File, String> {
    let path = path()?.with_extension("lock");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    Ok(file)
}

pub fn save(settings: &Settings) -> Result<(), String> {
    let _lock = config_lock()?;
    let current = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    let mut baseline = settings.baseline.lock().unwrap();
    let mut merged: serde_json::Value = if path()?.exists() { serde_json::from_slice(&fs::read(path()?).map_err(|e| e.to_string())?).map_err(|e| e.to_string())? } else { serde_json::json!({}) };
    for (key, value) in current.as_object().unwrap() {
        if baseline.get(key) == Some(value) { continue; }
        if key == "profiles" || key == "browser_scopes" {
            let old = baseline.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let incoming = value.as_array().unwrap();
            let saved = merged.as_object_mut().ok_or("Invalid shared configuration")?.entry(key.clone()).or_insert(serde_json::json!([]));
            let saved = saved.as_array_mut().ok_or("Invalid shared collection")?;
            let identity = |v: &serde_json::Value| if key == "profiles" { v["name"].to_string() } else { v.to_string() };
            for prior in &old {
                if !incoming.iter().any(|v| identity(v) == identity(prior)) { saved.retain(|v| identity(v) != identity(prior)); }
            }
            for item in incoming {
                let prior = old.iter().find(|v| identity(v) == identity(item));
                if prior == Some(item) { continue; }
                if let Some(existing) = saved.iter_mut().find(|v| identity(v) == identity(item)) {
                    if key == "profiles" {
                        for (field, field_value) in item.as_object().unwrap() {
                            if prior.and_then(|v| v.get(field)) != Some(field_value) { existing[field] = field_value.clone(); }
                        }
                    } else { *existing = item.clone(); }
                } else { saved.push(item.clone()); }
            }
        } else { merged[key] = value.clone(); }
    }
    if let Some(old) = baseline.as_object() {
        for key in old.keys().filter(|key| !current.as_object().unwrap().contains_key(*key)) {
            merged.as_object_mut().ok_or("Invalid shared configuration")?.remove(key);
        }
    }
    let data = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
    if data.len() > 1024 * 1024 {
        return Err("Connection configuration exceeds 1 MiB".into());
    }
    let path = path()?;
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    temp.write_all(&data).map_err(|e| e.to_string())?;
    temp.flush().map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    *baseline = current;
    Ok(())
}

pub fn password(name: &str) -> Result<String, String> {
    let _lock = password_lock()?;
    let mut stored = passwords()?;
    if let Some(password) = stored.get(name) {
        return Ok(password.clone());
    }
    match keyring::Entry::new("SqlDesk", name)
        .map_err(|e| e.to_string())?
        .get_password()
    {
        Ok(password) => {
            stored.insert(name.to_owned(), password.clone());
            save_passwords(&stored)?;
            delete_legacy_password(name)?;
            Ok(password)
        }
        Err(keyring::Error::NoEntry) => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn save_password(name: &str, password: &str) -> Result<(), String> {
    let _lock = password_lock()?;
    let mut passwords = passwords()?;
    passwords.insert(name.to_owned(), password.to_owned());
    save_passwords(&passwords)
}

pub fn delete_password(name: &str) -> Result<(), String> {
    let _lock = password_lock()?;
    let mut passwords = passwords()?;
    delete_legacy_password(name)?;
    if passwords.remove(name).is_some() {
        save_passwords(&passwords)?;
    }
    Ok(())
}

fn delete_legacy_password(name: &str) -> Result<(), String> {
    match keyring::Entry::new("SqlDesk", name)
        .map_err(|e| e.to_string())?
        .delete_credential()
    {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn password_path() -> Result<PathBuf, String> {
    let path = path()?.with_file_name("passwords.dat");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    Ok(path)
}

fn password_lock() -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(password_path()?.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err("PASSWORDS_BUSY".into()),
        Err(error) => Err(error.to_string()),
    }
}

fn passwords() -> Result<BTreeMap<String, String>, String> {
    let path = password_path()?;
    let read = |path: &PathBuf| -> Result<BTreeMap<String, String>, String> {
        if !path.exists() { return Ok(BTreeMap::new()); }
        if fs::metadata(path).map_err(|e| e.to_string())?.len() > 1024 * 1024 { return Err("Encrypted password file exceeds 1 MiB".into()); }
        let encrypted = fs::read(path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&protect(&encrypted, false)?).map_err(|e| e.to_string())
    };
    let mut stored = read(&path)?;
    let marker = path.with_file_name("myquery-passwords-migrated");
    if !marker.exists() {
        let legacy = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("passwords.dat");
        if legacy != path {
            for (name, password) in read(&legacy)? { stored.entry(name).or_insert(password); }
            if legacy.exists() { save_passwords(&stored)?; }
        }
        fs::write(marker, b"1").map_err(|e| e.to_string())?;
    }
    Ok(stored)
}

fn save_passwords(passwords: &BTreeMap<String, String>) -> Result<(), String> {
    let plain = serde_json::to_vec(passwords).map_err(|e| e.to_string())?;
    if plain.len() > 512 * 1024 {
        return Err("Password data exceeds 512 KiB".into());
    }
    let encrypted = protect(&plain, true)?;
    let path = password_path()?;
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    temp.write_all(&encrypted).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data
            .len()
            .try_into()
            .map_err(|_| "Password data too large")?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI allocates the output; LocalFree releases it after copying.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let result = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        if !encrypt {
            for index in 0..output.cbData as usize {
                output.pbData.add(index).write_volatile(0);
            }
        }
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::protect;
    #[test]
    fn encrypted_password_data_round_trips_and_rejects_corruption() {
        let plain = "{\"连接\":\"secret-密码\"}".as_bytes();
        let mut encrypted = protect(plain, true).unwrap();
        assert_ne!(encrypted, plain);
        assert_eq!(protect(&encrypted, false).unwrap(), plain);
        let last = encrypted.len() - 1;
        encrypted[last] ^= 1;
        assert!(protect(&encrypted, false).is_err());
    }
}
