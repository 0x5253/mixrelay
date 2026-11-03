#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <assert.h>
#include <pipewire/pipewire.h>

static struct pw_buffer *next_buffer;
static unsigned returned_buffers;
static int return_status;
static struct pw_buffer *test_dequeue(struct pw_stream *stream) {
    (void)stream;
    return next_buffer;
}
static int test_queue(struct pw_stream *stream, struct pw_buffer *buffer) {
    (void)stream;
    assert(buffer == next_buffer);
    returned_buffers++;
    return return_status;
}
#define pw_stream_dequeue_buffer test_dequeue
#define pw_stream_queue_buffer test_queue
#include "../MRPipeWire.c"
#undef pw_stream_dequeue_buffer
#undef pw_stream_queue_buffer

static void test_output(uint32_t encoding) {
    MRPWStream stream = { .native_output = true, .channels = 2 };
    initialize_transfer_observations(&stream);
    atomic_init(&stream.position, NULL);
    atomic_init(&stream.clock_snapshot, 0);
    atomic_init(&stream.quantum_snapshot, 0);
    atomic_init(&stream.format_snapshot, (uint64_t)encoding << 16);
    atomic_init(&stream.format_valid, true);
    assert(mr_pcm_ring_init(&stream.ring, 2, 8));
    const float samples[8] = {0.25f, -0.25f, 0.5f, -0.5f, 0.75f, -0.75f, 0, 0};
    assert(mr_pcm_ring_write(&stream.ring, samples, 2) == 2);
    assert(mr_pcm_ring_write(&stream.ring, samples + 4, 2) == 2);
    union { float aligned[6]; unsigned char bytes[24]; } storage = {0};
    const uint32_t stride = 2 * sample_bytes(encoding);
    struct spa_chunk chunk = {0};
    struct spa_data data = { .data = storage.bytes, .maxsize = 3 * stride, .chunk = &chunk };
    struct spa_buffer spa_buffer = { .n_datas = 1, .datas = &data };
    struct pw_buffer buffer = { .buffer = &spa_buffer, .size = 99, .requested = 2 };
    next_buffer = &buffer;
    unsigned before = returned_buffers;
    stream_process_buffer(&stream);
    assert(buffer.size == 2 && chunk.size == 2 * stride);
    assert(chunk.offset == 0 && chunk.stride == (int32_t)stride);
    assert(returned_buffers == before + 1);
    if (encoding == MR_PCM_FORMAT_FLOAT32)
        assert(memcmp(storage.aligned, samples, 4 * sizeof(float)) == 0);

    // Only two frames remain: the ring emits a whole silent three-frame block.
    buffer.requested = 100;
    memset(storage.bytes, 0xff, sizeof(storage.bytes));
    stream_process_buffer(&stream);
    assert(buffer.size == 3 && chunk.size == 3 * stride);
    for (uint32_t i = 0; i < chunk.size; ++i) assert(storage.bytes[i] == 0);
    assert(returned_buffers == before + 2);

    atomic_store(&stream.format_valid, false);
    stream_process_buffer(&stream);
    assert(buffer.size == 0 && chunk.size == 0);
    assert(returned_buffers == before + 3);
    buffer.size = 99;
    spa_buffer.n_datas = 0;
    stream_process_buffer(&stream);
    assert(buffer.size == 0 && returned_buffers == before + 4);
    next_buffer = NULL;
    stream_process_buffer(&stream);
    assert(returned_buffers == before + 4);
    mr_pcm_ring_clear(&stream.ring);
}

static void test_return_failure_isolates_the_acquisition(void) {
    MRPWStream stream = { .channels = 2, .sample_rate = 48000 };
    initialize_transfer_observations(&stream);
    atomic_init(&stream.state, PW_STREAM_STATE_STREAMING);
    struct spa_io_position position = {0};
    position.clock.rate = SPA_FRACTION(1, 48000);
    position.clock.id = 17;
    position.clock.duration = 128;
    atomic_init(&stream.position, &position);
    atomic_init(&stream.clock_snapshot, UINT64_C(17) << 32 | 48000);
    atomic_init(&stream.quantum_snapshot, UINT64_C(128) << 32 | 128);
    atomic_init(&stream.format_snapshot, UINT64_C(48000) << 32 | UINT64_C(1) << 16 | 2);
    atomic_init(&stream.format_valid, true);
    assert(mr_pcm_ring_init(&stream.ring, 2, 8));
    float samples[8] = {0.25f, -0.25f, 0.5f, -0.5f, 0.75f, -0.75f, 0, 0};
    struct spa_chunk chunk = { .size = sizeof(samples), .stride = 2 * sizeof(float) };
    struct spa_data data = { .data = samples, .maxsize = sizeof(samples), .chunk = &chunk };
    struct spa_buffer spa_buffer = { .n_datas = 1, .datas = &data };
    struct pw_buffer buffer = { .buffer = &spa_buffer };
    next_buffer = &buffer;
    unsigned before = returned_buffers;
    return_status = -EINVAL;
    stream_process_buffer(&stream);
    assert(returned_buffers == before + 1);
    assert(mr_pw_stream_return_error(&stream) == -EINVAL);
    assert(mr_pw_stream_failed(&stream) && !mr_pw_stream_ready(&stream));
    assert(!mr_pw_stream_connected(&stream) && mr_pw_stream_input_activity(&stream) == 0);
    assert(mr_pw_stream_clock(&stream) == 0 && mr_pw_stream_quantum(&stream) == 0);
    assert(mr_pw_stream_native_format(&stream) == 0);
    assert(mr_pw_stream_capture_failures(&stream) == 1);
    assert(mr_pcm_ring_stats(&stream.ring).queued == 4);
    float result[8];
    memset(result, 0xff, sizeof(result));
    assert(mr_pw_stream_read(&stream, result, 4) == 0);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.0f);
    assert(mr_pw_stream_write(&stream, samples, 4) == 0);
    assert(mr_pw_stream_trigger_output(&stream) == -EINVAL);
    stream_process_buffer(&stream);
    assert(returned_buffers == before + 1);
    assert(mr_pcm_ring_stats(&stream.ring).queued == 4);
    mr_pcm_ring_clear(&stream.ring);

    /* Another acquisition keeps processing; the fault has no global state. */
    return_status = 0;
    test_output(MR_PCM_FORMAT_FLOAT32);
}

