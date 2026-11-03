#include "../MRCallbackClock.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <math.h>
#include <stdatomic.h>

static AudioTimeStamp stamp(double sample, uint64_t host) {
    return (AudioTimeStamp){.mSampleTime = sample, .mHostTime = host,
        .mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid};
}
static void continuity(void) {
    MRCallbackClock clock;
    mr_callback_clock_init(&clock);
    MRCallbackClockSnapshot read = {0};
    assert(mr_callback_clock_snapshot(&clock, &read) && !read.valid);
    AudioTimeStamp time = stamp(-128, 1000);
    mr_callback_clock_record(&clock, &time, 128);
    time = stamp(0, 2000);
    mr_callback_clock_record(&clock, &time, 256);
    assert(mr_callback_clock_snapshot(&clock, &read));
    assert(read.valid && read.sample_time == 0 && read.host_time == 2000 && !read.discontinuities);
    time = stamp(512, 3000); /* a gap, not ordinary callback arrival jitter */
    mr_callback_clock_record(&clock, &time, 64);
    assert(mr_callback_clock_discontinuities(&clock) == 1);
    time = stamp(576, 2999); /* invalid backwards host clock */
    mr_callback_clock_record(&clock, &time, 128);
    assert(mr_callback_clock_discontinuities(&clock) == 2);
    time = stamp(704, 999999); /* host interval alone must not imply drift */
    mr_callback_clock_record(&clock, &time, 128);
    assert(mr_callback_clock_discontinuities(&clock) == 2);
    time.mFlags = kAudioTimeStampSampleTimeValid;
    mr_callback_clock_record(&clock, &time, 128);
    assert(mr_callback_clock_discontinuities(&clock) == 3);
    mr_callback_clock_record(&clock, NULL, 128);
    assert(mr_callback_clock_discontinuities(&clock) == 3);
    time = stamp(NAN, 9999999);
    mr_callback_clock_record(&clock, &time, 128);
    assert(mr_callback_clock_snapshot(&clock, &read) && !read.valid);
    time = stamp(1, 1); /* new timeline after invalidity */
    mr_callback_clock_record(&clock, &time, 128);
    assert(mr_callback_clock_snapshot(&clock, &read) && read.valid && read.discontinuities == 3);
    mr_callback_clock_reset(&clock);
    assert(mr_callback_clock_discontinuities(&clock) == 3);
}

typedef struct Fixture { MRCallbackClock clock; _Atomic bool done; } Fixture;
static void *writer(void *context) {
    Fixture *fixture = context;
    for (uint64_t i = 1; i <= 100000; ++i) {
        AudioTimeStamp time = stamp((double)i, i * 7);
        mr_callback_clock_record(&fixture->clock, &time, 1);
    }
    atomic_store(&fixture->done, true);
    return NULL;
}
static void concurrent_snapshot(void) {
    Fixture fixture;
    mr_callback_clock_init(&fixture.clock);
    atomic_init(&fixture.done, false);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, writer, &fixture) == 0);
    while (!atomic_load(&fixture.done)) {
        MRCallbackClockSnapshot read;
        if (mr_callback_clock_snapshot(&fixture.clock, &read) && read.valid) {
            assert(read.host_time == (uint64_t)read.sample_time * 7);
            assert(read.discontinuities == 0);
        }
    }
    assert(pthread_join(thread, NULL) == 0);
}
int main(void) {
    continuity();
    concurrent_snapshot();
    puts("PASS callback timestamps: validity, gaps, backwards clocks, 100000 concurrent publications");
}
