/* SDK entry points coupled to the production Rust runtime on an anonymous
 * mapping. This never registers a system device or starts a native stream. */
#include <sys/mman.h>
#include "../../common/MRAnonymousMemory.h"
#include "MRHALClockPropertyTests.h"

static void registered_client_pcm(void) {
    const MRHALCoreHostOps host = {NULL, fixture_changed, fixture_request};
    gCore = mr_hal_core_create_native(&host);
    assert(gCore != NULL);
    MREndpointTransportConfig config = {.capacity_frames = 256, .render_channels = 2, .capture_channels = 2};
    memset(config.endpoint_id, 1, sizeof(config.endpoint_id));
    const size_t size = mr_endpoint_transport_required_bytes(&config);
    assert(size != 0);
    const int fd = mr_shared_memory_create(size, true);
    assert(fd >= 0);
    void *mapping = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    assert(mapping != MAP_FAILED);
    assert(mr_endpoint_transport_initialize(mapping, size, &config) != NULL);
    uint32_t device = 0;
    assert(mr_hal_core_upsert(gCore, config.endpoint_id, "PCM fixture", 48000,
        fd, size, &device) == MR_HAL_CORE_OK);
    assert_clock_properties(device);
    observed_enter_audio();
    assert_clock_properties(device);
    observed_leave_audio();
    AudioServerPlugInClientInfo owner = {.mClientID = 7};
    AudioServerPlugInClientInfo output = {.mClientID = 8};
    assert(add_client(gDriver, device, &owner) == noErr);
    assert(start_io(gDriver, device, owner.mClientID) == noErr);
    assert(add_client(gDriver, device, &output) == noErr);
    AudioServerPlugInIOCycleInfo cycle = {0};
    cycle.mOutputTime.mSampleTime = 64;
    cycle.mInputTime.mSampleTime = 64;
    float signal[64], captured[64];
    for (unsigned index = 0; index < 64; ++index) signal[index] = (float)(index + 1U) / 256.0F;
    assert(do_io(gDriver, device, device + 2, output.mClientID,
        kAudioServerPlugInIOOperationMixOutput, 32, &cycle, signal, NULL) == noErr);
    assert(do_io(gDriver, device, device + 1, owner.mClientID,
        kAudioServerPlugInIOOperationReadInput, 32, &cycle, captured, NULL) == noErr);
    assert(memcmp(captured, signal, sizeof(signal)) == 0);
    assert(start_io(gDriver, device, output.mClientID) == noErr);
    assert(stop_io(gDriver, device, output.mClientID) == noErr);
    assert(do_io(gDriver, device, device + 1, owner.mClientID,
        kAudioServerPlugInIOOperationReadInput, 32, &cycle, captured, NULL) == noErr);
    assert(memcmp(captured, signal, sizeof(signal)) == 0);
    assert(stop_io(gDriver, device, owner.mClientID) == noErr);
    assert(start_io(gDriver, device, owner.mClientID) == noErr);
    assert(do_io(gDriver, device, device + 1, owner.mClientID,
        kAudioServerPlugInIOOperationReadInput, 32, &cycle, captured, NULL) == noErr);
    for (unsigned index = 0; index < 64; ++index) assert(captured[index] == 0.0F);
    assert(stop_io(gDriver, device, owner.mClientID) == noErr);
    assert(remove_client(gDriver, device, &output) == noErr);
    assert(remove_client(gDriver, device, &owner) == noErr);
    assert(mr_hal_core_remove(gCore, config.endpoint_id) == MR_HAL_CORE_OK);
    mr_hal_core_destroy(gCore); gCore = NULL;
    assert(munmap(mapping, size) == 0);
}
