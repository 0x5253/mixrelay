//! The privileged entry check: the installer never runs `sudo`, prompts, or changes identity.

use super::{NativeError, account::group_id_for_name};

/// Refuses to perform any native privileged work unless already running as root.
///
/// The installer never invokes `sudo`, prompts for a password, or changes its
/// effective identity.
/// # Errors
/// Returns [`NativeError::RootRequired`] unless the effective identity is root:wheel.
pub fn require_root() -> Result<(), NativeError> {
    // SAFETY: geteuid is a read-only process identity query.
    let effective_user_id = unsafe { libc::geteuid() };
    if effective_user_id != 0 {
        return Err(NativeError::RootRequired);
    }
    let wheel = group_id_for_name("wheel")?;
    // SAFETY: getegid is a read-only process identity query.
    let effective_group_id = unsafe { libc::getegid() };
    if root_wheel_identity(effective_user_id, effective_group_id, wheel) {
        Ok(())
    } else {
        Err(NativeError::RootRequired)
    }
}

fn root_wheel_identity(user_id: u32, group_id: u32, wheel_gid: u32) -> bool {
    user_id == 0 && group_id == wheel_gid
}

#[cfg(test)]
mod tests {
    #[test]
    fn privileged_operations_require_root_with_the_wheel_effective_group() {
        assert!(super::root_wheel_identity(0, 0, 0));
        assert!(!super::root_wheel_identity(0, 20, 0));
        assert!(!super::root_wheel_identity(501, 0, 0));
    }
}
