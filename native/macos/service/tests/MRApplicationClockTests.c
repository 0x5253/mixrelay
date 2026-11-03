#include "MRApplicationClock.h"
#include "MREngineSourceRegistry.h"
#include "MRApplicationSourceTransport.h"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>
_Static_assert(sizeof(MRApplicationClock) == 176U, "v7 clock header ABI; PCM coordinates follow it");
_Static_assert(offsetof(MRApplicationClock, target_backlog_frames) == 96U, "target offset");
_Static_assert(sizeof(MRApplicationSourceClockSnapshot) == 96U, "source snapshot ABI");
_Static_assert(offsetof(MRApplicationSourceClockSnapshot, starvation_episodes) == 56U, "telemetry offset");
/* Exercise the real producer mapping/epoch boundary without installing an XPC
 * service, creating a process tap, or touching the host's audio settings. */

static void test_clock_trailer_validation(void) {
    const size_t bytes = mr_application_clock_required_bytes(1024U, 2U);
    void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    assert(memory != MAP_FAILED);
    MRSharedAudioBuffer *buffer = mr_shared_audio_buffer_initialize(memory, bytes, 1024U, 2U);
    assert(buffer != NULL);
    MRApplicationClock *clock = mr_application_clock_initialize(memory, bytes, 1024U, 2U);
    assert(clock != NULL);
    assert(mr_shared_audio_buffer_attach(memory, bytes) == buffer);
    assert(mr_application_clock_attach(memory, mr_shared_audio_buffer_required_bytes(1024U, 2U), 1024U, 2U) == NULL);
    assert(mr_application_clock_attach(memory, bytes - 1U, 1024U, 2U) == NULL);
    assert(mr_application_clock_attach((char *)memory + 1U, bytes, 1024U, 2U) == NULL);
    assert(mr_application_clock_initialize(NULL, bytes, 1024U, 2U) == NULL);
    assert(mr_application_clock_required_bytes(0U, 2U) == 0U);
    clock->version = 2U;
    assert(mr_application_clock_attach(memory, bytes, 1024U, 2U) == NULL);
    clock->version = MR_APPLICATION_CLOCK_VERSION;
    clock->magic = 0U;
    assert(mr_application_clock_attach(memory, bytes, 1024U, 2U) == NULL);
    clock->magic = MR_APPLICATION_CLOCK_MAGIC;
    clock->header_bytes -= 1U;
    assert(mr_application_clock_attach(memory, bytes, 1024U, 2U) == NULL);
    clock->header_bytes = sizeof(*clock);
    assert(mr_application_clock_try_producer(clock));
    assert(!mr_application_clock_try_producer(clock));
    mr_application_clock_release_producer(clock);
    assert(mr_application_clock_try_producer(clock));
    mr_application_clock_release_producer(clock);
    assert(mr_application_clock_request_rate(clock, 0U) == 0U);
    const uint64_t first = mr_application_clock_request_rate(clock, 44100U);
    assert((uint32_t)first == 44100U);
    assert(mr_application_clock_request_rate(clock, 44100U) == first);
    assert(mr_application_clock_request_rate(clock, 96000U) != first);
    mr_application_clock_publish_consumer(clock, 7U, 900U, 128U, true, 512U);
    assert(atomic_load(&clock->consumer_position) == ((UINT64_C(7) << 48U) | 900U));
    assert(atomic_load(&clock->target_backlog_frames) == 512U);
    assert(atomic_load(&clock->consumer_ticks) == 128U);
    assert(atomic_load(&clock->consumer_running) == 1U);
    assert((atomic_load(&clock->consumer_sequence) & 1U) == 0U);
    assert(munmap(memory, bytes) == 0);
}

