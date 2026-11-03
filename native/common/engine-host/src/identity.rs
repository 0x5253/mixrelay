//! Native engine run identity construction.
/// 128 random bits from two independently keyed hashes of this process and its start.
#[must_use]
pub fn new_incarnation() -> String {
    use std::hash::{BuildHasher as _, RandomState};
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seed = (std::process::id(), started);
    format!(
        "{:016x}{:016x}",
        RandomState::new().hash_one(seed),
        RandomState::new().hash_one(seed)
    )
}
