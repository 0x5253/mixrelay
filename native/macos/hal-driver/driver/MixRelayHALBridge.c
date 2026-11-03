/* Apple COM vtable and SDK ABI projections only. All object/property/format,
 * registry, clock, transport ownership and callback routing policy is Rust.
 * This is the production bundle's narrow Apple ABI entry point. */
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CFPlugInCOM.h>
#include <mach/mach_time.h>
#include <stdatomic.h>
#include <string.h>
#include "MRHALCore.h"
#include "MRHALControl.h"
#include "MRHALIO.h"

static MRHALCore *gCore;
static _Atomic UInt32 gReferences = 1;
static AudioServerPlugInDriverInterface gInterface;
static AudioServerPlugInDriverInterface *gInterfacePointer = &gInterface;
static AudioServerPlugInDriverRef gDriver = &gInterfacePointer;

_Static_assert(sizeof(AudioObjectPropertyAddress) == sizeof(MRHALCorePropertyAddress), "property ABI");
_Static_assert(sizeof(AudioStreamBasicDescription) == 40, "ASBD ABI");
_Static_assert(sizeof(AudioStreamRangedDescription) == 56, "ranged ASBD ABI");

static OSStatus result(int32_t status) {
    if (status == MR_HAL_CORE_OK) return noErr;
    if (status == MR_HAL_CORE_NOT_FOUND) return kAudioHardwareBadObjectError;
    return kAudioHardwareIllegalOperationError;
}
static const MRHALCorePropertyAddress *address(const AudioObjectPropertyAddress *value) {
    return (const MRHALCorePropertyAddress *)value;
}
static bool valid(AudioServerPlugInDriverRef driver) { return driver == gDriver && gCore != NULL; }

