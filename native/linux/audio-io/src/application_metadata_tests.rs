use super::*;
use std::fs;

struct Fixture(mixrelay_test_support::FixtureDir);

impl Fixture {
    fn new() -> Self {
        Self(mixrelay_test_support::FixtureDir::new("icon").unwrap())
    }
    fn write(&self, path: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    fn roots(&self) -> Vec<PathBuf> {
        vec![self.0.path().to_owned()]
    }
    fn theme(&self, name: &str, directories: &str, inherits: &str) {
        self.write(
            &format!("icons/{name}/index.theme"),
            format!("[Icon Theme]\nName={name}\nDirectories={directories}\nInherits={inherits}\n")
                .as_bytes(),
        );
    }
}

fn key(id: Option<&str>, binary: Option<&str>, icon: Option<&str>) -> Key {
    Key {
        application_id: id.map(str::to_owned),
        executable: binary.map(str::to_owned),
        icon_name: icon.map(str::to_owned),
        theme_name: None,
    }
}

fn image() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 128, 0, 255])
            .unwrap();
    }
    bytes
}

#[test]
fn desktop_id_resolves_localized_name_and_png_without_executing_commands() {
    let fixture = Fixture::new();
    fixture.theme("hicolor", "48x48/apps", "");
    fixture.write("applications/org.example.Player.desktop", b"[Desktop Entry]\nType=Application\nName=Player\nName[ko]=\xED\x94\x8C\xEB\xA0\x88\xEC\x9D\xB4\xEC\x96\xB4\nIcon=player\nExec=touch /not-executed\n[Desktop Action Stop]\nName=Wrong\nIcon=wrong");
    fixture.write("icons/hicolor/48x48/apps/player.png", &image());
    let metadata = lookup(
        &key(Some("org.example.Player"), None, None),
        &fixture.roots(),
        Some("ko_KR"),
    );
    assert_eq!(metadata.display_name.as_deref(), Some("플레이어"));
    assert_eq!(
        metadata.icon_data_url,
        Some(format!(
            "data:image/png;base64,{}",
            STANDARD.encode(image())
        ))
    );
}

#[test]
fn binary_basename_and_explicit_pipewire_icon_are_supported() {
    let fixture = Fixture::new();
    fixture.write(
        "applications/player.desktop",
        b"[Desktop Entry]\nType=Application\nName=Player\nIcon=missing",
    );
    fixture.write("pixmaps/stream-icon.png", &image());
    let metadata = lookup(
        &key(None, Some("/usr/bin/player"), Some("stream-icon")),
        &fixture.roots(),
        None,
    );
    assert_eq!(metadata.display_name.as_deref(), Some("Player"));
    assert!(metadata.icon_data_url.is_some());
}

#[test]
fn fallback_does_not_invent_metadata_or_follow_relative_traversal() {
    let fixture = Fixture::new();
    fixture.write(
        "outside.desktop",
        b"[Desktop Entry]\nType=Application\nName=Wrong",
    );
    assert_eq!(
        lookup(
            &key(Some("../outside"), None, Some("../outside")),
            &fixture.roots(),
            None
        ),
        Metadata::default()
    );
    assert_eq!(
        lookup(&key(Some("missing"), None, None), &fixture.roots(), None),
        Metadata::default()
    );
}

