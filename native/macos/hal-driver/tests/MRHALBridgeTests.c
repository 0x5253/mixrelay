/* The Rust owner tests control-resource lifetime and reconnect behavior.
 * This fixture exercises the retained SDK COM adapter without starting the HAL. */
#include <assert.h>
#include <CoreAudio/CoreAudio.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include "MRHALCore.h"
#include "MRHALControlProtocol.h"

static int32_t forwarded_pid;
static uint32_t rendered_client, rendered_device;
static unsigned audio_scope_depth;
static void observed_enter_audio(void) {
    ++audio_scope_depth;
    mr_hal_core_enter_audio_thread();
}
static void observed_leave_audio(void) {
    assert(audio_scope_depth != 0U);
    --audio_scope_depth;
    mr_hal_core_leave_audio_thread();
}
static void observed_render_client(const MRHALCore *core, uint32_t device, uint32_t client) {
    rendered_device = device; rendered_client = client;
    mr_hal_core_render_client(core, device, client);
}
#define mr_hal_core_render_client observed_render_client
static int32_t observed_get(MRHALCore *core, uint32_t object, const MRHALCorePropertyAddress *property, uint32_t qs, const void *q, uint32_t capacity, uint32_t *written, void *output, int32_t pid) {
    forwarded_pid = pid;
    return mr_hal_core_property_get(core, object, property, qs, q, capacity, written, output, pid);
}
#define mr_hal_core_property_get observed_get
#define mr_hal_core_enter_audio_thread observed_enter_audio
#define mr_hal_core_leave_audio_thread observed_leave_audio
#include "../driver/MixRelayHALBridge.c"
#undef mr_hal_core_property_get
#undef mr_hal_core_render_client
#undef mr_hal_core_enter_audio_thread
#undef mr_hal_core_leave_audio_thread

/* Keep the Rust property's stable FourCC projection tied to the active SDK. */
_Static_assert(kAudioVolumeControlClassID == UINT32_C(0x766c6d65), "volume class ABI");
_Static_assert(kAudioMuteControlClassID == UINT32_C(0x6d757465), "mute class ABI");
_Static_assert(kAudioLevelControlClassID == UINT32_C(0x6c65766c), "level base ABI");
_Static_assert(kAudioBooleanControlClassID == UINT32_C(0x746f676c), "boolean base ABI");
_Static_assert(kAudioLevelControlPropertyScalarValue == UINT32_C(0x6c637376), "scalar ABI");
_Static_assert(kAudioLevelControlPropertyDecibelValue == UINT32_C(0x6c636476), "decibel ABI");
_Static_assert(kAudioLevelControlPropertyDecibelRange == UINT32_C(0x6c636472), "dB range ABI");
_Static_assert(kAudioLevelControlPropertyConvertScalarToDecibels == UINT32_C(0x6c637364), "scalar conversion ABI");
_Static_assert(kAudioLevelControlPropertyConvertDecibelsToScalar == UINT32_C(0x6c636473), "dB conversion ABI");
_Static_assert(sizeof(MRHALCoreIoOutcome) == 8, "native IO outcome ABI");
_Static_assert(offsetof(MRHALCoreIoOutcome, available) == 4, "capture availability ABI");
_Static_assert(kAudioBooleanControlPropertyValue == UINT32_C(0x6263766c), "mute value ABI");
_Static_assert(kAudioControlPropertyScope == UINT32_C(0x63736370), "control scope ABI");
_Static_assert(kAudioControlPropertyElement == UINT32_C(0x63656c6d), "control element ABI");
_Static_assert(kAudioFormatFlagIsFloat == 1U, "Float32 ASBD flag ABI");
_Static_assert(kAudioFormatFlagIsSignedInteger == 4U, "signed integer ASBD flag ABI");
_Static_assert(kAudioFormatFlagIsPacked == 8U, "packed PCM ASBD flag ABI");
_Static_assert(kAudioServerPlugInCustomPropertyDataTypeCFPropertyList == UINT32_C(0x706c7374), "property-list ABI");

