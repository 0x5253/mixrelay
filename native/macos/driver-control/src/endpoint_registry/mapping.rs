//! One initialized transport mapping. Geometry never changes in-place; every
//! replacement gets its own descriptor and shared-memory lifetime.
use super::{EndpointMapping, EndpointSpec, MappingAllocation};
#[cfg(test)]
use mixrelay_macos_transport::shared_audio_buffer::SharedAudioBuffer;
use mixrelay_macos_transport::{
    control_inventory::InventoryEntry,
    endpoint_transport::{
        EndpointTransport, EndpointTransportConfig, mr_endpoint_transport_capture_buffer,
        mr_endpoint_transport_initialize, mr_endpoint_transport_render_buffer,
        mr_endpoint_transport_required_bytes,
    },
};
use std::{
    ffi::{c_char, c_int, c_void},
    io,
    os::fd::{AsRawFd, OwnedFd},
    ptr::{self, NonNull},
    sync::Arc,
};

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        descriptor: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> c_int;
}

/// Never exposes a Rust reference into interprocess-writable storage. Only the
/// transport's atomic ABI may access it after initialization and publication.
struct Region {
    address: NonNull<c_void>,
    length: usize,
    descriptor: OwnedFd,
}

// SAFETY: mapping addresses and descriptor ownership are immutable. Readers
// only use the atomic transport ABI; Arc retains storage until the final pin.
unsafe impl Send for Region {}
// SAFETY: no API exposes mutable Rust references into this shared mapping.
unsafe impl Sync for Region {}

impl Region {
    fn new(length: usize) -> io::Result<Self> {
        let descriptor = mixrelay_macos_transport::create_shared_memory(length, true)?;
        let address = unsafe { mmap(ptr::null_mut(), length, 3, 1, descriptor.as_raw_fd(), 0) };
        if address == ptr::without_provenance_mut(usize::MAX) {
            return Err(io::Error::last_os_error());
        }
        let Some(address) = NonNull::new(address) else {
            // A zero-address mapping cannot be represented by the native ABI.
            unsafe { munmap(address, length) };
            return Err(io::Error::other("native mapping returned address zero"));
        };
        Ok(Self {
            address,
            length,
            descriptor,
        })
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: only this owner unmaps, after all callback pins are released.
        unsafe { munmap(self.address.as_ptr(), self.length) };
        // OwnedFd closes after unmapping. Independently duplicated HAL/snapshot
        // FDs continue to own the object after Driver control retires its map.
    }
}

pub(crate) struct Mapping {
    spec: EndpointSpec,
    generation: u64,
    region: Arc<Region>,
    transport: NonNull<EndpointTransport>,
}

// SAFETY: geometry/addresses/spec are immutable after initialization. Mutable
// shared storage is accessed solely through the atomic native transport ABI;
// this type never hands out mutable Rust references. Registry retirement pins
// ensure unmapping cannot race either a local callback or a snapshot FD copy.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    pub(super) fn required_bytes(spec: &EndpointSpec) -> io::Result<usize> {
        let config = EndpointTransportConfig {
            endpoint_id: spec.id,
            capacity_frames: spec.capacity_frames,
            render_channels: spec.render_channels,
            capture_channels: spec.capture_channels,
        };
        let length = unsafe { mr_endpoint_transport_required_bytes(&raw const config) };
        if length == 0 || u32::try_from(length).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid native endpoint layout",
            ));
        }
        Ok(length)
    }
    pub(super) fn new(spec: EndpointSpec, generation: u64) -> io::Result<Self> {
        let length = Self::required_bytes(&spec)?;
        let config = EndpointTransportConfig {
            endpoint_id: spec.id,
            capacity_frames: spec.capacity_frames,
            render_channels: spec.render_channels,
            capture_channels: spec.capture_channels,
        };
        let region = Region::new(length)?;
        let transport = NonNull::new(unsafe {
            mr_endpoint_transport_initialize(region.address.as_ptr(), length, &raw const config)
        })
        .ok_or_else(|| io::Error::other("endpoint transport initialization failed"))?;
        let render = unsafe { mr_endpoint_transport_render_buffer(transport.as_ptr()) };
        let capture = unsafe { mr_endpoint_transport_capture_buffer(transport.as_ptr()) };
        if (spec.render_channels != 0 && render.is_null())
            || (spec.capture_channels != 0 && capture.is_null())
        {
            return Err(io::Error::other("endpoint transport buffers are invalid"));
        }
        Ok(Self {
            spec,
            generation,
            region: Arc::new(region),
            transport,
        })
    }
    pub(super) fn reconfigured(&self, spec: EndpointSpec, generation: u64) -> Option<Self> {
        self.can_reconfigure(&spec).then(|| Self {
            spec,
            generation,
            region: Arc::clone(&self.region),
            transport: self.transport,
        })
    }
    pub(super) fn can_reconfigure(&self, spec: &EndpointSpec) -> bool {
        self.spec.render_channels == spec.render_channels
            && self.spec.capture_channels == spec.capture_channels
            && self.spec.capacity_frames == spec.capacity_frames
    }
    pub(super) fn region_usage(&self) -> (usize, usize) {
        (Arc::as_ptr(&self.region) as usize, self.region.length)
    }
    pub(super) fn transport(&self) -> *mut EndpointTransport {
        self.transport.as_ptr()
    }
    #[cfg(test)]
    pub(super) fn render_buffer(&self) -> *mut SharedAudioBuffer {
        // SAFETY: this Mapping retains its initialized transport region.
        unsafe { mr_endpoint_transport_render_buffer(self.transport.as_ptr()) }
    }
    #[cfg(test)]
    pub(super) fn capture_buffer(&self) -> *mut SharedAudioBuffer {
        // SAFETY: this Mapping retains its initialized transport region.
        unsafe { mr_endpoint_transport_capture_buffer(self.transport.as_ptr()) }
    }
    pub(super) fn inventory_entry(&self) -> io::Result<(InventoryEntry, OwnedFd)> {
        let descriptor = self.region.descriptor.try_clone()?;
        let mut entry = InventoryEntry {
            endpoint_id: self.spec.id,
            sample_rate: self.spec.sample_rate,
            descriptor: descriptor.as_raw_fd(),
            mapped_size: self.region.length,
            generation: self.generation,
            ..InventoryEntry::default()
        };
        for (slot, byte) in entry
            .display_name
            .iter_mut()
            .zip(self.spec.display_name.bytes())
        {
            *slot = c_char::from_ne_bytes([byte]);
        }
        Ok((entry, descriptor))
    }
}

pub(crate) struct NativeMappings;

impl MappingAllocation for NativeMappings {
    type Mapping = Mapping;
    fn required_bytes(&self, spec: &EndpointSpec) -> io::Result<usize> {
        Mapping::required_bytes(spec)
    }
    fn create(&self, spec: EndpointSpec, generation: u64) -> io::Result<Mapping> {
        Mapping::new(spec, generation)
    }
}

impl EndpointMapping for Mapping {
    fn spec(&self) -> &EndpointSpec {
        &self.spec
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn reconfigured(&self, spec: EndpointSpec, generation: u64) -> Option<Self> {
        self.reconfigured(spec, generation)
    }
    fn can_reconfigure(&self, spec: &EndpointSpec) -> bool {
        self.can_reconfigure(spec)
    }
    fn region_usage(&self) -> (usize, usize) {
        self.region_usage()
    }
}
