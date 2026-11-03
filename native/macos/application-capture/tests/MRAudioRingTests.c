#include "MRAudioRing.h"

#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <sched.h>

static void test_rejects_invalid_configuration(void) {
    assert(mr_audio_ring_create(0, 2) == NULL);
    assert(mr_audio_ring_create(3, 2) == NULL);
    assert(mr_audio_ring_create(8, 0) == NULL);
    assert(mr_audio_ring_create(8, 65) == NULL);
    assert(mr_audio_ring_create((size_t)MR_AUDIO_RING_MAX_CAPACITY_FRAMES * 2U, 2) == NULL);
    /* Both allocation failures and partial-owner cleanup are tested at the Rust
     * constructor boundary, without a test-only native allocator ABI. */
    assert(mr_audio_ring_capacity_frames(NULL) == 0);
    assert(mr_audio_ring_available_frames(NULL) == 0);
    assert(mr_audio_ring_dropped_frames(NULL) == 0);
    mr_audio_ring_destroy(NULL);
}

static void test_interleaved_wraparound_and_bounded_overflow(void) {
    MRAudioRing *ring = mr_audio_ring_create(4, 2);
    assert(ring != NULL);
    assert(mr_audio_ring_capacity_frames(ring) == 4);

    const float first[] = {1, 2, 3, 4, 5, 6};
    assert(mr_audio_ring_write_interleaved(ring, first, 3) == 3);
    assert(mr_audio_ring_available_frames(ring) == 3);

    float output[8] = {0};
    assert(mr_audio_ring_read_interleaved(ring, output, 2) == 2);
    assert(output[0] == 1 && output[1] == 2 && output[2] == 3 && output[3] == 4);

    const float second[] = {7, 8, 9, 10, 11, 12};
    assert(mr_audio_ring_write_interleaved(ring, second, 3) == 3);
    assert(mr_audio_ring_write_interleaved(ring, second, 1) == 0);
    assert(mr_audio_ring_dropped_frames(ring) == 1);

    assert(mr_audio_ring_read_interleaved(ring, output, 4) == 4);
    const float expected[] = {5, 6, 7, 8, 9, 10, 11, 12};
    for (size_t index = 0; index < 8; ++index) {
        assert(output[index] == expected[index]);
    }
    assert(mr_audio_ring_available_frames(ring) == 0);
    mr_audio_ring_destroy(ring);
}

static void test_planar_conversion_and_invalid_calls(void) {
    MRAudioRing *ring = mr_audio_ring_create(4, 2);
    const float left[] = {1, 2};
    const float right[] = {3, 4};
    const float *channels[] = {left, right};
    assert(mr_audio_ring_write_planar(ring, channels, 2) == 2);

    float output[4] = {0};
    assert(mr_audio_ring_read_interleaved(ring, output, 2) == 2);
    assert(output[0] == 1 && output[1] == 3 && output[2] == 2 && output[3] == 4);

    const float *invalid_channels[] = {left, NULL};
    assert(mr_audio_ring_write_planar(ring, invalid_channels, 1) == 0);
    assert(mr_audio_ring_dropped_frames(ring) == 1);
    assert(mr_audio_ring_write_planar(ring, NULL, 1) == 0);
    assert(mr_audio_ring_write_planar(ring, channels, 0) == 0);
    assert(mr_audio_ring_write_interleaved(ring, NULL, 1) == 0);
    assert(mr_audio_ring_write_interleaved(NULL, left, 1) == 0);
    assert(mr_audio_ring_read_interleaved(ring, NULL, 1) == 0);
    assert(mr_audio_ring_read_interleaved(ring, output, 0) == 0);
    assert(mr_audio_ring_read_interleaved(NULL, output, 1) == 0);
    assert(mr_audio_ring_write_interleaved(ring, left, 0) == 0);
    mr_audio_ring_destroy(ring);
}

typedef struct ConcurrentFixture { MRAudioRing *ring; uint32_t channels; } ConcurrentFixture;

static void *produce(void *context) {
    ConcurrentFixture *fixture = context;
    float input[64 * 17];
    for (unsigned block = 0; block < 10000; ++block) {
        for (unsigned frame = 0; frame < 17; ++frame)
            for (uint32_t channel = 0; channel < fixture->channels; ++channel)
                input[frame * fixture->channels + channel] = (float)((block * 17 + frame) * 64 + channel);
        while (mr_audio_ring_write_interleaved(fixture->ring, input, 17) == 0) sched_yield();
    }
    return NULL;
}

static void test_concurrent_pcm_is_ordered_and_channel_exact(void) {
    const uint32_t widths[] = {1, 2, 8, 64};
    for (size_t width = 0; width < 4; ++width) {
        ConcurrentFixture fixture = {mr_audio_ring_create(64, widths[width]), widths[width]};
        assert(fixture.ring);
        pthread_t writer;
        assert(pthread_create(&writer, NULL, produce, &fixture) == 0);
        float output[64 * 13];
        unsigned consumed = 0;
        while (consumed < 170000) {
            size_t frames = mr_audio_ring_read_interleaved(fixture.ring, output, 13);
            assert(mr_audio_ring_available_frames(fixture.ring) <= 64);
            for (size_t frame = 0; frame < frames; ++frame)
                for (uint32_t channel = 0; channel < fixture.channels; ++channel)
                    assert(output[frame * fixture.channels + channel] ==
                        (float)((consumed + frame) * 64 + channel));
            consumed += (unsigned)frames;
            if (!frames) sched_yield();
        }
        assert(pthread_join(writer, NULL) == 0);
        assert(mr_audio_ring_available_frames(fixture.ring) == 0);
        mr_audio_ring_destroy(fixture.ring);
    }
}

int main(void) {
    test_rejects_invalid_configuration();
    test_interleaved_wraparound_and_bounded_overflow();
    test_planar_conversion_and_invalid_calls();
    test_concurrent_pcm_is_ordered_and_channel_exact();
    puts("MRAudioRingTests passed");
    return 0;
}
