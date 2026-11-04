//! Public release checks and user-requested download links, outside the audio lifecycle.

use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

mod package;
use package::Package;
pub use package::ReleaseAsset;

pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Independently distributed products accepted by the native update commands.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Product {
    /// The desktop application.
    MixRelay,
}

/// Release observations; absent or failed publication is never reported as current.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Available,
    Current,
    Published,
    NotPublished,
    Unavailable,
}

/// A publisher-provided installation package for this machine's platform.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Download {
    os: String,
    arch: String,
    url: String,
    sha256: String,
    size: u64,
}

impl Download {
    /// Constructs a validated package independently of its publication provider.
    /// # Errors
    /// Rejects unsupported platform labels, unsafe URLs, invalid hashes and empty assets.
    pub fn published(
        os: &str,
        arch: &str,
        url: String,
        sha256: String,
        size: u64,
    ) -> Result<Self, UpdateError> {
        if !matches!(
            (os, arch),
            ("macos", "arm64" | "x64" | "universal") | ("linux", "arm64" | "x64")
        ) {
            return Err(UpdateError::InvalidCatalog);
        }
        let download = Self {
            os: os.into(),
            arch: arch.into(),
            url,
            sha256,
            size,
        };
        validate_download(&download)?;
        Ok(download)
    }

    /// Returns the published link; opening it still requires catalog revalidation.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// One product's installed version and independently observed publication state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUpdate {
    product: Product,
    installed_version: Option<String>,
    latest_version: Option<String>,
    state: UpdateState,
    downloads: Vec<Download>,
    notes_url: Option<String>,
}

impl ProductUpdate {
    /// Returns the publication state, including failures and unpublished products.
    #[must_use]
    pub const fn state(&self) -> UpdateState {
        self.state
    }

    /// Returns the publisher's latest stable release version when one is available.
    #[must_use]
    pub fn latest_version(&self) -> Option<&str> {
        self.latest_version.as_deref()
    }

    /// Returns validated packages for the current platform.
    #[must_use]
    pub fn downloads(&self) -> &[Download] {
        &self.downloads
    }
}

/// Failures while reading authoritative catalogs or opening a validated package link.
#[derive(Debug, Error, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateError {
    /// The public service could not return a successful bounded response.
    #[error("The update service is unavailable")]
    Unavailable,
    /// The catalog does not describe a valid published release for this machine.
    #[error("The update catalog is invalid")]
    InvalidCatalog,
    /// The response exceeded the catalog size limit.
    #[error("The update catalog is too large")]
    ResponseTooLarge,
    /// The supplied URL is unsafe or absent from the current platform catalog.
    #[error("The download is no longer in the current update catalog")]
    InvalidDownload,
    /// The operating system could not open the validated download link.
    #[error("The download link could not be opened")]
    OpenFailed,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    product: Product,
    // An explicit null is a valid empty catalog; a missing field is a malformed response.
    latest: LatestVersion,
    releases: Vec<Release>,
}

impl Catalog {
    /// Represents a provider with no published stable release.
    #[must_use]
    pub fn unpublished(product: Product) -> Self {
        Self {
            product,
            latest: LatestVersion::Empty(()),
            releases: Vec::new(),
        }
    }

