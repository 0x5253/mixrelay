#include "MRDeviceIO.h"
#include <CoreAudio/CoreAudio.h>
#include <stdlib.h>
#include <assert.h>
#include <stdio.h>
#include <time.h>

static uint64_t nanos(void) {
    struct timespec time;
    assert(clock_gettime(CLOCK_MONOTONIC, &time) == 0);
    return (uint64_t)time.tv_sec * 1000000000 + (uint64_t)time.tv_nsec;
}
static int compare(const void *a, const void *b) {
    uint64_t left = *(const uint64_t *)a, right = *(const uint64_t *)b;
    return (left > right) - (left < right);
}
static void measure(uint32_t channels, uint32_t frames, bool planar, bool input) {
    struct { UInt32 count; AudioBuffer buffers[64]; } list = {0};
    float native[64 * 256], graph[64 * 256];
    list.count = planar ? channels : 1;
    for (uint32_t b = 0; b < list.count; ++b)
        list.buffers[b] = (AudioBuffer){planar ? 1 : channels,
            frames * (planar ? 1 : channels) * sizeof(float),
            native + (planar ? b * frames : 0)};
    for (uint32_t f = 0; f < frames; ++f) for (uint32_t c = 0; c < channels; ++c) {
        native[planar ? c * frames + f : f * channels + c] = (float)(f * channels + c);
        graph[f * channels + c] = (float)(f * channels + c);
    }
    MRDeviceIO *io = mr_device_io_create("benchmark", input, channels, 48000);
    assert(io);
    uint64_t results[7];
    const unsigned iterations = 20000;
    for (unsigned pass = 0; pass < 8; ++pass) {
        uint64_t start = nanos();
        for (unsigned i = 0; i < iterations; ++i) {
            if (!input) assert(mr_device_io_bridge_write(io, graph, frames) == frames);
            mr_device_io_process(0, NULL, input ? (AudioBufferList *)&list : NULL, NULL,
                input ? NULL : (AudioBufferList *)&list, NULL, io);
            if (input) assert(mr_device_io_bridge_read(io, graph, frames) == frames);
        }
        uint64_t elapsed = nanos() - start;
        for (uint32_t f = 0; f < frames; ++f) for (uint32_t c = 0; c < channels; ++c)
            assert(native[planar ? c * frames + f : f * channels + c] == graph[f * channels + c]);
        if (pass) results[pass - 1] = elapsed;
    }
    assert(mr_device_io_destroy(io));
    qsort(results, 7, sizeof(*results), compare);
    printf("channels=%u frames=%u planar=%u input=%u ns_pair=%.3f min=%.3f max=%.3f\n",
        channels, frames, planar, input, (double)results[3] / iterations,
        (double)results[0] / iterations, (double)results[6] / iterations);
}
int main(void) {
    const uint32_t widths[] = {2, 8, 64}, frames[] = {32, 256};
    for (unsigned c = 0; c < 3; ++c) for (unsigned f = 0; f < 2; ++f)
        for (unsigned planar = 0; planar < 2; ++planar) for (unsigned input = 0; input < 2; ++input)
            measure(widths[c], frames[f], planar, input);
}
