use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CACHE_TTL_SECS: u64 = 3600;
pub const CACHE_FORMAT_VERSION: u32 = 2;

pub fn get_cache_path(cache_dir: &Path, filename: &str) -> PathBuf {
    cache_dir.join(filename)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedData<T> {
    pub version: u32,
    pub data: T,
}

pub(crate) enum ParsedCache<T> {
    Fresh(T),
    Stale(T),
}

fn load_cache_with_policy<T: for<'de> Deserialize<'de>>(
    cache_dir: &Path,
    filename: &str,
    allow_stale: bool,
) -> Option<T> {
    let canonical_path = get_cache_path(cache_dir, filename);
    let mut file = fs::File::open(&canonical_path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
    std::io::Read::read_to_end(&mut file, &mut bytes).ok()?;
    match parse_cache(&bytes, modified).ok()? {
        ParsedCache::Fresh(data) => Some(data),
        ParsedCache::Stale(data) if allow_stale => Some(data),
        ParsedCache::Stale(_) => None,
    }
}

pub fn load_cache<T: for<'de> Deserialize<'de>>(cache_dir: &Path, filename: &str) -> Option<T> {
    load_cache_with_policy(cache_dir, filename, false)
}

pub fn load_cache_any_age<T: for<'de> Deserialize<'de>>(
    cache_dir: &Path,
    filename: &str,
) -> Option<T> {
    load_cache_with_policy(cache_dir, filename, true)
}

pub(crate) fn parse_cache<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    modified: SystemTime,
) -> Result<ParsedCache<T>, String> {
    let cached: CachedData<T> = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if cached.version != CACHE_FORMAT_VERSION {
        return Err(format!(
            "unsupported pricing cache version: {}",
            cached.version
        ));
    }
    if is_fresh_at(modified, SystemTime::now())? {
        Ok(ParsedCache::Fresh(cached.data))
    } else {
        Ok(ParsedCache::Stale(cached.data))
    }
}

pub(crate) fn is_fresh_at(modified: SystemTime, now: SystemTime) -> Result<bool, String> {
    let age = now
        .duration_since(modified)
        .map_err(|_| "cache timestamp is later than the current system clock".to_string())?;
    Ok(age.as_secs() <= CACHE_TTL_SECS)
}

pub fn save_cache<T: Serialize>(
    cache_dir: &Path,
    filename: &str,
    data: &T,
) -> Result<(), std::io::Error> {
    fs::create_dir_all(cache_dir)?;

    let cached = CachedData {
        version: CACHE_FORMAT_VERSION,
        data,
    };
    let content = serde_json::to_vec(&cached)?;
    let final_path = get_cache_path(cache_dir, filename);
    match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&final_path)
    {
        Ok(mut file) => {
            if file.metadata()?.len() == content.len() as u64 {
                let mut previous = Vec::with_capacity(content.len());
                std::io::Read::read_to_end(&mut file, &mut previous)?;
                let current: serde_json::Value = serde_json::from_slice(&content)?;
                if serde_json::from_slice::<serde_json::Value>(&previous)
                    .is_ok_and(|previous| previous == current)
                {
                    // Freshness belongs to this exact inode. A concurrent atomic
                    // replacement must not receive a timestamp for other data.
                    file.set_modified(SystemTime::now())?;
                    file.sync_all()?;
                    return Ok(());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    crate::fs_atomic::write_atomic(&final_path, &content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn unchanged_data_only_renews_freshness_without_replacing_the_file() {
        let root = tempfile::TempDir::new().unwrap();
        let path = root.path().join("catalog.json");
        let previous = br#"{"data":{"b":2,"a":1},"version":2}"#;
        fs::write(&path, previous).unwrap();
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        assert!(load_cache::<serde_json::Value>(root.path(), "catalog.json").is_none());
        let old_identity =
            crate::input_record_cache::input_file_identity_from_open_file(&file).unwrap();
        save_cache(
            root.path(),
            "catalog.json",
            &serde_json::json!({"a":1,"b":2}),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), previous);
        let current = fs::File::open(&path).unwrap();
        assert_eq!(
            crate::input_record_cache::input_file_identity_from_open_file(&current).unwrap(),
            old_identity
        );
        assert!(load_cache::<serde_json::Value>(root.path(), "catalog.json").is_some());
    }

    #[test]
    fn changed_data_atomically_replaces_the_previous_snapshot() {
        use std::io::Read;
        let root = tempfile::TempDir::new().unwrap();
        save_cache(root.path(), "catalog.json", &serde_json::json!({"rate":1})).unwrap();
        let mut old = fs::File::open(root.path().join("catalog.json")).unwrap();
        save_cache(root.path(), "catalog.json", &serde_json::json!({"rate":2})).unwrap();
        let mut captured = String::new();
        old.read_to_string(&mut captured).unwrap();
        assert_eq!(
            serde_json::from_str::<CachedData<serde_json::Value>>(&captured)
                .unwrap()
                .data,
            serde_json::json!({"rate":1})
        );
        assert_eq!(
            load_cache::<serde_json::Value>(root.path(), "catalog.json").unwrap(),
            serde_json::json!({"rate":2})
        );
    }

    #[test]
    fn rejects_old_envelopes_and_future_freshness() {
        let now = SystemTime::now();
        assert!(
            parse_cache::<serde_json::Value>(br#"{"version":1,"timestamp":1,"data":{}}"#, now)
                .is_err()
        );
        assert!(parse_cache::<serde_json::Value>(
            br#"{"version":2,"data":{}}"#,
            now + Duration::from_secs(30)
        )
        .is_err());
        assert_eq!(is_fresh_at(now - Duration::from_secs(3601), now), Ok(false));
    }
}