static CFDataRef serialized_data(CFDataRef value) {
    CFErrorRef error = NULL;
    CFDataRef encoded = CFPropertyListCreateData(NULL, value, kCFPropertyListBinaryFormat_v1_0, 0, &error);
    assert(encoded != NULL && error == NULL);
    CFPropertyListRef decoded = CFPropertyListCreateWithData(NULL, encoded, kCFPropertyListImmutable, NULL, &error);
    CFRelease(encoded);
    assert(decoded != NULL && error == NULL && CFGetTypeID(decoded) == CFDataGetTypeID());
    return (CFDataRef)decoded;
}

/* Simulate the host's property-list serialization across the client/server boundary. */
static OSStatus client_set(AudioObjectID device, const AudioObjectPropertyAddress *address, UInt32 qs, const void *q, UInt32 size, const void *input) {
    if (size != sizeof(CFDataRef)) return kAudioHardwareBadPropertySizeError;
    CFDataRef value = NULL;
    memcpy(&value, input, sizeof(value));
    CFDataRef decoded = serialized_data(value);
    const OSStatus status = (*gDriver)->SetPropertyData(gDriver, device, getpid(), address, qs, q, sizeof(decoded), &decoded);
    CFRelease(decoded);
    return status;
}
static OSStatus client_size(AudioObjectID device, const AudioObjectPropertyAddress *address, UInt32 qs, const void *q, UInt32 *size) {
    return (*gDriver)->GetPropertyDataSize(gDriver, device, getpid(), address, qs, q, size);
}
static OSStatus client_get(AudioObjectID device, const AudioObjectPropertyAddress *address, UInt32 qs, const void *q, UInt32 *size, void *output) {
    CFDataRef value = NULL;
    const OSStatus status = (*gDriver)->GetPropertyData(gDriver, device, getpid(), address, qs, q, *size, size, &value);
    if (status != noErr) return status;
    CFDataRef decoded = serialized_data(value);
    CFRelease(value);
    memcpy(output, &decoded, sizeof(decoded));
    return noErr;
}
#define AudioObjectSetPropertyData client_set
#define AudioObjectGetPropertyDataSize client_size
#define AudioObjectGetPropertyData client_get
#include "MRProbeHalTrace.c"
#undef AudioObjectSetPropertyData
#undef AudioObjectGetPropertyDataSize
#undef AudioObjectGetPropertyData

static CFDataRef custom_data(AudioObjectID device, AudioObjectPropertySelector selector) {
    const AudioObjectPropertyAddress property = {selector, kAudioObjectPropertyScopeGlobal, 0};
    UInt32 size = 0, written = 0;
    assert((*gDriver)->GetPropertyDataSize(gDriver, device, getpid(), &property, 0, NULL, &size) == noErr);
    assert(size == sizeof(CFDataRef));
    CFDataRef value = NULL;
    assert((*gDriver)->GetPropertyData(gDriver, device, getpid(), &property, 0, NULL, size, &written, &value) == noErr);
    assert(written == sizeof(value) && value != NULL && CFGetTypeID(value) == CFDataGetTypeID());
    CFDataRef decoded = serialized_data(value);
    CFRelease(value);
    return decoded;
}

static OSStatus set_custom_flag(AudioObjectID device, UInt32 value) {
    const AudioObjectPropertyAddress property = {'mrdi', kAudioObjectPropertyScopeGlobal, 0};
    CFDataRef source = CFDataCreate(NULL, (const UInt8 *)&value, sizeof(value));
    assert(source != NULL);
    CFDataRef decoded = serialized_data(source);
    CFRelease(source);
    const OSStatus status = (*gDriver)->SetPropertyData(gDriver, device, getpid(), &property, 0, NULL, sizeof(decoded), &decoded);
    CFRelease(decoded);
    return status;
}

