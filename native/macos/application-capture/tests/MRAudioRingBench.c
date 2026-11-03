/* Same native entry workload before/after the Rust ownership extraction. */
#include "MRAudioRing.h"
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static uint64_t nanoseconds(void) {
    struct timespec value;
    assert(clock_gettime(CLOCK_MONOTONIC_RAW, &value) == 0);
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}

static int compare(const void *left, const void *right) {
    const uint64_t a = *(const uint64_t *)left, b = *(const uint64_t *)right;
    return (a > b) - (a < b);
}

static void measure(uint32_t channels, size_t frames, bool planar) {
    const unsigned iterations = 20000;
    float input[64 * 256], output[64 * 256];
    const float *planes[64];
    for (size_t index = 0; index < channels * frames; ++index) input[index] = (float)index;
    for (uint32_t channel = 0; channel < channels; ++channel) planes[channel] = input + channel * frames;
    uint64_t times[7];
    for (unsigned run = 0; run < 8; ++run) {
        MRAudioRing *ring = mr_audio_ring_create(1024, channels);
        assert(ring);
        uint64_t start = nanoseconds();
        for (unsigned index = 0; index < iterations; ++index) {
            size_t written = planar ? mr_audio_ring_write_planar(ring, planes, frames)
                                    : mr_audio_ring_write_interleaved(ring, input, frames);
            assert(written == frames);
            assert(mr_audio_ring_read_interleaved(ring, output, frames) == frames);
        }
        uint64_t elapsed = nanoseconds() - start;
        assert(mr_audio_ring_available_frames(ring) == 0);
        assert(mr_audio_ring_dropped_frames(ring) == 0);
        for (size_t frame = 0; frame < frames; ++frame)
            for (uint32_t channel = 0; channel < channels; ++channel)
                assert(output[frame * channels + channel] ==
                    input[planar ? channel * frames + frame : frame * channels + channel]);
        mr_audio_ring_destroy(ring);
        if (run) times[run - 1] = elapsed;
    }
    qsort(times, 7, sizeof(*times), compare);
    printf("channels=%u frames=%zu layout=%s ns_per_pair_min=%.3f median=%.3f max=%.3f\n",
        channels, frames, planar ? "planar" : "interleaved",
        (double)times[0] / iterations, (double)times[3] / iterations, (double)times[6] / iterations);
}

int main(void) {
    const uint32_t channels[] = {2, 8, 64};
    const size_t frames[] = {32, 256};
    for (size_t channel = 0; channel < 3; ++channel)
        for (size_t frame = 0; frame < 2; ++frame) {
            measure(channels[channel], frames[frame], false);
            measure(channels[channel], frames[frame], true);
        }
}
