use super::{MAX_RESPONSE_BYTES, UpdateError, read_response};
use mixrelay_desktop_core::updates::{Platform, Product, UpdateState, product_update};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Answers one HTTP request with `head` and then `body`, from a local fixture server.
async fn serve_once(head: String, body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("fixture connection");
        let mut request = [0; 1024];
        let received = socket.read(&mut request).await.expect("fixture request");
        assert!(
            received > 0,
            "client sent a request before the fixture response"
        );
        socket
            .write_all(head.as_bytes())
            .await
            .expect("fixture header");
        // The client may close once it has seen enough; that is an expected boundary.
        let _ = socket.write_all(&body).await;
    });
    (format!("http://{address}"), server)
}

/// The production client settings, without HTTPS, for the local fixture.
fn fixture_client() -> reqwest::Client {
    super::client_builder()
        .no_proxy()
        .build()
        .expect("fixture client")
}

#[tokio::test]
async fn response_body_limit_checks_declared_and_streamed_size() {
    let length = MAX_RESPONSE_BYTES + 1;
    for declared in [true, false] {
        let (head, body) = if declared {
            (
                format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n"),
                Vec::new(),
            )
        } else {
            let mut body = vec![b' '; length];
            body.extend_from_slice(b"\r\n0\r\n\r\n");
            (
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{length:x}\r\n"),
                body,
            )
        };
        let (url, server) = serve_once(head, body).await;
        let response = fixture_client()
            .get(url)
            .send()
            .await
            .expect("fixture response");
        assert!(matches!(
            read_response(response).await,
            Err(UpdateError::ResponseTooLarge)
        ));
        server.await.expect("fixture server");
    }
}

async fn fetch(head: &str, body: Vec<u8>) -> Result<super::Catalog, UpdateError> {
    let (url, server) = serve_once(head.to_owned(), body).await;
    let catalog = super::fetch_catalog(&fixture_client(), &url, Product::MixRelay).await;
    server.await.expect("fixture server");
    catalog
}

fn update(
    catalog: Result<super::Catalog, UpdateError>,
) -> mixrelay_desktop_core::updates::ProductUpdate {
    let platform = Platform::from_identity("macos", "aarch64", None).unwrap();
    product_update(Product::MixRelay, Some("1.2.9".into()), platform, catalog)
}

#[tokio::test]
async fn a_missing_release_means_nothing_is_published_yet() {
    let catalog = fetch(
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        Vec::new(),
    )
    .await;
    assert_eq!(update(catalog).state(), UpdateState::NotPublished);
}

#[tokio::test]
async fn redirects_and_server_errors_fail_the_check_instead_of_being_followed() {
    for head in [
        "HTTP/1.1 302 Found\r\nLocation: https://example.com/\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
    ] {
        assert!(
            matches!(fetch(head, Vec::new()).await, Err(UpdateError::Unavailable)),
            "{head}"
        );
    }
}

#[tokio::test]
async fn a_published_release_offers_the_newer_package_for_this_platform() {
    let body = serde_json::to_vec(&github_release()).unwrap();
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
    let update = update(fetch(&head, body).await);
    assert_eq!(update.state(), UpdateState::Available);
    assert_eq!(update.latest_version(), Some("1.2.10"));
}

#[tokio::test]
async fn opening_a_download_rejects_anything_but_a_plain_https_link_before_any_request() {
    for url in [
        "http://github.com/0x5253/mixrelay/releases/download/v1.2.10/MixRelay-Installer-1.2.10.pkg",
        "https://user@github.com/0x5253/mixrelay/releases/download/v1.2.10/a.pkg",
        "not a url",
    ] {
        assert!(matches!(
            super::updates_open_download(Product::MixRelay, url.into()).await,
            Err(UpdateError::InvalidDownload)
        ));
    }
}

fn github_release() -> serde_json::Value {
    let assets = [
        "MixRelay-Installer-1.2.10.pkg", "MixRelay_1.2.10_amd64.deb",
        "MixRelay_1.2.10_arm64.deb", "mix-relay-1.2.10-1-x86_64.pkg.tar.zst",
        "mix-relay-1.2.10-1-aarch64.pkg.tar.zst", "MixRelay-1.2.10-1.x86_64.rpm", "MixRelay-1.2.10-1.aarch64.rpm",
        "MixRelay-StreamDeck-1.2.10.streamDeckPlugin", "SHA256SUMS",
    ].map(|name| serde_json::json!({
        "name": name, "state": "uploaded", "size": 1024,
        "digest": format!("sha256:{}", "a".repeat(64)),
        "browser_download_url": format!("https://github.com/0x5253/mixrelay/releases/download/v1.2.10/{name}"),
    }));
    serde_json::json!({"tag_name":"v1.2.10", "draft":false,"prerelease":false,
        "html_url":"https://github.com/0x5253/mixrelay/releases/tag/v1.2.10",
        "published_at":"2026-11-02T00:00:00Z", "assets":assets})
}

