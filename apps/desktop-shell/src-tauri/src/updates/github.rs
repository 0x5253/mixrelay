//! Public GitHub release metadata translated into the desktop update contract.

use mixrelay_desktop_core::updates::{
    Catalog, Download, MAX_RESPONSE_BYTES, Product, UpdateError, release_assets, stable_version,
};
use serde::Deserialize;
use std::collections::BTreeSet;

const RELEASES: &str = "https://github.com/0x5253/mixrelay/releases";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    html_url: String,
    published_at: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    browser_download_url: String,
    size: u64,
    digest: String,
}

pub(super) fn decode_release(bytes: &[u8], product: Product) -> Result<Catalog, UpdateError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(UpdateError::ResponseTooLarge);
    }
    let release: Release =
        serde_json::from_slice(bytes).map_err(|_| UpdateError::InvalidCatalog)?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or(UpdateError::InvalidCatalog)?;
    stable_version(version)?;
    if release.draft
        || release.prerelease
        || release.html_url != format!("{RELEASES}/tag/{}", release.tag_name)
    {
        return Err(UpdateError::InvalidCatalog);
    }
    // The support contract names every published file: installers and companions.
    let published = release_assets(version)?;
    let installers = published.iter().filter(|asset| asset.installer).count();
    let mut seen = BTreeSet::new();
    let mut downloads = Vec::new();
    for asset in release.assets {
        let known = published.iter().find(|known| known.name == asset.name);
        let sha256 = asset
            .digest
            .strip_prefix("sha256:")
            .ok_or(UpdateError::InvalidCatalog)?;
        if !seen.insert(asset.name.clone())
            || asset.state != "uploaded"
            || asset.size == 0
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || asset.browser_download_url
                != format!("{RELEASES}/download/{}/{}", release.tag_name, asset.name)
            || (known.is_none() && asset.name != "SHA256SUMS")
        {
            return Err(UpdateError::InvalidCatalog);
        }
        if let Some(installer) = known.filter(|known| known.installer) {
            downloads.push(Download::published(
                &installer.os,
                &installer.arch,
                asset.browser_download_url,
                sha256.into(),
                asset.size,
            )?);
        }
    }
    if downloads.len() != installers {
        return Err(UpdateError::InvalidCatalog);
    }
    Catalog::published(
        product,
        version.into(),
        release.published_at,
        Some(release.html_url),
        downloads,
    )
}
