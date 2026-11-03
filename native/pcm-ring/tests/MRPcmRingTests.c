#include "MRPcmRing.h"
#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>

static void quantum_test(uint32_t channels) {
    MRPcmRing r;
    assert(mr_pcm_ring_init(&r, channels, 8192));
    float in[1024 * 64], out[128 * 64];
    uint32_t sent = 0, received = 0;
    /* A 1024-frame native producer versus 128-frame DSP consumer. The former
     * FIFO bug discarded 896 frames on each burst. Verify every channel/sample. */
    for (uint32_t block = 0; block < 500; block++) {
        for (uint32_t f = 0; f < 1024; f++)
            for (uint32_t c = 0; c < channels; c++) in[f * channels + c] = (float)(sent + f + c);
        assert(mr_pcm_ring_write(&r, in, 1024) == 1024);
        sent += 1024;
        for (unsigned chunk = 0; chunk < 8; chunk++) {
            uint32_t frames = mr_pcm_ring_read(&r, out, 128);
            if (block == 0) { assert(frames == 0); continue; }
            assert(frames == 128);
            for (uint32_t f = 0; f < frames; f++)
                for (uint32_t c = 0; c < channels; c++) assert(out[f * channels + c] == (float)(received + f + c));
            received += frames;
        }
    }
    MRPcmRingStats stats = mr_pcm_ring_stats(&r);
    assert(stats.overflows == 0 && stats.starvations == 0);
    mr_pcm_ring_clear(&r);
}

static void bounds_test(void) {
    MRPcmRing r;
    assert(!mr_pcm_ring_init(&r, 0, 16));
    assert(!mr_pcm_ring_init(&r, 65, 16));
    assert(mr_pcm_ring_init(&r, 2, 16));
    float in[16], out[16];
    for (unsigned i = 0; i < 16; i++) in[i] = (float)i;
    assert(mr_pcm_ring_write(&r, in, 8) == 8);
    assert(mr_pcm_ring_write(&r, in, 8) == 8);
    assert(mr_pcm_ring_write(&r, in, 8) == 0);
    assert(mr_pcm_ring_read(&r, out, 8) == 8);
    assert(memcmp(in, out, sizeof(in)) == 0);
    assert(mr_pcm_ring_read(&r, out, 8) == 8);
    memset(out, 0x7f, sizeof(out));
    assert(mr_pcm_ring_read(&r, out, 8) == 0);
    for (unsigned i = 0; i < 16; i++) assert(out[i] == 0.0f);
    MRPcmRingStats stats = mr_pcm_ring_stats(&r);
    assert(stats.overflows == 1 && stats.starvations == 1);
    /* Recovery re-primes; no stale audio replay. */
    assert(mr_pcm_ring_write(&r, in, 8) == 8);
    assert(mr_pcm_ring_read(&r, out, 8) == 0);
    assert(mr_pcm_ring_write(&r, in, 8) == 8);
    assert(mr_pcm_ring_read(&r, out, 8) == 8);
    mr_pcm_ring_clear(&r);
}

