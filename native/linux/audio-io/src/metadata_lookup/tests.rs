use super::*;
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Default)]
struct Files {
    bytes: BTreeMap<PathBuf, Vec<u8>>,
    images: BTreeMap<PathBuf, String>,
    reads: RefCell<Vec<PathBuf>>,
}
impl Files {
    fn text(&mut self, path: &str, text: &str) {
        self.bytes.insert(path.into(), text.as_bytes().to_vec());
    }
}
impl Catalog for Files {
    fn is_file(&self, path: &Path) -> bool {
        self.bytes.contains_key(path)
    }
    fn read_regular(&self, path: &Path) -> Option<Vec<u8>> {
        self.reads.borrow_mut().push(path.into());
        self.bytes.get(path).cloned()
    }
    fn icon_file(&self, path: &Path) -> Option<String> {
        self.images.get(path).cloned()
    }
}
fn key(theme: Option<&str>) -> Key {
    Key {
        application_id: Some("org.player".into()),
        executable: None,
        icon_name: None,
        theme_name: theme.map(str::to_owned),
    }
}
#[test]
fn localized_name_and_selected_theme_precede_inherited_icons_without_io() {
    let mut files = Files::default();
    files.text("/user/applications/org.player.desktop", "[Desktop Entry]\nType=Application\nName=Player\nName[ko]=플레이어\nIcon=player\nExec=never-executed");
    files.text(
        "/user/icons/custom/index.theme",
        "[Icon Theme]\nDirectories=32x32/apps,scalable/apps\nInherits=parent\n",
    );
    files.text(
        "/system/icons/parent/index.theme",
        "[Icon Theme]\nDirectories=32x32/apps\nInherits=custom\n",
    );
    files.images.insert(
        "/user/icons/custom/scalable/apps/player.svg".into(),
        "selected".into(),
    );
    files.images.insert(
        "/system/icons/parent/32x32/apps/player.png".into(),
        "inherited".into(),
    );
    let roots = [PathBuf::from("/user"), PathBuf::from("/system")];
    assert_eq!(
        lookup(&files, &key(Some("custom")), &roots, Some("ko_KR")),
        Metadata {
            display_name: Some("플레이어".into()),
            icon_data_url: Some("selected".into())
        }
    );
    files
        .images
        .remove(Path::new("/user/icons/custom/scalable/apps/player.svg"));
    assert_eq!(
        lookup(&files, &key(Some("custom")), &roots, Some("ko_KR"))
            .icon_data_url
            .as_deref(),
        Some("inherited")
    );
    files.images.clear();
    assert_eq!(
        lookup(&files, &key(Some("custom")), &roots, None).icon_data_url,
        None
    );
    assert!(
        files.reads.borrow().len() < 30,
        "theme cycles must terminate"
    );
}
#[test]
fn hidden_user_override_does_not_resurrect_system_name_and_traversal_is_rejected() {
    let mut files = Files::default();
    files.text(
        "/user/applications/org.player.desktop",
        "[Desktop Entry]\nType=Application\nHidden=true\nName=Hidden",
    );
    files.text(
        "/system/applications/org.player.desktop",
        "[Desktop Entry]\nType=Application\nName=System",
    );
    let roots = [PathBuf::from("/user"), PathBuf::from("/system")];
    assert_eq!(
        lookup(&files, &key(None), &roots, None),
        Metadata::default()
    );
    assert!(
        !files
            .reads
            .borrow()
            .iter()
            .any(|path| path.starts_with("/system/applications"))
    );
    let unsafe_key = Key {
        application_id: Some("../outside".into()),
        icon_name: Some("../outside".into()),
        ..key(None)
    };
    assert_eq!(
        lookup(&files, &unsafe_key, &roots, None),
        Metadata::default()
    );
    assert_eq!(
        theme_from_settings("[Settings]\ngtk-icon-theme-name=../outside"),
        None
    );
    assert_eq!(
        theme_from_settings("[Settings]\ngtk-icon-theme-name=custom"),
        Some("custom".into())
    );
    assert_eq!(bounded(Some(&"x".repeat(513))), None);
    assert_eq!(bounded(Some("")), None);
}
