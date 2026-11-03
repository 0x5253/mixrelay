//! Exercise the built Mach-O factory and COM ABI without Initialize/StartIO.
//! Loading this probe cannot connect to the engine or publish an OS device.
use std::{
    error::Error,
    ffi::{CString, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr::{self, NonNull},
};

use core_foundation_sys::{
    base::{CFAllocatorRef, CFRelease},
    uuid::{CFUUIDBytes, CFUUIDCreateFromUUIDBytes, CFUUIDRef},
};

type Factory = unsafe extern "C" fn(CFAllocatorRef, CFUUIDRef) -> *mut c_void;

// Exact IUNKNOWN_C_GUTS prefix from Apple's CFPlugInCOM.h. Do not model the
// remaining AudioServerPlugIn vtable here: this probe never initializes it.
#[repr(C)]
struct Unknown {
    reserved: *mut c_void,
    query: unsafe extern "C" fn(*mut c_void, CFUUIDBytes, *mut *mut c_void) -> i32,
    retain: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
}

struct Library(NonNull<c_void>);

impl Library {
    fn open(executable: &Path) -> Result<Self, Box<dyn Error>> {
        let path = CString::new(executable.as_os_str().as_bytes())?;
        // SAFETY: owned NUL-terminated path; RTLD_NOW resolves the produced
        // bundle's complete dependencies before returning any function pointer.
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        NonNull::new(handle)
            .map(Self)
            .ok_or_else(|| "cannot load the produced HAL executable".into())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: this guard owns the dlopen reference; all probe calls finished.
        unsafe {
            libc::dlclose(self.0.as_ptr());
        }
    }
}

struct Uuid(CFUUIDRef);

impl Uuid {
    fn new(bytes: CFUUIDBytes) -> Result<Self, Box<dyn Error>> {
        // SAFETY: valid value struct, default allocator, owned Create result.
        let value = unsafe { CFUUIDCreateFromUUIDBytes(ptr::null(), bytes) };
        if value.is_null() {
            Err("cannot create factory UUID".into())
        } else {
            Ok(Self(value))
        }
    }
}

impl Drop for Uuid {
    fn drop(&mut self) {
        // SAFETY: exactly one release for the non-null Create-owned object.
        unsafe {
            CFRelease(self.0.cast());
        }
    }
}

pub(super) fn factory(executable: &Path) -> Result<(), Box<dyn Error>> {
    let library = Library::open(executable)?;
    // SAFETY: live dlopen handle and literal NUL-terminated exported factory.
    let symbol = unsafe { libc::dlsym(library.0.as_ptr(), c"MixRelayHAL_Create".as_ptr()) };
    if symbol.is_null() {
        return Err("HAL factory symbol is missing".into());
    }
    // SAFETY: this exact symbol is defined by our bridge with Factory's ABI.
    let create: Factory = unsafe { std::mem::transmute(symbol) };
    let plugin = Uuid::new(uuid_bytes([
        0x44, 0x3a, 0xba, 0xb8, 0xe7, 0xb3, 0x49, 0x1a, 0xb9, 0x85, 0xbe, 0xb9, 0x18, 0x70, 0x30,
        0xdb,
    ]))?;
    // SAFETY: null allocator is permitted, plugin UUID remains alive.
    let driver = unsafe { create(ptr::null(), plugin.0) };
    if driver.is_null() {
        return Err("HAL factory rejected AudioServerPlugIn type".into());
    }
    // SAFETY: the factory contract returns a pointer to the driver's vtable.
    let interface = unsafe { &**driver.cast::<*const Unknown>() };
    verify_queries(driver, interface)?;
    // SAFETY: null requested type is explicitly rejected by our factory.
    if !unsafe { create(ptr::null(), ptr::null()) }.is_null() {
        return Err("HAL factory accepted a missing type UUID".into());
    }
    println!("Native HAL factory/COM probe passed (no device initialization)");
    Ok(())
}

fn verify_queries(driver: *mut c_void, interface: &Unknown) -> Result<(), Box<dyn Error>> {
    let unknown = uuid_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    let mut queried = ptr::null_mut();
    // SAFETY: correct COM ABI, live driver and writable output pointer.
    let status = unsafe { (interface.query)(driver, unknown, &raw mut queried) };
    if status != 0 || queried != driver {
        return Err("HAL IUnknown query did not return the factory driver".into());
    }
    // SAFETY: successful QueryInterface acquired this reference. These calls
    // exercise retain/release without releasing the factory-owned base reference.
    let (retained, released, baseline) = unsafe {
        (
            (interface.retain)(driver),
            (interface.release)(driver),
            (interface.release)(queried),
        )
    };
    if retained != 3 || released != 2 || baseline != 1 {
        return Err("HAL COM ownership counts are inconsistent".into());
    }
    // SAFETY: all-zero UUID is unsupported; output is valid writable storage.
    let status = unsafe { (interface.query)(driver, CFUUIDBytes::default(), &raw mut queried) };
    if status >= 0 || !queried.is_null() {
        return Err("HAL COM query accepted an unsupported interface".into());
    }
    Ok(())
}

fn uuid_bytes(bytes: [u8; 16]) -> CFUUIDBytes {
    CFUUIDBytes {
        byte0: bytes[0],
        byte1: bytes[1],
        byte2: bytes[2],
        byte3: bytes[3],
        byte4: bytes[4],
        byte5: bytes[5],
        byte6: bytes[6],
        byte7: bytes[7],
        byte8: bytes[8],
        byte9: bytes[9],
        byte10: bytes[10],
        byte11: bytes[11],
        byte12: bytes[12],
        byte13: bytes[13],
        byte14: bytes[14],
        byte15: bytes[15],
    }
}