static void test_clocked_registry_rejects_unacknowledged_or_old_rate_audio(void) {
    MREngineSourceRegistry *registry = mr_engine_source_registry_create();
    MREngineSourceRequest request = { .source_id = "clocked-fixture", .capacity_frames = 1024U,
        .channels = 2U, .transport_version = MR_APPLICATION_CLOCK_VERSION };
    int descriptor = -1;
    size_t bytes = 0U;
    uint64_t granted_lease = 0U;
    uint64_t granted_epoch = 0U;
    assert(mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &granted_lease, &granted_epoch));
    void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, descriptor, 0);
    assert(memory != MAP_FAILED);
    MRSharedAudioBuffer *buffer = mr_shared_audio_buffer_attach(memory, bytes);
    MRApplicationClock *clock = mr_application_clock_attach(memory, bytes, 1024U, 2U);
    assert(buffer != NULL && clock != NULL);
    assert(granted_lease != 0U && granted_lease == atomic_load(&clock->producer_lease));
    assert(granted_epoch == mr_shared_audio_buffer_stats(buffer).epoch);
    MREngineSourceAttachment attachment = { descriptor, bytes, granted_lease, granted_epoch };
    MRApplicationSourceTransport *producer = mr_application_source_transport_attach(&attachment, 1024U, 2U, NULL);
    assert(producer != NULL && attachment.descriptor == -1);
    descriptor = -1;
    float input[768U * 2U];
    float output[128U * 2U];
    for (size_t i = 0U; i < 768U * 2U; ++i) input[i] = 0.25f;
    const uint64_t token = mr_engine_source_id_token(request.source_id);
    assert(mr_shared_audio_buffer_write(buffer, granted_epoch, 0U, input, 768U));
    assert(mr_engine_source_registry_read_clocked(registry, token, output, 128U, 2U, 44100U) == 0U);
    const uint64_t format = atomic_load(&clock->format_word);
    assert((uint32_t)format == 44100U);
    assert(output[0] == 0.0f && atomic_load(&clock->consumer_running) == 0U);
    assert(mr_application_source_transport_reset(producer, format, 48000U));
    assert(mr_application_clock_try_producer(clock));
    assert(!mr_application_source_transport_write(producer, input, 128U));
    assert(!mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &granted_lease, &granted_epoch));
    mr_application_clock_release_producer(clock);
    assert(mr_application_source_transport_write(producer, input, 768U));
    mr_application_source_transport_publish_native(producer, 1200U, true);
    assert(mr_engine_source_registry_read_clocked(registry, token, output, 128U, 2U, 44100U) == 128U);
    assert(output[0] == 0.25f);
    assert((atomic_load(&clock->consumer_position) & MR_APPLICATION_CLOCK_FRAME_MASK) == 384U);
    assert(atomic_load(&clock->consumer_running) == 1U);
    MRApplicationSourceClock native_clock = {0};
    assert(mr_engine_source_registry_clock(registry, token, &native_clock));
    assert(native_clock.native_rate == 48000U && native_clock.graph_rate == 44100U);
    assert(native_clock.native_frames == 1200U && native_clock.synchronized == 1U);
    MRApplicationSourceClockSnapshot snapshot = {0};
    assert(mr_application_source_transport_clock(producer, &snapshot));
    assert(snapshot.produced_frames == 768U && snapshot.consumed_frames == 384U);
    assert(mr_engine_source_registry_set_clock_master(registry, token, true));
    assert(mr_application_source_transport_clock(producer, &snapshot) && snapshot.clock_master == 1U);
    assert(mr_engine_source_registry_set_clock_master(registry, token, false));
    assert(mr_application_source_transport_clock(producer, &snapshot) && snapshot.clock_master == 0U);
    atomic_fetch_add(&clock->consumer_sequence, 1U);
    assert(!mr_application_source_transport_clock(producer, &snapshot));
    atomic_fetch_add(&clock->consumer_sequence, 1U);
    assert(mr_engine_source_registry_read_clocked(registry, token, output, 128U, 2U, 96000U) == 0U);
    assert(atomic_load(&clock->format_word) != format);
    assert(!mr_application_source_transport_write(producer, input, 128U));
    assert(!mr_engine_source_registry_clock(registry, token, &native_clock));
    /* Reconnect invalidates the old producer acknowledgement and stream epoch. */
    const uint64_t old_epoch = mr_shared_audio_buffer_stats(buffer).epoch;
    assert(mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &granted_lease, &granted_epoch));
    assert(mr_shared_audio_buffer_stats(buffer).epoch != old_epoch);
    assert(atomic_load(&clock->acknowledged_format) == 0U);
    assert(!mr_application_source_transport_clock(producer, &snapshot));
    assert(!mr_application_source_transport_reset(producer, atomic_load(&clock->format_word), 48000U));
    assert(!mr_application_source_transport_write(producer, input, 128U));
    request.transport_version = 0U;
    /* Copying a current epoch cannot let a retired lease impersonate its owner. */
    MREngineSourceAttachment stale_grant = { dup(descriptor), bytes, 1U,
        mr_shared_audio_buffer_stats(buffer).epoch };
    assert(mr_application_source_transport_attach(&stale_grant, 1024U, 2U, NULL) == NULL);
    assert(close(descriptor) == 0); descriptor = -1;
    assert(!mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &granted_lease, &granted_epoch));
    assert(munmap(memory, bytes) == 0);
    mr_application_source_transport_destroy(producer);
    mr_engine_source_registry_destroy(registry);
}

