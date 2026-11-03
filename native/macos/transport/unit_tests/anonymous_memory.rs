use super::*;
use std::{process::Command, ptr::NonNull};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        size: usize,
        protection: c_int,
        flags: c_int,
        descriptor: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, size: usize) -> c_int;
    fn xpc_fd_create(descriptor: c_int) -> *mut c_void;
    fn xpc_fd_dup(object: *mut c_void) -> c_int;
    fn xpc_release(object: *mut c_void);
}

struct Mapping {
    pointer: NonNull<c_void>,
    size: usize,
}
impl Mapping {
    fn new(descriptor: &OwnedFd, size: usize, writable: bool) -> io::Result<Self> {
        let protection = if writable { 3 } else { 1 }; // PROT_READ | optional PROT_WRITE.
        let pointer = unsafe {
            mmap(
                std::ptr::null_mut(),
                size,
                protection,
                1,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if pointer as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pointer: NonNull::new(pointer).expect("non-null mmap result"),
            size,
        })
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        assert_eq!(unsafe { munmap(self.pointer.as_ptr(), self.size) }, 0);
    }
}

struct XpcFd(NonNull<c_void>);
impl Drop for XpcFd {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0.as_ptr()) };
    }
}

fn shared_roundtrip() {
    let original = create(4096, true).unwrap();
    assert_ne!(
        unsafe { fcntl(original.as_raw_fd(), F_GETFD) } & FD_CLOEXEC,
        0
    );
    let first = Mapping::new(&original, 4096, true).unwrap();
    unsafe {
        *first.pointer.as_ptr().cast::<u64>() = 0x1234_5678_90ab_cdef;
    }
    let object =
        XpcFd(NonNull::new(unsafe { xpc_fd_create(original.as_raw_fd()) }).expect("XPC FD object"));
    let duplicate = owned_descriptor(unsafe { xpc_fd_dup(object.0.as_ptr()) }).unwrap();
    assert_ne!(original.as_raw_fd(), duplicate.as_raw_fd());
    drop(object);
    drop(original);
    let second = Mapping::new(&duplicate, 4096, true).unwrap();
    assert_eq!(
        unsafe { *second.pointer.as_ptr().cast::<u64>() },
        0x1234_5678_90ab_cdef
    );
    unsafe {
        *second.pointer.as_ptr().cast::<u64>() = 0xfedc_ba09_8765_4321;
    }
    assert_eq!(
        unsafe { *first.pointer.as_ptr().cast::<u64>() },
        0xfedc_ba09_8765_4321
    );
    drop(duplicate);
    // Neither mapping depends on a live descriptor or namespace entry.
    assert_eq!(
        unsafe { *second.pointer.as_ptr().cast::<u64>() },
        0xfedc_ba09_8765_4321
    );
}

#[test]
fn anonymous_descriptor_roundtrips_through_actual_xpc_and_shared_mappings() {
    shared_roundtrip();
}

#[test]
fn read_only_descriptors_cannot_acquire_writable_mappings_and_have_no_file_path() {
    for writable in [false, true] {
        let descriptor = create(4096, writable).unwrap();
        let mut path = [0_u8; 1024];
        // Darwin F_GETPATH works for filesystem-backed mkstemp files, including
        // unlinked ones. POSIX shared-memory objects do not have a disk path.
        assert_eq!(
            unsafe { fcntl(descriptor.as_raw_fd(), 50, path.as_mut_ptr()) },
            -1
        );
        let mapping = Mapping::new(&descriptor, 4096, true);
        assert_eq!(mapping.is_ok(), writable);
        assert!(Mapping::new(&descriptor, 4096, false).is_ok());
    }
}

#[test]
fn invalid_sizes_are_rejected_without_creating_resources() {
    assert_eq!(
        create(0, true).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        create(usize::MAX, true).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(mr_shared_memory_create(0, true), -1);
}

#[test]
fn repeated_shared_memory_and_xpc_ownership_returns_to_baseline() {
    const CHILD: &str = "MIXRELAY_SHARED_MEMORY_TEST_CHILD";
    if std::env::var(CHILD).as_deref() != Ok("1") {
        // Isolate FD accounting from unrelated Cargo test threads. The child
        // runs this actual test, never an ignored or alternate implementation.
        let module = module_path!()
            .split_once("::")
            .expect("crate module path")
            .1;
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::repeated_shared_memory_and_xpc_ownership_returns_to_baseline"),
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("256 anonymous FD/XPC cycles: descriptor count unchanged")
        );
        return;
    }
    shared_roundtrip(); // Warm system XPC initialization before ownership accounting.
    let descriptor_count = || std::fs::read_dir("/dev/fd").unwrap().count();
    let before = descriptor_count();
    for _ in 0..256 {
        shared_roundtrip();
    }
    assert_eq!(descriptor_count(), before);
    println!("256 anonymous FD/XPC cycles: descriptor count unchanged");
}