static void custom_property_lists(AudioObjectID device) {
    const AudioObjectPropertyAddress list = {kAudioObjectPropertyCustomPropertyInfoList, kAudioObjectPropertyScopeGlobal, 0};
    AudioServerPlugInCustomPropertyInfo info[2] = {0};
    UInt32 size = 0, written = 0;
    assert((*gDriver)->GetPropertyDataSize(gDriver, device, getpid(), &list, 0, NULL, &size) == noErr);
    assert(size == sizeof(info));
    assert((*gDriver)->GetPropertyData(gDriver, device, getpid(), &list, 0, NULL, sizeof(info), &written, info) == noErr);
    assert(written == sizeof(info));
    for (unsigned index = 0; index < 2; ++index) {
        assert(info[index].mSelector == (index == 0 ? 'mrdi' : 'mrdo'));
        assert(info[index].mPropertyDataType == kAudioServerPlugInCustomPropertyDataTypeCFPropertyList);
        assert(info[index].mQualifierDataType == kAudioServerPlugInCustomPropertyDataTypeNone);
    }
    assert(set_custom_flag(device, 1) == noErr);
    assert(set_custom_flag(device, 2) == kAudioHardwareIllegalOperationError);
    const AudioObjectPropertyAddress flag = {'mrdi', kAudioObjectPropertyScopeGlobal, 0};
    UInt32 raw = 0;
    assert((*gDriver)->SetPropertyData(gDriver, device, getpid(), &flag, 0, NULL, sizeof(raw), &raw) == kAudioHardwareBadPropertySizeError);
    CFStringRef wrong = CFSTR("not CFData");
    assert((*gDriver)->SetPropertyData(gDriver, device, getpid(), &flag, 0, NULL, sizeof(wrong), &wrong) == kAudioHardwareIllegalOperationError);
    CFDataRef enabled = custom_data(device, 'mrdi');
    assert(CFDataGetLength(enabled) == sizeof(raw));
    memcpy(&raw, CFDataGetBytePtr(enabled), sizeof(raw));
    CFRelease(enabled);
    assert(raw == 1);
    CFDataRef snapshot = custom_data(device, 'mrdo');
    assert(CFDataGetLength(snapshot) == (17 + 8192 * 11) * (CFIndex)sizeof(uint64_t));
    uint64_t header[5] = {0};
    memcpy(header, CFDataGetBytePtr(snapshot), sizeof(header));
    CFRelease(snapshot);
    assert(header[0] == 5 && header[1] == 10 && header[2] == 8192 && header[3] == 0 && header[4] == 0);
    assert(set_custom_flag(device, 0) == noErr);
    enabled = custom_data(device, 'mrdi');
    memcpy(&raw, CFDataGetBytePtr(enabled), sizeof(raw));
    CFRelease(enabled);
    assert(raw == 0);
    assert(mr_probe_hal_trace_enable(device, true).stage == NULL);
    char path[] = "/tmp/mixrelay-hal-trace-XXXXXX";
    const int fd = mkstemp(path);
    assert(fd >= 0 && close(fd) == 0 && unlink(path) == 0);
    assert(mr_probe_hal_trace_save(device, path).stage == NULL);
    FILE *file = fopen(path, "r");
    char json[512] = {0};
    assert(file != NULL && fgets(json, sizeof(json), file) != NULL);
    assert(fclose(file) == 0 && unlink(path) == 0);
    assert(strcmp(json, "{\"version\":5,\"issued\":0,\"dropped\":0,\"capacity\":8192,\"admission\":{\"read\":0,\"write\":0,\"timestamp\":0,\"extent\":0,\"stream\":0,\"frozen\":0,\"inactive\":0,\"native\":0,\"lastRejectedTimestampBits\":\"0000000000000000\",\"lastRejectedExtent\":{\"frames\":0,\"bufferPresent\":false,\"render\":false},\"nullBuffer\":0,\"oversizedFrames\":0},\"rows\":[]}\n") == 0);
    assert(mr_probe_hal_trace_enable(device, false).stage == NULL);
}

