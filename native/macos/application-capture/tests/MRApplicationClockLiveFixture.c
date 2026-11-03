/* Explicit test-only build: reuse the production producer implementation while
 * injecting an in-process registry attachment instead of an installed XPC service. */
#include "MRApplicationSourceTransport.h"
#include "MRApplicationClock.h"
#include "MREngineSourceRegistry.h"
#include <unistd.h>

MREngineSourceRegistry *mr_application_clock_fixture_create(MRApplicationSourceTransport **output) {
    if (output == NULL) return NULL;
    *output = NULL;
    MREngineSourceRegistry *registry = mr_engine_source_registry_create();
    MREngineSourceRequest request = { .source_id = "owned-live-clock-fixture", .capacity_frames = 16384U,
        .channels = 2U, .transport_version = MR_APPLICATION_CLOCK_VERSION };
    int descriptor = -1;
    size_t bytes = 0U;
    uint64_t lease = 0U, epoch = 0U;
    if (!mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch)) goto fail;
    MREngineSourceAttachment attachment = { descriptor, bytes, lease, epoch };
    descriptor = -1;
    MRApplicationSourceTransport *transport = mr_application_source_transport_attach(
        &attachment, 16384U, 2U, NULL);
    if (transport == NULL) goto fail;
    *output = transport;
    return registry;
fail:
    if (descriptor >= 0) close(descriptor);
    mr_engine_source_registry_destroy(registry);
    return NULL;
}

uint32_t mr_application_clock_fixture_read(MREngineSourceRegistry *registry, float *samples, uint32_t rate) {
    return mr_engine_source_registry_read_clocked(registry,
        mr_engine_source_id_token("owned-live-clock-fixture"), samples, 128U, 2U, rate);
}
bool mr_application_clock_fixture_clock(MREngineSourceRegistry *registry, MRApplicationSourceClock *clock) {
    return mr_engine_source_registry_clock(registry, mr_engine_source_id_token("owned-live-clock-fixture"), clock);
}
