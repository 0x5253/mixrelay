use std::{
    ffi::{CStr, c_void},
    ptr::NonNull,
};

// SDK: CoreFoundation/{CFArray,CFDictionary,CFNumber,CFString,CFUUID}.h.
// Callback tables remain opaque: only their symbol addresses cross back to CF.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: u8;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFRelease(value: *const c_void);
    fn CFRetain(value: *const c_void) -> *const c_void;
    fn CFEqual(left: *const c_void, right: *const c_void) -> u8;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        count: isize,
        callbacks: *const u8,
    ) -> *const c_void;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const u8,
        value_callbacks: *const u8,
    ) -> *const c_void;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberCreate(allocator: *const c_void, kind: isize, value: *const c_void)
    -> *const c_void;
    fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> u8;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFStringCreateWithFormat(
        allocator: *const c_void,
        options: *const c_void,
        format: *const c_void,
        ...
    ) -> *const c_void;
    fn CFUUIDCreate(allocator: *const c_void) -> *const c_void;
    fn CFUUIDCreateString(allocator: *const c_void, uuid: *const c_void) -> *const c_void;
}

const UTF8: u32 = 0x0800_0100;
const SINT32: isize = 3;

/// One owned +1 CF reference; all constructors immediately close the create/get rule.
#[derive(Debug)]
pub(super) struct Object(NonNull<c_void>);

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: each constructor establishes exactly one owned retain count.
        unsafe { CFRelease(self.as_ptr()) };
    }
}

impl Object {
    /// Takes one create/copy-rule reference, or returns None for a failed OS allocation.
    pub(super) unsafe fn owned(pointer: *const c_void) -> Option<Self> {
        NonNull::new(pointer.cast_mut()).map(Self)
    }

    pub(super) fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr()
    }

    pub(super) fn string(value: &CStr) -> Option<Self> {
        // SAFETY: the borrowed C string is terminated and alive; CF returns a +1 object.
        unsafe {
            Self::owned(CFStringCreateWithCString(
                std::ptr::null(),
                value.as_ptr(),
                UTF8,
            ))
        }
    }

    pub(super) fn number(value: i32) -> Option<Self> {
        // SAFETY: SInt32 reads exactly one live i32; the result follows the create rule.
        unsafe {
            Self::owned(CFNumberCreate(
                std::ptr::null(),
                SINT32,
                (&raw const value).cast(),
            ))
        }
    }

    pub(super) fn equal(&self, other: &Self) -> bool {
        // SAFETY: both references remain owned during the polymorphic CF equality call.
        unsafe { CFEqual(self.as_ptr(), other.as_ptr()) != 0 }
    }

    pub(super) fn dictionary<const N: usize>(entries: [(&CStr, &Self); N]) -> Option<Self> {
        let mut keys: [Option<Self>; N] = std::array::from_fn(|_| None);
        let mut pointers = [std::ptr::null(); N];
        for (index, (key, _)) in entries.iter().enumerate() {
            let key = Self::string(key)?;
            pointers[index] = key.as_ptr();
            keys[index] = Some(key);
        }
        let values = entries.map(|(_, value)| value.as_ptr());
        // SAFETY: all pointer arrays have N entries and their owners remain live; the
        // standard CF callbacks retain keys/values before this constructor returns.
        unsafe {
            Self::owned(
                CFDictionaryCreate(
                    std::ptr::null(),
                    pointers.as_ptr(),
                    values.as_ptr(),
                    isize::try_from(N).ok()?,
                    &raw const kCFTypeDictionaryKeyCallBacks,
                    &raw const kCFTypeDictionaryValueCallBacks,
                )
                .cast(),
            )
        }
    }

    pub(super) fn array(values: &[Option<Self>]) -> Option<Self> {
        let mut pointers = [std::ptr::null(); 32];
        let selected = pointers.get_mut(..values.len())?;
        for (target, value) in selected.iter_mut().zip(values) {
            *target = value.as_ref()?.as_ptr();
        }
        // SAFETY: selected points to live CF objects and standard callbacks retain every element.
        unsafe {
            Self::owned(
                CFArrayCreate(
                    std::ptr::null(),
                    pointers.as_ptr(),
                    isize::try_from(values.len()).ok()?,
                    &raw const kCFTypeArrayCallBacks,
                )
                .cast(),
            )
        }
    }

    pub(super) fn len(&self) -> Option<usize> {
        // SAFETY: type inspection accepts all CF objects; array functions are called only on arrays.
        unsafe {
            if CFGetTypeID(self.as_ptr()) != CFArrayGetTypeID() {
                return None;
            }
            usize::try_from(CFArrayGetCount(self.as_ptr().cast())).ok()
        }
    }

    pub(super) fn element(&self, index: usize) -> Option<Self> {
        if index >= self.len()? {
            return None;
        }
        // SAFETY: array type/bounds were checked and self retains the container; retaining
        // its get-rule element gives the result an independent lifetime.
        unsafe {
            let value = CFArrayGetValueAtIndex(self.as_ptr().cast(), isize::try_from(index).ok()?);
            if value.is_null() {
                return None;
            }
            Self::owned(CFRetain(value))
        }
    }

    pub(super) fn value(&self, key: &CStr) -> Option<Self> {
        let key = Self::string(key)?;
        // SAFETY: dictionary access follows a checked runtime type; the live container
        // owns the borrowed value until its extra retain is acquired.
        unsafe {
            if CFGetTypeID(self.as_ptr()) != CFDictionaryGetTypeID() {
                return None;
            }
            let value = CFDictionaryGetValue(self.as_ptr().cast(), key.as_ptr());
            if value.is_null() {
                return None;
            }
            Self::owned(CFRetain(value))
        }
    }

    pub(super) fn integer(&self) -> Option<i32> {
        let mut value = 0;
        // SAFETY: number access follows a checked runtime type and writes exactly one i32.
        unsafe {
            if CFGetTypeID(self.as_ptr()) != CFNumberGetTypeID() {
                return None;
            }
            (CFNumberGetValue(self.as_ptr(), SINT32, (&raw mut value).cast()) != 0).then_some(value)
        }
    }

    pub(super) fn unique_uid() -> Option<Self> {
        let format = Self::string(c"io.mixrelay.private-group.%@")?;
        // SAFETY: each create-rule result is guarded before another fallible operation;
        // %@ consumes the live CF string object supplied as the sole variadic argument.
        unsafe {
            let uuid = Self::owned(CFUUIDCreate(std::ptr::null()).cast())?;
            let text =
                Self::owned(CFUUIDCreateString(std::ptr::null(), uuid.as_ptr().cast()).cast())?;
            Self::owned(
                CFStringCreateWithFormat(
                    std::ptr::null(),
                    std::ptr::null(),
                    format.as_ptr().cast(),
                    text.as_ptr(),
                )
                .cast(),
            )
        }
    }
}