static bool inspect_fixture(int fd, size_t size, MRHALCoreGeometry *geometry) {
    (void)fd; (void)size;
    memset(geometry, 0, sizeof(*geometry));
    memset(geometry->endpoint_id, 1, 16);
    geometry->render_channels = geometry->capture_channels = 2;
    return true;
}
static void *create_fixture(const MRHALCoreGeometry *geometry, uint32_t rate, int fd, size_t size) {
    (void)geometry; (void)rate; (void)fd; (void)size;
    return malloc(1);
}
static void destroy_fixture(void *runtime) { free(runtime); }
static void fixture_changed(void *context, uint32_t object, uint32_t change) { (void)context; (void)object; (void)change; }
static bool fixture_request(void *context, uint32_t object, uint64_t action, uint64_t generation) {
    (void)context; (void)object; (void)action; (void)generation; return false;
}
static void *prepare_fixture(const MRHALCoreGeometry *geometry, int fd, size_t size) {
    (void)geometry; (void)fd; (void)size; return NULL;
}
static void discard_fixture(void *change) { (void)change; }
static bool replace_fixture(void *runtime, void *change, uint32_t rate) {
    (void)runtime; (void)change; (void)rate; return false;
}
static bool start_fixture(void *runtime, uint64_t time) { (void)runtime; (void)time; return true; }
static void stop_fixture(void *runtime) { (void)runtime; }
static bool rate_fixture(void *runtime, uint32_t rate) { (void)runtime; (void)rate; return false; }
static uint32_t transport_device;
static uint32_t transport_value(void) {
    AudioObjectPropertyAddress property = {kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, 0};
    uint32_t value = 0, written = 0;
    assert(property_get(gDriver, transport_device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr);
    assert(written == 4);
    return value;
}
static void assert_control_transport(void) {
    assert(audio_scope_depth == 0U);
    const uint32_t value = transport_value();
    /* A control query may exhaust its 20 ms identity budget under descheduling.
     * Scope ownership is asserted directly; Rust fake-clock tests own identity policy. */
    assert(value == kAudioDeviceTransportTypeUSB || value == kAudioDeviceTransportTypeVirtual);
}

static bool timestamp_fixture(void *runtime, uint64_t time, MRHALCoreTimestamp *stamp) {
    (void)runtime; (void)time; (void)stamp;
    assert(transport_value() == kAudioDeviceTransportTypeVirtual);
    return false;
}
static bool prepare_client_fixture(void *runtime, uint32_t client, void **change) {
    (void)runtime;
    *change = (void *)(uintptr_t)(client + 1U);
    return true;
}
static void release_fixture(void *runtime) { (void)runtime; }
static bool client_fixture(void *runtime, uint32_t client, uint32_t action, void *change) {
    (void)runtime; (void)client; (void)action; (void)change; return true;
}
static MRHALCoreIoOutcome io_fixture(void *runtime, bool render, uint32_t client, uint64_t frame, float *data, uint32_t count) {
    (void)runtime; (void)render; (void)client; (void)frame; (void)data; (void)count;
    assert(transport_value() == kAudioDeviceTransportTypeVirtual);
    return (MRHALCoreIoOutcome){false, 0};
}
static bool replacement_fixture(void *runtime) { (void)runtime; return false; }
static bool same_fixture(void *runtime, int fd, size_t size) { (void)runtime; (void)fd; (void)size; return false; }
static void transport_identity(void) {
    const MRHALCoreNativeOps native = {inspect_fixture, create_fixture, destroy_fixture, prepare_fixture,
        discard_fixture, release_fixture, replace_fixture, start_fixture, stop_fixture, stop_fixture, rate_fixture,
        timestamp_fixture, prepare_client_fixture, client_fixture, io_fixture, replacement_fixture, same_fixture};
    const MRHALCoreHostOps host = {NULL, fixture_changed, fixture_request};
    gCore = mr_hal_core_create(&native, &host);
    assert(gCore != NULL);
    FILE *file = tmpfile();
    assert(file != NULL);
    uint8_t endpoint[16]; memset(endpoint, 1, sizeof(endpoint));
    uint32_t device = 0;
    assert(mr_hal_core_upsert(gCore, endpoint, "Fixture", 48000, dup(fileno(file)), 24, &device) == MR_HAL_CORE_OK);
    fclose(file);

    xpc_object_t inventory = mr_hal_control_create_inventory_snapshot(NULL, 0, 1);
    assert(inventory != NULL);
    xpc_object_t policy = xpc_array_create(NULL, 0);
    xpc_object_t bundle = xpc_string_create("com.test.Copied");
    xpc_array_append_value(policy, bundle);
    xpc_dictionary_set_value(inventory, "transport_compatibility", policy);
    xpc_release(bundle); xpc_release(policy);
    /* Avoid authoritative device removal: establish policy on an empty registry,
     * then recreate this fixture with its new object generation. */
    assert(mr_hal_core_control_connected(gCore, 1));
    assert(mr_hal_core_control_message(gCore, 1, inventory));
    xpc_release(inventory);
    file = tmpfile(); assert(file != NULL);
    assert(mr_hal_core_upsert(gCore, endpoint, "Fixture", 48000, dup(fileno(file)), 24, &device) == MR_HAL_CORE_OK);
    fclose(file);
    CFMutableStringRef identity = CFStringCreateMutable(NULL, 0);
    CFStringAppendCString(identity, "com.test.Copied", kCFStringEncodingUTF8);
    AudioServerPlugInClientInfo client = {.mClientID = 7, .mProcessID = getpid(), .mBundleID = identity};
    assert(add_client(gDriver, device, &client) == noErr);
    CFStringReplaceAll(identity, CFSTR("com.test.Changed"));
    CFRelease(identity);
    client.mBundleID = NULL;
    AudioObjectPropertyAddress property = {kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, 0};
    uint32_t value = 0, written = 0;
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr);
    assert(forwarded_pid == getpid() && written == 4);
    assert(value == kAudioDeviceTransportTypeUSB || value == kAudioDeviceTransportTypeVirtual);
    assert(property_get(gDriver, device, 0, &property, 0, NULL, 4, &written, &value) == noErr);
    assert(forwarded_pid == 0 && value == kAudioDeviceTransportTypeVirtual);
    transport_device = device;
    custom_property_lists(device);
    Boolean will = false, in_place = false;
    assert((*gDriver)->WillDoIOOperation(gDriver, device, 7, kAudioServerPlugInIOOperationMixOutput, &will, &in_place) == noErr);
    assert(will && in_place);
    assert(will_io(gDriver, device, 7, kAudioServerPlugInIOOperationWriteMix, &will, &in_place) == noErr);
    assert(!will && in_place);
    /* WillDo is a startup query, not an IO-thread scope. */
    assert_control_transport();
    assert(will_io(NULL, device, 7, 0, &will, &in_place) == kAudioHardwareBadObjectError);
    assert_control_transport();
    AudioServerPlugInIOCycleInfo cycle = {0};
    float samples[2] = {0};
    assert(start_io(gDriver, device, 7) == noErr);
    assert((*gDriver)->BeginIOOperation(gDriver, device, 7, kAudioServerPlugInIOOperationMixOutput, 1, &cycle) == noErr);
    assert(transport_value() == kAudioDeviceTransportTypeVirtual);
    /* Native IO reenters the property path; its nested scope cannot end the cycle. */
    assert(do_io(gDriver, device, device + 2, 7, kAudioServerPlugInIOOperationMixOutput, 1, &cycle, samples, NULL) == kAudioHardwareIllegalOperationError);
    assert(transport_value() == kAudioDeviceTransportTypeVirtual);
    assert(rendered_client == 7 && rendered_device == device);
    samples[0] = 0.125F; samples[1] = -0.25F;
    assert(do_io(gDriver, device, device + 2, 7, kAudioServerPlugInIOOperationProcessOutput, 1, &cycle, samples, NULL) == noErr);
    assert(rendered_client == 7 && rendered_device == device);
    assert(samples[0] == 0.125F && samples[1] == -0.25F);
    assert(do_io(gDriver, device, device + 2, 7, 0, 1, NULL, samples, NULL) == kAudioHardwareBadObjectError);
    assert(transport_value() == kAudioDeviceTransportTypeVirtual);
    assert((*gDriver)->EndIOOperation(gDriver, device, 7, kAudioServerPlugInIOOperationMixOutput, 1, &cycle) == noErr);
    assert_control_transport();
    /* Standalone DoIO and timestamp must clear every success/error return. */
    assert(do_io(gDriver, device, device + 2, 7, kAudioServerPlugInIOOperationMixOutput, 1, &cycle, samples, NULL) == kAudioHardwareIllegalOperationError);
    assert_control_transport();
    assert(do_io(NULL, device, device + 2, 7, 0, 1, &cycle, samples, NULL) == kAudioHardwareBadObjectError);
    assert_control_transport();
    assert(do_io(gDriver, device, device + 2, 7, 0, 1, &cycle, samples, NULL) == noErr);
    assert_control_transport();
    Float64 sample = 0; UInt64 time = 0, seed = 0;
    assert(timestamp(gDriver, device, 7, &sample, &time, &seed) == kAudioHardwareIllegalOperationError);
    assert_control_transport();
    assert(timestamp(NULL, device, 7, &sample, &time, &seed) == kAudioHardwareBadObjectError);
    assert_control_transport();
    /* The Rust FFI entry points also scope IO when called without the bridge. */
    MRHALCoreTimestamp stamp = {0};
    assert(!mr_hal_core_timestamp(gCore, device, mach_absolute_time(), &stamp));
    assert_control_transport();
    assert(!mr_hal_core_io(gCore, device, device + 2, true, 42, 0, samples, 1));
    assert_control_transport();
    assert(begin_io(NULL, device, 7, 0, 1, &cycle) == kAudioHardwareBadObjectError);
    assert_control_transport();
    assert(stop_io(gDriver, device, 7) == noErr);
    property.mSelector = kAudioDevicePropertyDeviceIsAlive;
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr);
    assert(forwarded_pid == getpid() && value == 1);
    assert(remove_client(gDriver, device, &client) == noErr);
    client.mClientID = 8;
    assert(add_client(gDriver, device, &client) == noErr);
    assert(remove_client(gDriver, device, &client) == noErr);
    assert(add_client(gDriver, device, NULL) == kAudioHardwareBadObjectError);
    mr_hal_core_destroy(gCore); gCore = NULL;
}