static void test_invalid_source_version_never_creates_a_slot(void) {
    MREngineSourceRegistry *registry = mr_engine_source_registry_create();
    MREngineSourceRequest request = { .source_id = "invalid-version", .capacity_frames = 1024U, .channels = 1U };
    int descriptor = -1;
    size_t bytes = 0U;
    uint64_t lease = 0U, epoch = 0U;
    assert(!mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch));
    request.transport_version = 99U;
    assert(!mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch));
    assert(descriptor == -1 && bytes == 0U && lease == 0U && epoch == 0U);
    const uint64_t token = mr_engine_source_id_token(request.source_id);
    MRApplicationSourceClock snapshot;
    assert(!mr_engine_source_registry_clock(registry, token, &snapshot));
    assert(!mr_engine_source_registry_set_clock_master(registry, token, true));
    request.transport_version = MR_APPLICATION_CLOCK_VERSION;
    assert(mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch));
    assert(descriptor >= 0 && bytes != 0U && lease != 0U && epoch != 0U);
    assert(close(descriptor) == 0);
    mr_engine_source_registry_destroy(registry);
}

static void test_reset_exhaustion_does_not_acknowledge_format(void) {
    MREngineSourceRegistry *registry = mr_engine_source_registry_create();
    MREngineSourceRequest request = { .source_id = "reset-exhaustion", .capacity_frames = 1024U,
        .channels = 2U, .transport_version = MR_APPLICATION_CLOCK_VERSION };
    MREngineSourceAttachment attachment = { .descriptor = -1 };
    assert(mr_engine_source_registry_attach_clocked(registry, &request,
        &attachment.descriptor, &attachment.mapped_size, &attachment.producer_lease, &attachment.source_epoch));
    const size_t bytes = attachment.mapped_size;
    void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, attachment.descriptor, 0);
    assert(memory != MAP_FAILED);
    MRSharedAudioBuffer *buffer = mr_shared_audio_buffer_attach(memory, bytes);
    MRApplicationClock *clock = mr_application_clock_attach(memory, bytes, 1024U, 2U);
    assert(buffer != NULL && clock != NULL);
    /* Use the public API, not a private header poke: no epoch is reused. */
    uint64_t owned_epoch = mr_shared_audio_buffer_stats(buffer).epoch;
    for (uint64_t expected = owned_epoch + 1U; expected <= UINT16_MAX; ++expected) {
        owned_epoch = mr_shared_audio_buffer_try_begin_epoch(buffer);
        assert(owned_epoch == expected);
    }
    const uint64_t format = mr_application_clock_request_rate(clock, 48000U);
    atomic_store(&clock->acknowledged_format, format);
    atomic_store(&clock->native_running, 1U);
    atomic_store(&clock->native_sample_rate, 44100U);
    attachment.source_epoch = owned_epoch;
    MRApplicationSourceTransport *producer = mr_application_source_transport_attach(&attachment, 1024U, 2U, NULL);
    assert(producer != NULL);
    assert(!mr_application_source_transport_reset(producer, format, 48000U));
    assert(atomic_load(&clock->acknowledged_format) == 0U);
    assert(atomic_load(&clock->native_running) == 0U);
    assert(atomic_load(&clock->native_sample_rate) == 44100U);
    assert(mr_shared_audio_buffer_requires_replacement(buffer));
    assert(mr_application_clock_try_producer(clock));
    mr_application_clock_release_producer(clock);
    const float samples[2] = {0.25f, -0.25f};
    assert(!mr_application_source_transport_write(producer, samples, 1U));
    assert(munmap(memory, bytes) == 0);
    mr_application_source_transport_destroy(producer);
    mr_engine_source_registry_destroy(registry);
}

