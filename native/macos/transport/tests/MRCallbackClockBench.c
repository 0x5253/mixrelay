/* Same callback workload before/after implementation changes. No devices opened. */
#include "../MRCallbackClock.h"
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <time.h>

static uint64_t nanoseconds(void) {
    struct timespec value;
    assert(clock_gettime(CLOCK_MONOTONIC_RAW, &value) == 0);
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}

int main(void) {
    const uint64_t count = 2000000;
    /* The first complete pass warms both implementations identically. */
    for (unsigned run = 0; run < 8; ++run) {
        MRCallbackClock clock;
        mr_callback_clock_init(&clock);
        AudioTimeStamp stamp = {.mFlags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid};
        uint64_t start = nanoseconds();
        for (uint64_t index = 0; index < count; ++index) {
            stamp.mSampleTime = (double)(index * 128);
            stamp.mHostTime = index + 1;
            mr_callback_clock_record(&clock, &stamp, 128);
        }
        uint64_t elapsed = nanoseconds() - start;
        MRCallbackClockSnapshot snapshot;
        assert(mr_callback_clock_snapshot(&clock, &snapshot));
        assert(snapshot.valid && snapshot.discontinuities == 0 && snapshot.host_time == count);
        if (run != 0) printf("run=%u iterations=%" PRIu64 " elapsed_ns=%" PRIu64 " ns_per_record=%.3f\n",
               run - 1, count, elapsed, (double)elapsed / (double)count);
    }
}