    /// Constructs one stable publication without exposing provider-specific DTOs.
    /// # Errors
    /// Rejects invalid versions, missing publication data and malformed packages.
    pub fn published(
        product: Product,
        version: String,
        published_at: String,
        notes_url: Option<String>,
        downloads: Vec<Download>,
    ) -> Result<Self, UpdateError> {
        stable_version(&version)?;
        if published_at.trim().is_empty()
            || downloads.is_empty()
            || notes_url.as_deref().is_some_and(|url| !https_url(url))
        {
            return Err(UpdateError::InvalidCatalog);
        }
        for download in &downloads {
            validate_download(download)?;
        }
        Ok(Self {
            product,
            latest: LatestVersion::Published(version.clone()),
            releases: vec![Release {
                version,
                published_at,
                notes_url,
                downloads,
            }],
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LatestVersion {
    Published(String),
    Empty(()),
}

#[derive(Debug, Deserialize)]
struct Release {
    version: String,
    published_at: String,
    notes_url: Option<String>,
    downloads: Vec<Download>,
}

#[derive(Clone, Copy, Debug)]
pub struct Platform {
    os: &'static str,
    arch: &'static str,
    package: Package,
}

impl Platform {
    /// Resolves a supported package from the supplied native platform identity.
    /// # Errors
    /// Rejects unsupported operating systems, architectures, and distributions.
    pub fn from_identity(
        os: &str,
        arch: &str,
        os_release: Option<&str>,
    ) -> Result<Self, UpdateError> {
        let os = match os {
            "macos" => "macos",
            "linux" => "linux",
            _ => return Err(UpdateError::Unavailable),
        };
        let arch = match arch {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            _ => return Err(UpdateError::Unavailable),
        };
        let package = if os == "linux" {
            package::linux(os_release.ok_or(UpdateError::Unavailable)?, arch)?
        } else {
            Package::Pkg
        };
        Ok(Self { os, arch, package })
    }

    fn accepts(self, download: &Download) -> bool {
        download.os == self.os
            && (download.arch == self.arch || (self.os == "macos" && download.arch == "universal"))
            && self.package.accepts(&download.url)
    }
}

/// Every file the support contract publishes for one release version.
/// # Errors
/// Rejects a version that is not a stable release version.
pub fn release_assets(version: &str) -> Result<Vec<ReleaseAsset>, UpdateError> {
    stable_version(version)?;
    package::release_assets(version)
}

/// # Errors
/// Rejects malformed, noncanonical, prerelease, and build-suffixed versions.
pub fn stable_version(value: &str) -> Result<Version, UpdateError> {
    let version = Version::parse(value).map_err(|_| UpdateError::InvalidCatalog)?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != value {
        return Err(UpdateError::InvalidCatalog);
    }
    Ok(version)
}

#[must_use]
pub fn https_url(value: &str) -> bool {
    !value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn validate_download(download: &Download) -> Result<(), UpdateError> {
    if !https_url(&download.url)
        || download.sha256.len() != 64
        || !download.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || download.size == 0
    {
        return Err(UpdateError::InvalidCatalog);
    }
    Ok(())
}

fn published_release(
    catalog: Catalog,
    product: Product,
    platform: Platform,
) -> Result<Option<Release>, UpdateError> {
    if catalog.product != product {
        return Err(UpdateError::InvalidCatalog);
    }
    let latest = match catalog.latest {
        LatestVersion::Empty(()) if catalog.releases.is_empty() => return Ok(None),
        LatestVersion::Empty(()) => return Err(UpdateError::InvalidCatalog),
        LatestVersion::Published(latest) => latest,
    };
    stable_version(&latest)?;
    let mut release = catalog
        .releases
        .into_iter()
        .find(|release| release.version == latest)
        .ok_or(UpdateError::InvalidCatalog)?;
    if release.published_at.trim().is_empty()
        || release
            .notes_url
            .as_deref()
            .is_some_and(|url| !https_url(url))
    {
        return Err(UpdateError::InvalidCatalog);
    }
    release
        .downloads
        .retain(|download| platform.accepts(download));
    if release.downloads.is_empty() {
        return Err(UpdateError::InvalidCatalog);
    }
    for download in &release.downloads {
        validate_download(download)?;
    }
    Ok(Some(release))
}

#[must_use]
pub fn product_update(
    product: Product,
    installed_version: Option<String>,
    platform: Platform,
    catalog: Result<Catalog, UpdateError>,
) -> ProductUpdate {
    let mut update = unavailable_product(product, installed_version);
    match catalog.and_then(|catalog| published_release(catalog, product, platform)) {
        Ok(None) => update.state = UpdateState::NotPublished,
        Ok(Some(release)) => {
            let state = match update.installed_version.as_deref() {
                None => UpdateState::Published,
                Some(installed) => {
                    match (stable_version(installed), stable_version(&release.version)) {
                        (Ok(installed), Ok(latest)) if installed >= latest => UpdateState::Current,
                        (Ok(_), Ok(_)) => UpdateState::Available,
                        _ => return update,
                    }
                }
            };
            update.state = state;
            update.latest_version = Some(release.version);
            update.downloads = release.downloads;
            update.notes_url = release.notes_url;
        }
        Err(_) => {}
    }
    update
}

#[must_use]
pub fn unavailable_product(product: Product, installed_version: Option<String>) -> ProductUpdate {
    ProductUpdate {
        product,
        installed_version,
        latest_version: None,
        state: UpdateState::Unavailable,
        downloads: Vec::new(),
        notes_url: None,
    }
}

/// # Errors
/// Rejects absent, invalid, or stale packages for the requested platform.
pub fn authorize_download(
    catalog: Catalog,
    product: Product,
    platform: Platform,
    url: &str,
) -> Result<(), UpdateError> {
    let release =
        published_release(catalog, product, platform)?.ok_or(UpdateError::InvalidDownload)?;
    if !release.downloads.iter().any(|download| download.url == url) {
        return Err(UpdateError::InvalidDownload);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Reads the current authoritative publication without changing audio state.
pub trait CatalogSource {
    /// Returns one catalog for the requested product and qualified platform.
    fn fetch(
        &self,
        product: Product,
        platform: Platform,
    ) -> impl std::future::Future<Output = Result<Catalog, UpdateError>> + Send;
}

/// Opens a link through the operating system after catalog authorization.
pub trait DownloadOpener {
    /// Delegates one already-authorized HTTPS link to the browser.
    fn open(&self, url: &str) -> impl std::future::Future<Output = Result<(), UpdateError>> + Send;
}

/// Revalidates a link against the current catalog before opening it.
/// # Errors
/// Returns catalog or browser failures, and rejects unsafe or unpublished links.
pub async fn open_download(
    source: &impl CatalogSource,
    opener: &impl DownloadOpener,
    product: Product,
    platform: Platform,
    url: &str,
) -> Result<(), UpdateError> {
    if !https_url(url) {
        return Err(UpdateError::InvalidDownload);
    }
    let catalog = source.fetch(product, platform).await?;
    authorize_download(catalog, product, platform, url)?;
    opener.open(url).await
}
