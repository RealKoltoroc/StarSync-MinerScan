use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCache {
    Uex,
    StarCitizenTools,
    StarCitizenWikiApi,
}

impl ProviderCache {
    pub fn folder_name(self) -> &'static str {
        match self {
            Self::Uex => "uex",
            Self::StarCitizenTools => "starcitizen-tools",
            Self::StarCitizenWikiApi => "star-citizen-wiki-api",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Uex => "UEX",
            Self::StarCitizenTools => "StarCitizen.Tools",
            Self::StarCitizenWikiApi => "api.star-citizen.wiki",
        }
    }
}

pub fn cache_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("StarSync MinerScan").join("cache")
}

pub fn provider_cache_dir(provider: ProviderCache) -> PathBuf {
    cache_root().join(provider.folder_name())
}

pub fn ensure_provider_cache(provider: ProviderCache) -> io::Result<PathBuf> {
    let path = provider_cache_dir(provider);
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn clear_provider_cache(provider: ProviderCache) -> io::Result<()> {
    let path = provider_cache_dir(provider);
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    fs::create_dir_all(&path)
}

pub fn clear_all_provider_caches() -> io::Result<()> {
    let root = cache_root();
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)
}

pub fn provider_cache_size(provider: ProviderCache) -> u64 {
    directory_size(&provider_cache_dir(provider)).unwrap_or(0)
}

pub fn read_cached_text(
    provider: ProviderCache,
    cache_key: &str,
) -> anyhow::Result<Option<String>> {
    let path = provider_cache_dir(provider).join(format!("{:016x}.cache", stable_hash(cache_key)));
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    Ok(Some(String::from_utf8(bytes)?))
}

pub fn get_or_fetch_text<F>(
    provider: ProviderCache,
    cache_key: &str,
    ttl: Duration,
    fetch: F,
) -> anyhow::Result<String>
where
    F: FnOnce() -> anyhow::Result<String>,
{
    let bytes = get_or_fetch_bytes(provider, cache_key, ttl, || Ok(fetch()?.into_bytes()))?;
    String::from_utf8(bytes).map_err(Into::into)
}

pub fn get_or_fetch_bytes<F>(
    provider: ProviderCache,
    cache_key: &str,
    ttl: Duration,
    fetch: F,
) -> anyhow::Result<Vec<u8>>
where
    F: FnOnce() -> anyhow::Result<Vec<u8>>,
{
    let dir = ensure_provider_cache(provider)?;
    let path = dir.join(format!("{:016x}.cache", stable_hash(cache_key)));
    let cached = fs::read(&path).ok();
    let fresh_enough = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| age <= ttl)
        .unwrap_or(false);
    if fresh_enough {
        if let Some(cached) = cached.clone() {
            return Ok(cached);
        }
    }

    match fetch() {
        Ok(fresh) => {
            fs::write(path, &fresh)?;
            Ok(fresh)
        }
        Err(error) => {
            // A stale provider cache remains a valid offline fallback until the user
            // explicitly clears it in Settings.
            if let Some(cached) = cached {
                Ok(cached)
            } else {
                Err(error)
            }
        }
    }
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn directory_size(path: &PathBuf) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::ProviderCache;

    #[test]
    fn provider_folders_are_stable() {
        assert_eq!(ProviderCache::Uex.folder_name(), "uex");
        assert_eq!(
            ProviderCache::StarCitizenTools.folder_name(),
            "starcitizen-tools"
        );
        assert_eq!(
            ProviderCache::StarCitizenWikiApi.folder_name(),
            "star-citizen-wiki-api"
        );
    }
}
