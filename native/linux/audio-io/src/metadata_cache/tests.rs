use super::*;
use mixrelay_time::ManualClock;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct EnvironmentSource;
impl MetadataEnvironment for EnvironmentSource {
    fn snapshot(&self) -> Environment {
        Environment {
            roots: vec![PathBuf::from("/fixture")],
            locale: Some("ko".into()),
            theme_name: None,
        }
    }
}
struct Files(Arc<AtomicUsize>);
impl MetadataFiles for Files {
    fn lookup(&self, key: &Key, roots: &[PathBuf], locale: Option<&str>) -> Metadata {
        assert_eq!(roots, &[PathBuf::from("/fixture")]);
        assert_eq!(locale, Some("ko"));
        self.0.fetch_add(1, Ordering::Relaxed);
        Metadata {
            display_name: key.application_id.clone(),
            icon_data_url: None,
        }
    }
}
fn key(id: Option<String>) -> Key {
    Key {
        application_id: id,
        executable: None,
        icon_name: None,
        theme_name: None,
    }
}
#[test]
fn positive_and_negative_metadata_expire_at_the_deadline_without_sleeping() {
    let clock = ManualClock::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = Resolver::new(clock.clone(), EnvironmentSource, Files(Arc::clone(&calls)));
    for id in [Some("Player".into()), None] {
        assert_eq!(resolver.resolve(key(id.clone())).display_name, id);
        resolver.resolve(key(id));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    clock.advance(CACHE_LIFETIME.checked_sub(Duration::from_nanos(1)).unwrap());
    resolver.resolve(key(None));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    clock.advance(Duration::from_nanos(1));
    resolver.resolve(key(None));
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}
#[test]
fn cache_is_bounded_and_owners_do_not_share_entries() {
    let clock = ManualClock::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = Resolver::new(clock.clone(), EnvironmentSource, Files(Arc::clone(&calls)));
    for id in 0..=MAX_CACHE_ENTRIES {
        resolver.resolve(key(Some(id.to_string())));
    }
    assert_eq!(resolver.entries.lock().unwrap().len(), MAX_CACHE_ENTRIES);
    resolver.resolve(key(Some("0".into())));
    assert_eq!(calls.load(Ordering::Relaxed), MAX_CACHE_ENTRIES + 2);
    let other = Resolver::new(clock.clone(), EnvironmentSource, Files(Arc::clone(&calls)));
    other.resolve(key(Some("0".into())));
    assert_eq!(calls.load(Ordering::Relaxed), MAX_CACHE_ENTRIES + 3);
}
