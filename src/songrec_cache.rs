//! Simple file-based cache for songrec lookups.
//!
//! Caches the raw songrec JSON response keyed by a SHA-256 hash of the WAV
//! segment content.  The cache lives in `~/.cache/songrec.cache` as a plain
//! text file with one entry per line:  `<hex-hash> <json>`

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// Return the path to the cache file (`~/.cache/songrec.cache`).
fn cache_path() -> Option<PathBuf> {
    dirs_hint().map(|dir| dir.join("songrec.cache"))
}

/// Best-effort `~/.cache` directory.
fn dirs_hint() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
}

/// Compute a simple hash of a byte slice.
/// Uses a 64-bit FNV-1a hash (good enough for cache keys, no crypto needed).
fn hash_bytes(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Load the full cache from disk into a HashMap.
pub fn load_cache() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let path = match cache_path() {
        Some(p) => p,
        None => return map,
    };
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return map,
    };
    for line in BufReader::new(file).lines() {
        if let Ok(line) = line {
            // Format: "<hash> <json...>"
            if let Some(idx) = line.find(' ') {
                let key = line[..idx].to_string();
                let value = line[idx + 1..].to_string();
                map.insert(key, value);
            }
        }
    }
    map
}

/// Append a single entry to the cache file.
pub fn append_to_cache(key: &str, json: &str) {
    let path = match cache_path() {
        Some(p) => p,
        None => return,
    };
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        // Store on a single line — collapse any newlines in the JSON
        let one_line = json.replace('\n', " ").replace('\r', "");
        let _ = writeln!(f, "{} {}", key, one_line);
    }
}

/// Look up a WAV file in the cache by hashing its contents.
/// Returns `Some(json_string)` on cache hit, `None` on miss.
pub fn lookup(wav_path: &str, cache: &HashMap<String, String>) -> Option<String> {
    let data = fs::read(wav_path).ok()?;
    let key = hash_bytes(&data);
    cache.get(&key).cloned()
}

/// Compute the cache key for a WAV file (hash of its contents).
pub fn cache_key(wav_path: &str) -> Option<String> {
    let data = fs::read(wav_path).ok()?;
    Some(hash_bytes(&data))
}
