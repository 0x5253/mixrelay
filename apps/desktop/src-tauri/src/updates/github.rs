//! Public GitHub release metadata translated into the desktop update contract.

use mixrelay_desktop_application::updates::{
    Catalog, Download, MAX_RESPONSE_BYTES, Product, UpdateError, stable_version,
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
    let installers = [
        (
            format!("MixRelay-Installer-{version}.pkg"),
            "macos",
            "universal",
        ),
        (format!("MixRelay_{version}_amd64.deb"), "linux", "x64"),
        (format!("MixRelay_{version}_arm64.deb"), "linux", "arm64"),
        (
            format!("mix-relay-{version}-1-x86_64.pkg.tar.zst"),
            "linux",
            "x64",
        ),
    ];
    let companion = format!("MixRelay-StreamDeck-{version}.streamDeckPlugin");
    let mut seen = BTreeSet::new();
    let mut downloads = Vec::new();
    for asset in release.assets {
        let installer = installers.iter().find(|(name, _, _)| *name == asset.name);
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
            || (installer.is_none() && asset.name != companion && asset.name != "SHA256SUMS")
        {
            return Err(UpdateError::InvalidCatalog);
        }
        if let Some((_, os, arch)) = installer {
            downloads.push(Download::published(
                os,
                arch,
                asset.browser_download_url,
                sha256.into(),
                asset.size,
            )?);
        }
    }
    if downloads.len() != installers.len() {
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