#[test]
fn rejects_malformed_svg_oversized_files_and_extreme_png_dimensions() {
    let fixture = Fixture::new();
    let svg = fixture.write("icon.svg", b"<svg><invalid>");
    assert!(icon(svg.to_str().unwrap(), &[], None).is_none());
    let oversized = fixture.write("huge.png", &vec![1; MAX_FILE_BYTES + 1]);
    assert!(read_regular(&oversized).is_none());
    let mut bytes = image();
    bytes[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
    let bomb = fixture.write("bomb.png", &bytes);
    assert!(png(&bomb).is_none());
    assert!(read_regular(&fixture.0).is_none());
    let truncated = fixture.write("truncated.png", &image()[..24]);
    assert!(png(&truncated).is_none());
    let mut corrupted = image();
    corrupted[29] ^= 255; // IHDR CRC corruption is not a usable image.
    assert!(png(&fixture.write("corrupt.png", &corrupted)).is_none());
}

#[test]
fn svg_only_custom_theme_uses_inherited_icon_and_outputs_bounded_png() {
    let fixture = Fixture::new();
    fixture.theme("custom", "scalable/apps", "parent");
    fixture.theme("parent", "scalable/apps", "custom");
    fixture.write(
        "icons/parent/scalable/apps/player.svg",
        b"<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><rect width='64' height='64' fill='red'/></svg>",
    );
    let mut selected = key(Some("player"), None, None);
    selected.theme_name = Some("custom".into());
    let metadata = lookup(&selected, &fixture.roots(), None);
    let icon_data_url = metadata.icon_data_url.expect("inherited SVG icon");
    let encoded = icon_data_url
        .strip_prefix("data:image/png;base64,")
        .unwrap();
    let decoded = STANDARD.decode(encoded).unwrap();
    let path = fixture.write("rendered.png", &decoded);
    assert!(png(&path).is_some());
    assert_eq!(u32::from_be_bytes(decoded[16..20].try_into().unwrap()), 32);
    assert_eq!(u32::from_be_bytes(decoded[20..24].try_into().unwrap()), 32);
    assert!(icon("missing", &fixture.roots(), Some("custom")).is_none());
}

#[test]
fn scalable_svg_remains_reachable_after_many_index_directories() {
    let fixture = Fixture::new();
    let dirs = (0..128)
        .map(|index| format!("16x16/unused{index}"))
        .chain(std::iter::once("scalable/apps".into()))
        .collect::<Vec<_>>()
        .join(",");
    fixture.theme("hicolor", &dirs, "");
    fixture.write(
        "icons/hicolor/scalable/apps/scalable-only.svg",
        b"<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><circle cx='32' cy='32' r='30' fill='blue'/></svg>",
    );
    assert!(
        icon("scalable-only", &fixture.roots(), None)
            .is_some_and(|image| image.starts_with("data:image/png;base64,"))
    );
}

#[test]
fn malformed_svg_and_unsafe_theme_paths_fall_back_without_exposing_markup() {
    let fixture = Fixture::new();
    fixture.theme("custom", "../outside,scalable/apps", "../../outside");
    fixture.write("icons/custom/scalable/apps/bad.svg", b"<svg><invalid>");
    fixture.write("outside/bad.png", &image());
    assert!(icon("bad", &fixture.roots(), Some("custom")).is_none());
    assert!(icon("../outside", &fixture.roots(), Some("custom")).is_none());
    assert!(icon("bad", &fixture.roots(), Some("../../outside")).is_none());
}

#[cfg(feature = "native-lab")]
#[test]
fn svg_without_a_base_uri_does_not_load_external_files_or_network_images() {
    use std::{
        io::Write,
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    let fixture = Fixture::new();
    let referenced_file = fixture.write("private.png", &image());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let file_svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><image href='file://{}' width='64' height='64'/></svg>",
        referenced_file.display()
    );
    let network_svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><image href='http://127.0.0.1:{port}/private.png' width='64' height='64'/></svg>"
    );
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut client, _)) => {
                    let bytes = image();
                    let header =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", bytes.len());
                    client.write_all(header.as_bytes()).unwrap();
                    client.write_all(&bytes).unwrap();
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("unexpected local listener error: {error}"),
            }
        }
        false
    });
    for (name, source) in [("file.svg", file_svg), ("network.svg", network_svg)] {
        let path = fixture.write(name, source.as_bytes());
        let Some(url) = icon(path.to_str().unwrap(), &[], None) else {
            continue;
        };
        let bytes = STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(frame.color_type, png::ColorType::Rgba);
        assert!(
            !pixels[..frame.buffer_size()]
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 200
                    && pixel[1] > 90
                    && pixel[1] < 180
                    && pixel[2] < 20
                    && pixel[3] > 0),
            "SVG resolved an external image: {name}"
        );
    }
    assert!(!server.join().unwrap(), "SVG made a network request");
}

#[test]
fn desktop_parser_respects_sections_hidden_and_application_type() {
    assert_eq!(
        parse_desktop("[Desktop Entry]\nType=Link\nName=Wrong", None),
        None
    );
    assert_eq!(
        parse_desktop(
            "[Desktop Entry]\nType=Application\nHidden=true\nName=Wrong",
            None
        ),
        None
    );
    let parsed = parse_desktop(
        "[Desktop Entry]\nType=Application\nName=Default\nName[en]=English\nName[en_US]=American",
        Some("en_US"),
    );
    assert_eq!(parsed, Some((Some("American".into()), None)));
}

#[test]
fn xdg_root_precedence_keeps_user_metadata_first() {
    let user = Fixture::new();
    let system = Fixture::new();
    user.write(
        "applications/app.desktop",
        b"[Desktop Entry]\nType=Application\nName=User",
    );
    system.write(
        "applications/app.desktop",
        b"[Desktop Entry]\nType=Application\nName=System",
    );
    let metadata = lookup(
        &key(Some("app.desktop"), None, None),
        &[user.0.path().to_owned(), system.0.path().to_owned()],
        None,
    );
    assert_eq!(metadata.display_name.as_deref(), Some("User"));
    user.write(
        "applications/app.desktop",
        b"[Desktop Entry]\nType=Application\nHidden=true\nName=Deleted",
    );
    assert_eq!(
        lookup(
            &key(Some("app"), None, None),
            &[user.0.path().to_owned(), system.0.path().to_owned()],
            None
        ),
        Metadata::default()
    );
}

#[test]
fn identity_inputs_and_png_reads_have_size_limits() {
    assert_eq!(bounded(Some(&"x".repeat(513))), None);
    assert_eq!(bounded(Some("")), None);
    assert!(!safe_name("bad\nname"));
    assert!(!safe_name(".."));
    assert!(!safe_name(&"x".repeat(256)));
}

#[test]
fn gtk_settings_theme_name_is_bounded_and_only_read_from_settings_section() {
    assert_eq!(
        theme_from_settings(
            "[Other]\ngtk-icon-theme-name=wrong\n[Settings]\ngtk-icon-theme-name=custom\n"
        ),
        Some("custom".into())
    );
    assert_eq!(
        theme_from_settings("[Settings]\ngtk-icon-theme-name=../outside\n"),
        None
    );
}
