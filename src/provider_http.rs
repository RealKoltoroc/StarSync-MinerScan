use std::time::Duration;

use crate::provider_cache::{ProviderCache, get_or_fetch_bytes, get_or_fetch_text};

pub fn cached_get_text(
    provider: ProviderCache,
    url: &str,
    ttl: Duration,
) -> anyhow::Result<String> {
    get_or_fetch_text(provider, url, ttl, || {
        let response = ureq::get(url).call()?;
        let mut body = response.into_body();
        let text = body.read_to_string()?;
        Ok(text)
    })
}

pub fn cached_get_bytes(
    provider: ProviderCache,
    url: &str,
    ttl: Duration,
) -> anyhow::Result<Vec<u8>> {
    get_or_fetch_bytes(provider, url, ttl, || {
        let response = ureq::get(url).call()?;
        let mut body = response.into_body();
        Ok(body.read_to_vec()?)
    })
}