#[test]
fn github_publication_selects_the_native_package_and_preserves_its_github_digest() {
    for (os, arch, distro, filename) in [
        ("macos", "aarch64", None, "MixRelay-Installer-1.2.10.pkg"),
        ("macos", "x86_64", None, "MixRelay-Installer-1.2.10.pkg"),
        (
            "linux",
            "x86_64",
            Some("ID=debian\n"),
            "MixRelay_1.2.10_amd64.deb",
        ),
        (
            "linux",
            "aarch64",
            Some("ID=debian\n"),
            "MixRelay_1.2.10_arm64.deb",
        ),
        (
            "linux",
            "x86_64",
            Some("ID=arch\n"),
            "mix-relay-1.2.10-1-x86_64.pkg.tar.zst",
        ),
        (
            "linux",
            "aarch64",
            Some("ID=archarm\nID_LIKE=arch\n"),
            "mix-relay-1.2.10-1-aarch64.pkg.tar.zst",
        ),
        (
            "linux",
            "x86_64",
            Some("ID=fedora\n"),
            "MixRelay-1.2.10-1.x86_64.rpm",
        ),
        (
            "linux",
            "aarch64",
            Some("ID=fedora\n"),
            "MixRelay-1.2.10-1.aarch64.rpm",
        ),
    ] {
        let catalog = super::github::decode_release(
            &serde_json::to_vec(&github_release()).unwrap(),
            Product::MixRelay,
        )
        .unwrap();
        let platform = Platform::from_identity(os, arch, distro).unwrap();
        let update = product_update(
            Product::MixRelay,
            Some("1.2.9".into()),
            platform,
            Ok(catalog),
        );
        assert_eq!(update.state(), UpdateState::Available);
        assert_eq!(update.latest_version(), Some("1.2.10"));
        assert_eq!(update.downloads().len(), 1);
        let actual = serde_json::to_value(&update.downloads()[0]).unwrap();
        assert_eq!(
            actual["url"],
            format!("https://github.com/0x5253/mixrelay/releases/download/v1.2.10/{filename}")
        );
        assert_eq!(actual["sha256"], "a".repeat(64));
        assert_eq!(actual["size"], 1024);
    }
}

#[test]
fn github_catalog_rejects_unstable_incomplete_or_misdirected_publications() {
    use serde_json::json;
    for (pointer, value) in [
        ("/tag_name", json!("1.2.10")),
        ("/tag_name", json!("v1.2.10-beta")),
        ("/tag_name", json!("v1.2.10+build")),
        ("/tag_name", json!("v01.2.10")),
        ("/draft", json!(true)),
        ("/prerelease", json!(true)),
        ("/published_at", json!("")),
        (
            "/html_url",
            json!("https://github.com/another/mixrelay/releases/tag/v1.2.10"),
        ),
        (
            "/assets/0/browser_download_url",
            json!(
                "https://github.com/another/mixrelay/releases/download/v1.2.10/MixRelay-Installer-1.2.10.pkg"
            ),
        ),
        (
            "/assets/0/browser_download_url",
            json!(
                "https://github.com/0x5253/mixrelay/releases/download/v1.2.9/MixRelay-Installer-1.2.10.pkg"
            ),
        ),
        (
            "/assets/0/browser_download_url",
            json!(
                "https://github.com/0x5253/mixrelay/releases/download/v1.2.10/MixRelay-Installer-1.2.10.pkg?redirect=evil"
            ),
        ),
        (
            "/assets/0/browser_download_url",
            json!(
                "http://github.com/0x5253/mixrelay/releases/download/v1.2.10/MixRelay-Installer-1.2.10.pkg"
            ),
        ),
        (
            "/assets/0/browser_download_url",
            json!(
                "https://user@github.com/0x5253/mixrelay/releases/download/v1.2.10/MixRelay-Installer-1.2.10.pkg"
            ),
        ),
        ("/assets/0/name", json!("MixRelay-Installer-1.2.10.exe")),
        ("/assets/1/name", json!("MixRelay_1.2.10_x64.deb")),
        ("/assets/0/state", json!("starter")),
        ("/assets/0/size", json!(0)),
        ("/assets/0/size", json!(-1)),
        ("/assets/0/size", json!(1.5)),
        ("/assets/0/digest", json!(null)),
        ("/assets/0/digest", json!("sha256:abc")),
        (
            "/assets/0/digest",
            json!(format!("sha512:{}", "a".repeat(64))),
        ),
        (
            "/assets/0/digest",
            json!(format!("sha256:{}", "g".repeat(64))),
        ),
    ] {
        let mut release = github_release();
        *release.pointer_mut(pointer).unwrap() = value;
        assert!(
            super::github::decode_release(
                &serde_json::to_vec(&release).unwrap(),
                Product::MixRelay
            )
            .is_err(),
            "{pointer}: {release}"
        );
    }
}

#[test]
fn github_catalog_rejects_duplicate_missing_or_oversized_assets() {
    for duplicate in [false, true] {
        let mut release = github_release();
        let assets = release["assets"].as_array_mut().unwrap();
        if duplicate {
            assets.push(assets[0].clone());
        } else {
            assets.remove(0);
        }
        assert!(
            super::github::decode_release(
                &serde_json::to_vec(&release).unwrap(),
                Product::MixRelay
            )
            .is_err()
        );
    }
    assert!(matches!(
        super::github::decode_release(&vec![b' '; MAX_RESPONSE_BYTES + 1], Product::MixRelay),
        Err(UpdateError::ResponseTooLarge)
    ));
}