/* The SDK GetPropertyData entry forwards requester identity: apps read measured
 * content delay while coreaudiod keeps zero scheduling latency. */
static void input_latency_requester(void) {
    const MRHALCoreNativeOps native = {inspect_fixture, create_fixture, destroy_fixture, prepare_fixture,
        discard_fixture, release_fixture, replace_fixture, start_fixture, stop_fixture, stop_fixture, rate_fixture,
        timestamp_fixture, prepare_client_fixture, client_fixture, io_fixture, replacement_fixture, same_fixture};
    const MRHALCoreHostOps host = {NULL, fixture_changed, fixture_request};
    gCore = mr_hal_core_create(&native, &host);
    assert(gCore != NULL);
    FILE *file = tmpfile();
    assert(file != NULL);
    uint8_t endpoint[16]; memset(endpoint, 1, sizeof(endpoint));
    uint32_t device = 0;
    assert(mr_hal_core_upsert(gCore, endpoint, "Fixture", 48000, dup(fileno(file)), 24, &device) == MR_HAL_CORE_OK);
    fclose(file);

    xpc_object_t inventory = mr_hal_control_create_inventory_snapshot(NULL, 0, 1);
    assert(inventory != NULL);
    xpc_object_t latencies = xpc_array_create(NULL, 0);
    xpc_object_t latency = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_uint64(latency, "version", 1);
    xpc_dictionary_set_string(latency, "operation", "latency");
    xpc_dictionary_set_data(latency, "endpoint_id", endpoint, sizeof(endpoint));
    xpc_dictionary_set_uint64(latency, "rate", 48000);
    xpc_dictionary_set_uint64(latency, "engine_pid", 4242);
    xpc_dictionary_set_uint64(latency, "lease", 1);
    xpc_dictionary_set_uint64(latency, "revision", 1);
    xpc_dictionary_set_uint64(latency, "known", 1);
    xpc_dictionary_set_uint64(latency, "frames", 321);
    xpc_array_append_value(latencies, latency);
    xpc_dictionary_set_value(inventory, "input_latencies", latencies);
    xpc_release(latency); xpc_release(latencies);
    assert(mr_hal_core_control_connected(gCore, 1));
    assert(mr_hal_core_control_message(gCore, 1, inventory));
    xpc_release(inventory);

    AudioObjectPropertyAddress property = {kAudioDevicePropertyLatency, kAudioObjectPropertyScopeInput, 0};
    uint32_t value = 0, written = 0;
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr);
    assert(forwarded_pid == getpid() && written == 4 && value == 321);
    /* Content-delay publication cannot move the host's input coordinates. */
    assert(property_get(gDriver, device, 0, &property, 0, NULL, 4, &written, &value) == noErr);
    assert(forwarded_pid == 0 && written == 4 && value == 0);
    AudioServerPlugInClientInfo client = {.mClientID = 7, .mProcessID = getpid(), .mBundleID = CFSTR("com.test.Renderer")};
    assert(add_client(gDriver, device, &client) == noErr);
    assert(start_io(gDriver, device, 7) == noErr);
    AudioServerPlugInIOCycleInfo cycle = {0};
    float samples[2] = {0.125F, -0.25F};
    transport_device = device;
    assert(do_io(gDriver, device, device + 2, kAudioServerPlugInHostClientID,
        kAudioServerPlugInIOOperationMixOutput, 1, &cycle, samples, NULL) == kAudioHardwareIllegalOperationError);
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr && value == 321);
    assert(do_io(gDriver, device, device + 2, 7,
        kAudioServerPlugInIOOperationMixOutput, 1, &cycle, samples, NULL) == kAudioHardwareIllegalOperationError);
    assert(samples[0] == 0.125F && samples[1] == -0.25F);
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr && value == 0);
    assert(stop_io(gDriver, device, 7) == noErr);
    assert(property_get(gDriver, device, getpid(), &property, 0, NULL, 4, &written, &value) == noErr && value == 321);
    assert(remove_client(gDriver, device, &client) == noErr);
    mr_hal_core_destroy(gCore); gCore = NULL;
}

