#include "MRPcmRing.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static uint64_t now(void) {
    struct timespec time;
    assert(clock_gettime(CLOCK_MONOTONIC, &time) == 0);
    return (uint64_t)time.tv_sec * 1000000000 + (uint64_t)time.tv_nsec;
}
static int compare(const void *a, const void *b) {
    uint64_t left = *(const uint64_t *)a, right = *(const uint64_t *)b;
    return (left > right) - (left < right);
}
static void measure(uint32_t channels, uint32_t frames, bool prefill) {
    float input[256 * 64], output[256 * 64];
    for (uint32_t i = 0; i < frames * channels; ++i) input[i] = (float)i;
    const unsigned iterations = 20000;
    uint64_t times[7];
    for (unsigned run = 0; run < 8; ++run) {
        MRPcmRing ring;
        assert(mr_pcm_ring_init(&ring, channels, 16384));
        assert(mr_pcm_ring_write(&ring, input, frames) == frames);
        uint64_t start = now();
        for (unsigned i = 0; i < iterations; ++i) {
            assert(mr_pcm_ring_write(&ring, input, frames) == frames);
            assert((prefill ? mr_pcm_ring_read(&ring, output, frames)
                           : mr_pcm_ring_read_available(&ring, output, frames)) == frames);
        }
        uint64_t elapsed = now() - start;
        assert(memcmp(input, output, (size_t)frames * channels * sizeof(float)) == 0);
        mr_pcm_ring_clear(&ring);
        if (run) times[run - 1] = elapsed;
    }
    qsort(times, 7, sizeof(*times), compare);
    printf("channels=%u frames=%u prefill=%u ns_per_pair_min=%.3f median=%.3f max=%.3f\n",
        channels, frames, prefill, (double)times[0] / iterations,
        (double)times[3] / iterations, (double)times[6] / iterations);
}
int main(void) {
    const uint32_t widths[] = {2, 8, 64}, sizes[] = {32, 256};
    for (unsigned width = 0; width < 3; ++width)
        for (unsigned size = 0; size < 2; ++size) {
            measure(widths[width], sizes[size], false);
            measure(widths[width], sizes[size], true);
        }
}