static void changed(void *context, uint32_t object, uint32_t change) {
    AudioServerPlugInHostRef host = context;
    MRHALCoreNotification notifications[16];
    const size_t count = mr_hal_core_notifications(object, change, notifications, 16);
    for (size_t index = 0; index < count; ++index) {
        AudioObjectPropertyAddress property;
        memcpy(&property, &notifications[index].address, sizeof(property));
        (void)host->PropertiesChanged(host, notifications[index].object_id, 1, &property);
    }
}
static bool request_configuration(void *context, uint32_t object, uint64_t action, uint64_t generation) {
    AudioServerPlugInHostRef host = context;
    return host->RequestDeviceConfigurationChange(host, object, action, (void *)(uintptr_t)generation) == noErr;
}
static HRESULT query(void *driver, REFIID uuid, LPVOID *output) {
    if (driver != gDriver || output == NULL) return E_NOINTERFACE;
    CFUUIDRef requested = CFUUIDCreateFromUUIDBytes(NULL, uuid);
    const bool supported = requested != NULL && (CFEqual(requested, IUnknownUUID) || CFEqual(requested, kAudioServerPlugInDriverInterfaceUUID));
    if (requested != NULL) CFRelease(requested);
    if (!supported) { *output = NULL; return E_NOINTERFACE; }
    atomic_fetch_add_explicit(&gReferences, 1, memory_order_relaxed);
    *output = gDriver;
    return S_OK;
}
static ULONG add_ref(void *driver) {
    if (driver != gDriver) return 0;
    return atomic_fetch_add_explicit(&gReferences, 1, memory_order_relaxed) + 1;
}
static ULONG release_ref(void *driver) {
    if (driver != gDriver) return 0;
    UInt32 value = atomic_load_explicit(&gReferences, memory_order_relaxed);
    while (value > 0 && !atomic_compare_exchange_weak_explicit(&gReferences, &value, value - 1, memory_order_relaxed, memory_order_relaxed)) {}
    /* The HAL owns the factory singleton for process lifetime. Releasing a COM
     * query does not destroy XPC blocks while callbacks can still arrive. */
    return value > 0 ? value - 1 : 0;
}
static OSStatus initialize(AudioServerPlugInDriverRef driver, AudioServerPlugInHostRef host) {
    if (driver != gDriver || host == NULL || gCore != NULL) return kAudioHardwareIllegalOperationError;
    /* HostOps carries an opaque C callback context. It never mutates the SDK's
     * const host interface; both callbacks restore AudioServerPlugInHostRef. */
    const MRHALCoreHostOps callbacks = {(void *)host, changed, request_configuration};
    return mr_hal_control_initialize(&callbacks, &gCore) ? noErr : kAudioHardwareIllegalOperationError;
}
static OSStatus create_device(AudioServerPlugInDriverRef driver, CFDictionaryRef description, const AudioServerPlugInClientInfo *client, AudioObjectID *device) {
    (void)driver; (void)description; (void)client; (void)device;
    return kAudioHardwareUnsupportedOperationError;
}
static OSStatus destroy_device(AudioServerPlugInDriverRef driver, AudioObjectID device) {
    (void)driver; (void)device;
    return kAudioHardwareUnsupportedOperationError;
}
static OSStatus add_client(AudioServerPlugInDriverRef driver, AudioObjectID device, const AudioServerPlugInClientInfo *client) {
    if (!valid(driver) || client == NULL) return kAudioHardwareBadObjectError;
    char bundle[128] = {0};
    const char *identity = client->mBundleID != NULL && CFStringGetCString(client->mBundleID, bundle, sizeof(bundle), kCFStringEncodingUTF8) ? bundle : NULL;
    return result(mr_hal_core_client_identity(gCore, device, client->mClientID, client->mProcessID, identity, true));
}
static OSStatus remove_client(AudioServerPlugInDriverRef driver, AudioObjectID device, const AudioServerPlugInClientInfo *client) {
    return valid(driver) && client != NULL ? result(mr_hal_core_client_identity(gCore, device, client->mClientID, 0, NULL, false)) : kAudioHardwareBadObjectError;
}
static OSStatus perform(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt64 action, void *info) {
    return valid(driver) ? result(mr_hal_core_perform(gCore, device, action, (uintptr_t)info)) : kAudioHardwareBadObjectError;
}
static OSStatus abort_change(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt64 action, void *info) {
    return valid(driver) ? result(mr_hal_core_abort(gCore, device, action, (uintptr_t)info)) : kAudioHardwareBadObjectError;
}
static Boolean has_property(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t pid, const AudioObjectPropertyAddress *property) {
    (void)pid;
    return valid(driver) && mr_hal_core_has_property(gCore, object, address(property));
}
static OSStatus settable(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t pid, const AudioObjectPropertyAddress *property, Boolean *output) {
    (void)pid;
    if (!valid(driver) || output == NULL) return kAudioHardwareBadObjectError;
    bool value = false;
    OSStatus status = mr_hal_core_property_settable(gCore, object, address(property), &value);
    if (status == noErr) *output = value;
    return status;
}
static OSStatus property_size(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t pid, const AudioObjectPropertyAddress *property, UInt32 qualifier_size, const void *qualifier, UInt32 *output) {
    (void)pid; (void)qualifier_size; (void)qualifier;
    return valid(driver) ? mr_hal_core_property_size(gCore, object, address(property), output) : kAudioHardwareBadObjectError;
}
static OSStatus property_get(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t pid, const AudioObjectPropertyAddress *property, UInt32 qualifier_size, const void *qualifier, UInt32 capacity, UInt32 *written, void *output) {
    if (!valid(driver)) return kAudioHardwareBadObjectError;
    /* Every read forwards the requester: Rust answers transport type and input latency
     * per requester, and treats PID 0 as coreaudiod itself. */
    OSStatus status = mr_hal_core_property_get(gCore, object, address(property), qualifier_size, qualifier, capacity, written, output, pid);
    return status;
}
static OSStatus property_set(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t pid, const AudioObjectPropertyAddress *property, UInt32 qualifier_size, const void *qualifier, UInt32 size, const void *input) {
    (void)pid; (void)qualifier_size; (void)qualifier;
    return valid(driver) ? mr_hal_core_property_set(gCore, object, address(property), size, input) : kAudioHardwareBadObjectError;
}
static OSStatus start_io(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client) {
    return valid(driver) ? result(mr_hal_core_start(gCore, device, client, mach_absolute_time())) : kAudioHardwareBadObjectError;
}
static OSStatus stop_io(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client) {
    return valid(driver) ? result(mr_hal_core_stop(gCore, device, client)) : kAudioHardwareBadObjectError;
}
static OSStatus timestamp_impl(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client, Float64 *sample, UInt64 *host, UInt64 *seed) {
    (void)client;
    if (!valid(driver) || sample == NULL || host == NULL || seed == NULL) return kAudioHardwareBadObjectError;
    MRHALCoreTimestamp value;
    if (!mr_hal_core_timestamp(gCore, device, mach_absolute_time(), &value)) return kAudioHardwareIllegalOperationError;
    *sample = (Float64)value.sample_time; *host = value.host_time; *seed = value.seed;
    return noErr;
}
static OSStatus timestamp(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client, Float64 *sample, UInt64 *host, UInt64 *seed) {
    mr_hal_core_enter_audio_thread();
    const OSStatus status = timestamp_impl(driver, device, client, sample, host, seed);
    mr_hal_core_leave_audio_thread();
    return status;
}
static OSStatus will_io(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client, UInt32 operation, Boolean *will, Boolean *in_place) {
    (void)client;
    if (!valid(driver) || will == NULL || in_place == NULL || !mr_hal_core_device_alive(gCore, device)) return kAudioHardwareBadObjectError;
    *will = operation == kAudioServerPlugInIOOperationReadInput || operation == kAudioServerPlugInIOOperationMixOutput;
    *in_place = true;
    return noErr;
}
static OSStatus begin_io(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client, UInt32 operation, UInt32 frames, const AudioServerPlugInIOCycleInfo *cycle) {
    (void)device; (void)client; (void)operation; (void)frames; (void)cycle;
    if (!valid(driver)) return kAudioHardwareBadObjectError;
    mr_hal_core_enter_audio_thread();
    return noErr;
}
static OSStatus end_io(AudioServerPlugInDriverRef driver, AudioObjectID device, UInt32 client, UInt32 operation, UInt32 frames, const AudioServerPlugInIOCycleInfo *cycle) {
    (void)device; (void)client; (void)operation; (void)frames; (void)cycle;
    mr_hal_core_leave_audio_thread();
    return valid(driver) ? noErr : kAudioHardwareBadObjectError;
}
static OSStatus do_io_impl(AudioServerPlugInDriverRef driver, AudioObjectID device, AudioObjectID stream, UInt32 client, UInt32 operation, UInt32 frames, const AudioServerPlugInIOCycleInfo *cycle, void *main_buffer, void *secondary) {
    (void)secondary;
    if (!valid(driver)) return kAudioHardwareBadObjectError;
    const bool render = operation == kAudioServerPlugInIOOperationMixOutput;
    if (cycle == NULL || main_buffer == NULL) {
        if (render || operation == kAudioServerPlugInIOOperationReadInput) {
            const double time = cycle == NULL ? __builtin_nan("") : render ? cycle->mOutputTime.mSampleTime : cycle->mInputTime.mSampleTime;
            (void)mr_hal_core_io_sample_time(gCore, device, stream, render, client, time, main_buffer, frames);
        }
        return kAudioHardwareBadObjectError;
    }
    if (!render && operation != kAudioServerPlugInIOOperationReadInput) return noErr;
    if (render) mr_hal_core_render_client(gCore, device, client);
    const double sample_time = render ? cycle->mOutputTime.mSampleTime : cycle->mInputTime.mSampleTime;
    return mr_hal_core_io_sample_time(gCore, device, stream, render, client, sample_time, main_buffer, frames) ? noErr : kAudioHardwareIllegalOperationError;
}

static OSStatus do_io(AudioServerPlugInDriverRef driver, AudioObjectID device, AudioObjectID stream, UInt32 client, UInt32 operation, UInt32 frames, const AudioServerPlugInIOCycleInfo *cycle, void *main_buffer, void *secondary) {
    mr_hal_core_enter_audio_thread();
    const OSStatus status = do_io_impl(driver, device, stream, client, operation, frames, cycle, main_buffer, secondary);
    mr_hal_core_leave_audio_thread();
    return status;
}

static AudioServerPlugInDriverInterface gInterface = {
    NULL, query, add_ref, release_ref, initialize, create_device, destroy_device,
    add_client, remove_client, perform, abort_change, has_property, settable,
    property_size, property_get, property_set, start_io, stop_io, timestamp,
    will_io, begin_io, do_io, end_io
};

void *MixRelayHAL_Create(CFAllocatorRef allocator, CFUUIDRef requested) {
    (void)allocator;
    return requested != NULL && CFEqual(requested, kAudioServerPlugInTypeUUID) ? gDriver : NULL;
}
