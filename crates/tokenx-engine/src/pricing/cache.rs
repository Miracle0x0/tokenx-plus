use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CACHE_TTL_SECS: u64 = 3600;

pub fn get_cache_path(cache_dir: &Path, filename: &str) -> PathBuf {
    cache_dir.join(filename)
}

#[derive(Serialize, Deserialize)]
pub struct CachedData<T> {
    pub timestamp: u64,
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
    match parse_cache(&fs::read(&canonical_path).ok()?).ok()? {
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
) -> Result<ParsedCache<T>, String> {
    let cached: CachedData<T> = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    if cached.timestamp > now {
        return Err("cache timestamp is later than the current system clock".to_string());
    }
    if now.saturating_sub(cached.timestamp) > CACHE_TTL_SECS {
        Ok(ParsedCache::Stale(cached.data))
    } else {
        Ok(ParsedCache::Fresh(cached.data))
    }
}

pub fn save_cache<T: Serialize>(
    cache_dir: &Path,
    filename: &str,
    data: &T,
) -> Result<(), std::io::Error> {
    fs::create_dir_all(cache_dir)?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs();

    let cached = CachedData {
        timestamp: now,
        data,
    };
    let content = serde_json::to_string(&cached)?;

    let final_path = get_cache_path(cache_dir, filename);
    // INVARIANT: All cache writes use atomic temp-file rename. NEVER delete
    // the canonical cache file before writing — a partial save or process
    // crash between delete and rename would lose the cache. The temp-file
    // pattern makes corruption-on-crash impossible.
    crate::fs_atomic::write_atomic(&final_path, content.as_bytes())
}