static void test_exhausted_attachment_replaces_the_actual_mapping(void) {
    MREngineSourceRegistry *registry = mr_engine_source_registry_create();
    MREngineSourceRequest request = { .source_id = "exhausted-attachment", .capacity_frames = 1024U,
        .channels = 1U, .transport_version = MR_APPLICATION_CLOCK_VERSION };
    int descriptor = -1;
    size_t bytes = 0U;
    uint64_t lease = 0U, epoch = 0U;
    assert(mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch));
    void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, descriptor, 0);
    assert(memory != MAP_FAILED);
    MRSharedAudioBuffer *buffer = mr_shared_audio_buffer_attach(memory, bytes);
    MRApplicationClock *clock = mr_application_clock_attach(memory, bytes, 1024U, 1U);
    assert(buffer != NULL && clock != NULL);
    for (uint64_t next = epoch + 1U; next <= UINT16_MAX; ++next) {
        assert(mr_shared_audio_buffer_try_begin_epoch(buffer) == next);
    }
    const uint64_t previous_lease = lease;
    assert(close(descriptor) == 0);
    descriptor = -1; bytes = 0U; lease = 0U; epoch = 0U;
    assert(mr_engine_source_registry_attach_clocked(registry, &request, &descriptor, &bytes, &lease, &epoch));
    assert(descriptor >= 0 && bytes != 0U && lease > previous_lease && epoch != 0U);
    void *replacement = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, descriptor, 0);
    assert(replacement != MAP_FAILED);
    assert(close(descriptor) == 0);
    MRSharedAudioBuffer *new_buffer = mr_shared_audio_buffer_attach(replacement, bytes);
    assert(new_buffer != NULL && new_buffer != buffer);
    const float pcm[1] = { 0.25f };
    assert(mr_shared_audio_buffer_write(new_buffer, epoch, 0U, pcm, 1U));
    assert(atomic_load(&clock->producer_lease) == 0U);
    assert(mr_shared_audio_buffer_requires_replacement(buffer));
    assert(mr_application_clock_try_producer(clock));
    mr_application_clock_release_producer(clock);
    assert(munmap(memory, mr_application_clock_required_bytes(1024U, 1U)) == 0);
    assert(munmap(replacement, bytes) == 0);
    mr_engine_source_registry_destroy(registry);
}

int main(void) {
    test_exhausted_attachment_replaces_the_actual_mapping();
    test_reset_exhaustion_does_not_acknowledge_format();
    test_clock_trailer_validation();
    test_clocked_registry_rejects_unacknowledged_or_old_rate_audio();
    test_invalid_source_version_never_creates_a_slot();
    puts("MRApplicationClockTests passed");
    return 0;
}