static void *produce(void *userdata) {
    MRPcmRing *r = userdata;
    for (uint32_t i = 0; i < 100000; i++) {
        float frame[2] = {(float)i, -(float)i};
        while (mr_pcm_ring_write(r, frame, 1) == 0) sched_yield();
    }
    return NULL;
}
static void concurrency_test(void) {
    MRPcmRing r;
    assert(mr_pcm_ring_init(&r, 2, 1024));
    pthread_t producer;
    assert(pthread_create(&producer, NULL, produce, &r) == 0);
    /* Leave one frame queued to satisfy the startup reserve even at the tail. */
    for (uint32_t i = 0; i < 99999; i++) {
        float frame[2];
        while (mr_pcm_ring_read(&r, frame, 1) == 0) sched_yield();
        assert(frame[0] == (float)i && frame[1] == -(float)i);
    }
    assert(pthread_join(producer, NULL) == 0);
    mr_pcm_ring_clear(&r);
}
static void discard_test(void) {
    MRPcmRing ring;
    assert(mr_pcm_ring_init(&ring, 2, 64));
    float stale[16], fresh[16], output[8];
    for (unsigned n = 0; n < 16; n++) { stale[n] = 1.0f; fresh[n] = 2.0f; }
    for (unsigned cycle = 0; cycle < 1000; cycle++) {
        mr_pcm_ring_discard(&ring);
        assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        assert(mr_pcm_ring_read(&ring, output, 4) == 4);
        mr_pcm_ring_discard(&ring);
        assert(mr_pcm_ring_read(&ring, output, 4) == 0);
        for (unsigned n = 0; n < 8; n++) assert(output[n] == 0.0f);
        assert(mr_pcm_ring_write(&ring, fresh, 8) == 8);
        assert(mr_pcm_ring_write(&ring, fresh, 8) == 8);
        assert(mr_pcm_ring_read(&ring, output, 4) == 4);
        for (unsigned n = 0; n < 8; n++) assert(output[n] == 2.0f);
    }
    mr_pcm_ring_clear(&ring);
}
static void restart_test(void) {
    MRPcmRing ring;
    assert(mr_pcm_ring_init(&ring, 2, 32));
    float stale[16], fresh[16], output[16];
    for (unsigned n = 0; n < 16; n++) { stale[n] = 1.0f; fresh[n] = 2.0f; }
    for (unsigned cycle = 0; cycle < 1000; cycle++) {
        mr_pcm_ring_discard(&ring);
        assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        mr_pcm_ring_restart(&ring); /* pause */
        mr_pcm_ring_restart(&ring); /* resume before any consumer callback */
        assert(mr_pcm_ring_read(&ring, output, 8) == 0);
        for (unsigned n = 0; n < 16; n++) assert(output[n] == 0.0f);
        assert(mr_pcm_ring_write(&ring, fresh, 8) == 8);
        assert(mr_pcm_ring_read(&ring, output, 8) == 0); /* fresh epoch prefill */
        assert(mr_pcm_ring_write(&ring, fresh, 8) == 8);
        assert(mr_pcm_ring_read(&ring, output, 8) == 8);
        for (unsigned n = 0; n < 16; n++) assert(output[n] == 2.0f);
        mr_pcm_ring_restart(&ring);
        assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        mr_pcm_ring_discard(&ring); /* newer than unobserved epoch acknowledgement */
        uint64_t discarded = mr_pcm_ring_stats(&ring).read;
        assert(mr_pcm_ring_read(&ring, output, 8) == 0);
        assert(mr_pcm_ring_stats(&ring).read == discarded);
        for (unsigned n = 0; n < 4; n++) assert(mr_pcm_ring_write(&ring, stale, 8) == 8);
        mr_pcm_ring_restart(&ring);
        /* A full stale ring must not deadlock acknowledgement: even a rejected
         * write publishes the epoch boundary, which only the reader releases. */
        assert(mr_pcm_ring_write(&ring, fresh, 8) == 0);
        assert(mr_pcm_ring_read(&ring, output, 8) == 0);
        MRPcmRingStats stats = mr_pcm_ring_stats(&ring);
        assert(stats.read == stats.written);
    }
    mr_pcm_ring_clear(&ring);
}
static void immediate_read_retains_epoch_safety(void) {
    MRPcmRing ring;
    assert(mr_pcm_ring_init(&ring, 1, 64));
    float input[8] = {1,2,3,4,5,6,7,8}, out[8];
    assert(mr_pcm_ring_write(&ring, input, 8) == 8);
    assert(mr_pcm_ring_read_available(&ring, out, 8) == 8);
    assert(!memcmp(input, out, sizeof(input)));
    assert(mr_pcm_ring_write(&ring, input, 8) == 8);
    mr_pcm_ring_restart(&ring);
    assert(mr_pcm_ring_read_available(&ring, out, 8) == 0);
    for (unsigned i = 0; i < 8; ++i) assert(out[i] == 0);
    input[0] = 9;
    assert(mr_pcm_ring_write(&ring, input, 8) == 8);
    assert(mr_pcm_ring_read_available(&ring, out, 8) == 8);
    assert(!memcmp(input, out, sizeof(input)));
    mr_pcm_ring_clear(&ring);
}
int main(void) {
    immediate_read_retains_epoch_safety();
    const uint32_t channels[] = {1, 2, 8, 16, 32, 64};
    for (unsigned i = 0; i < sizeof(channels) / sizeof(channels[0]); i++) quantum_test(channels[i]);
    bounds_test();
    concurrency_test();
    discard_test();
    restart_test();
    puts("PASS: native PCM quantum, channel, wrap, overflow, silence, recovery and concurrent ownership");
}
