//! Native release transport and desktop commands.
use mixrelay_desktop_core::updates::{
    self, Catalog, CatalogSource, DownloadOpener, MAX_RESPONSE_BYTES, Platform, Product,
    ProductUpdate, UpdateError,
};
use reqwest::{Client, ClientBuilder, StatusCode};
use std::time::Duration;

mod github;

const RELEASE_ENDPOINT: &str = "https://api.github.com/repos/0x5253/mixrelay/releases/latest";

fn platform() -> Result<Platform, UpdateError> {
    let release = if std::env::consts::OS == "linux" {
        Some(std::fs::read_to_string("/etc/os-release").map_err(|_| UpdateError::Unavailable)?)
    } else {
        None
    };
    Platform::from_identity(
        std::env::consts::OS,
        std::env::consts::ARCH,
        release.as_deref(),
    )
}

/// Bounded requests that never retry or follow a redirect.
fn client_builder() -> ClientBuilder {
    Client::builder()
        .user_agent(concat!("MixRelay/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
}

fn client() -> Result<Client, UpdateError> {
    client_builder()
        .https_only(true)
        .build()
        .map_err(|_| UpdateError::Unavailable)
}

/// Reads the latest publication. A missing release means nothing is published yet; any
/// other status but OK, a redirect included, fails the check.
async fn fetch_catalog(
    client: &Client,
    endpoint: &str,
    product: Product,
) -> Result<Catalog, UpdateError> {
    let response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await
        .map_err(|_| UpdateError::Unavailable)?;
    match response.status() {
        StatusCode::OK => github::decode_release(&read_response(response).await?, product),
        StatusCode::NOT_FOUND => Ok(Catalog::unpublished(product)),
        _ => Err(UpdateError::Unavailable),
    }
}

async fn read_response(mut response: reqwest::Response) -> Result<Vec<u8>, UpdateError> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(UpdateError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| UpdateError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return Err(UpdateError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct PublicCatalog {
    client: Client,
}

impl CatalogSource for PublicCatalog {
    async fn fetch(&self, product: Product, _platform: Platform) -> Result<Catalog, UpdateError> {
        fetch_catalog(&self.client, RELEASE_ENDPOINT, product).await
    }
}

struct NativeBrowser;

impl DownloadOpener for NativeBrowser {
    async fn open(&self, url: &str) -> Result<(), UpdateError> {
        let url = url.to_owned();
        tauri::async_runtime::spawn_blocking(move || open::that(url))
            .await
            .map_err(|_| UpdateError::OpenFailed)?
            .map_err(|_| UpdateError::OpenFailed)
    }
}

/// Compares `installed` with the latest publication for this machine's platform.
async fn check(product: Product, installed: Option<String>) -> ProductUpdate {
    let (Ok(platform), Ok(client)) = (platform(), client()) else {
        return updates::unavailable_product(product, installed);
    };
    let catalog = fetch_catalog(&client, RELEASE_ENDPOINT, product).await;
    updates::product_update(product, installed, platform, catalog)
}

/// Checks the one complete `MixRelay` release against the installed Driver package.
pub(crate) async fn updates_check_driver() -> ProductUpdate {
    let installed =
        tauri::async_runtime::spawn_blocking(crate::driver_control::installed_mixrelay_version)
            .await
            .ok()
            .flatten();
    check(Product::MixRelay, installed).await
}

/// Checks the complete product without transmitting devices or audio state.
#[tauri::command]
pub(crate) async fn updates_check(app: tauri::AppHandle) -> Vec<ProductUpdate> {
    let installed = Some(app.package_info().version.to_string());
    vec![check(Product::MixRelay, installed).await]
}

/// Opens a user-selected package after checking the current publication.
#[tauri::command]
pub(crate) async fn updates_open_download(
    product: Product,
    url: String,
) -> Result<(), UpdateError> {
    if !updates::https_url(&url) {
        return Err(UpdateError::InvalidDownload);
    }
    updates::open_download(
        &PublicCatalog { client: client()? },
        &NativeBrowser,
        product,
        platform()?,
        &url,
    )
    .await
}

#[cfg(test)]
mod tests;
