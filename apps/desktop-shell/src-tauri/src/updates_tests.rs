use super::{MAX_RESPONSE_BYTES, UpdateError, read_response};

#[tokio::test]
async fn response_body_limit_checks_declared_and_streamed_size() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    for declared in [true, false] {
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
            let length = MAX_RESPONSE_BYTES + 1;
            let header = if declared {
                format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n")
            } else {
                format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{length:x}\r\n")
            };
            socket
                .write_all(header.as_bytes())
                .await
                .expect("fixture header");
            if !declared {
                // The client may close once it crosses the limit; that is the expected boundary.
                let _ = socket.write_all(&vec![b' '; length]).await;
                let _ = socket.write_all(b"\r\n0\r\n\r\n").await;
            }
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("fixture client")
            .get(format!("http://{address}"))
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

fn github_release() -> serde_json::Value {
    let assets = [
        "MixRelay-Installer-1.2.10.pkg", "MixRelay_1.2.10_amd64.deb",
        "MixRelay_1.2.10_arm64.deb", "mix-relay-1.2.10-1-x86_64.pkg.tar.zst",
        "MixRelay-1.2.10-1.x86_64.rpm", "MixRelay-1.2.10-1.aarch64.rpm",
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
    use mixrelay_desktop_core::updates::{Platform, Product, UpdateState, product_update};
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
    use mixrelay_desktop_core::updates::Product;
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
    use mixrelay_desktop_core::updates::Product;
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