#include "MRHALBridgePCMTests.h"

int main(void) {
    assert(MixRelayHAL_Create(NULL, NULL) == NULL);
    assert(MixRelayHAL_Create(NULL, kAudioServerPlugInTypeUUID) == gDriver);
    assert(add_ref(NULL) == 0 && release_ref(NULL) == 0);
    assert(initialize(gDriver, NULL) == kAudioHardwareIllegalOperationError);
    assert(gCore == NULL);
    void *output = NULL;
    assert(query(gDriver, CFUUIDGetUUIDBytes(IUnknownUUID), &output) == S_OK);
    assert(output == gDriver);
    assert(release_ref(gDriver) == 1);
    assert(query(gDriver, CFUUIDGetUUIDBytes(kAudioServerPlugInDriverInterfaceUUID), &output) == S_OK);
    assert(output == gDriver);
    assert(release_ref(gDriver) == 1);
    CFUUIDRef unknown = CFUUIDCreate(NULL);
    assert(unknown != NULL);
    assert(query(gDriver, CFUUIDGetUUIDBytes(unknown), &output) == E_NOINTERFACE);
    assert(output == NULL);
    CFRelease(unknown);
    assert(release_ref(gDriver) == 0);
    assert(release_ref(gDriver) == 0);
    assert(add_ref(gDriver) == 1);
    MRHALCoreNotification latency_notice = {0};
    assert(mr_hal_core_notifications(10, MR_HAL_CORE_CHANGE_INPUT_LATENCY, &latency_notice, 1) == 1);
    assert(latency_notice.object_id == 10);
    assert(latency_notice.address.selector == kAudioDevicePropertyLatency);
    assert(latency_notice.address.scope == kAudioObjectPropertyScopeInput);
    transport_identity();
    input_latency_requester();
    registered_client_pcm();
    puts("PASS HAL COM lifetime, transport and latency PID forwarding, copied and nullable bundle identity, balanced IO scopes, registered client PCM");
    return 0;
}