static void test_capture_clock_replacement_fences_old_pcm(void) {
    MRPWStream stream = { .channels = 2, .sample_rate = 48000 };
    initialize_transfer_observations(&stream);
    atomic_init(&stream.state, PW_STREAM_STATE_STREAMING);
    struct spa_io_position position = {0};
    position.clock.rate = SPA_FRACTION(1, 48000);
    position.clock.id = 1;
    position.clock.duration = 128;
    atomic_init(&stream.position, &position);
    atomic_init(&stream.clock_snapshot, 0);
    atomic_init(&stream.quantum_snapshot, 0);
    atomic_init(&stream.format_snapshot, UINT64_C(48000) << 32 | UINT64_C(1) << 16 | 2);
    atomic_init(&stream.format_valid, true);
    assert(mr_pcm_ring_init(&stream.ring, 2, 16));
    float samples[8];
    for (unsigned i = 0; i < 8; ++i) samples[i] = 0.125f;
    struct spa_chunk chunk = { .size = sizeof(samples), .stride = 2 * sizeof(float) };
    struct spa_data data = { .data = samples, .maxsize = sizeof(samples), .chunk = &chunk };
    struct spa_buffer spa_buffer = { .n_datas = 1, .datas = &data };
    struct pw_buffer buffer = { .buffer = &spa_buffer };
    next_buffer = &buffer;
    stream_process_buffer(&stream);
    uint64_t generation = mr_pw_stream_capture_failures(&stream);
    position.clock.duration = 256;
    position.clock.position += 512;
    stream_process_buffer(&stream);
    assert(mr_pw_stream_capture_failures(&stream) == generation);
    assert(mr_pcm_ring_stats(&stream.ring).queued == 8);

    position.clock.id = 2;
    for (unsigned i = 0; i < 8; ++i) samples[i] = 0.5f;
    stream_process_buffer(&stream);
    stream_process_buffer(&stream);
    assert(mr_pw_stream_capture_failures(&stream) == generation + 1);
    float result[8];
    assert(mr_pw_stream_read(&stream, result, 4) == 4);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.5f);
    assert(mr_pcm_ring_stats(&stream.ring).queued == 4);
    assert(mr_pw_stream_clock(&stream) == (UINT64_C(2) << 32 | 48000));
    mr_pcm_ring_clear(&stream.ring);
}

static void test_empty_capture_never_replays_recycled_pcm(void) {
    MRPWStream stream = { .channels = 2, .sample_rate = 48000 };
    initialize_transfer_observations(&stream);
    atomic_init(&stream.format_snapshot, UINT64_C(48000) << 32 | UINT64_C(1) << 16 | 2);
    atomic_init(&stream.format_valid, true);
    assert(mr_pcm_ring_init(&stream.ring, 2, 16));
    float recycled[8];
    for (unsigned i = 0; i < 8; ++i) recycled[i] = 0.875f;
    struct spa_chunk chunk = { .size = sizeof(recycled), .stride = 2 * sizeof(float),
                               .flags = SPA_CHUNK_FLAG_EMPTY };
    struct spa_data data = { .data = recycled, .maxsize = sizeof(recycled), .chunk = &chunk };
    float result[8];
    capture_buffer(&stream, &data, 128);
    capture_buffer(&stream, &data, 128);
    assert(mr_pw_stream_read(&stream, result, 4) == 4);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.0f);
    chunk.flags = 0;
    capture_buffer(&stream, &data, 128);
    capture_buffer(&stream, &data, 128);
    assert(mr_pw_stream_read(&stream, result, 4) == 4);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.0f);
    assert(mr_pw_stream_read(&stream, result, 4) == 4);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.875f);
    chunk.flags = SPA_CHUNK_FLAG_EMPTY;
    capture_buffer(&stream, &data, 128);
    capture_buffer(&stream, &data, 128);
    assert(mr_pw_stream_read(&stream, result, 4) == 4);
    for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.875f);
    for (unsigned block = 0; block < 2; ++block) {
        assert(mr_pw_stream_read(&stream, result, 4) == 4);
        for (unsigned i = 0; i < 8; ++i) assert(result[i] == 0.0f);
    }
    assert(mr_pcm_ring_stats(&stream.ring).overflows == 0);
    mr_pcm_ring_clear(&stream.ring);
}

int main(void) {
    for (uint32_t encoding = MR_PCM_FORMAT_FLOAT32; encoding <= MR_PCM_FORMAT_SIGNED32; ++encoding)
        test_output(encoding);
    test_return_failure_isolates_the_acquisition();
    test_capture_clock_replacement_fences_old_pcm();
    test_empty_capture_never_replays_recycled_pcm();
    return 0;
}
