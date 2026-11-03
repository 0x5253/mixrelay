#ifndef MR_CALLBACK_CLOCK_H
#define MR_CALLBACK_CLOCK_H
#include <CoreAudio/CoreAudioTypes.h>
#include <stdbool.h>
#include <stdint.h>

typedef struct MRCallbackClockSnapshot {
    double sample_time;
    uint64_t host_time;
    uint64_t discontinuities;
    uint32_t valid;
} MRCallbackClockSnapshot;

/* Caller-owned Rust storage. Initialize before registering IO; never copy,
 * inspect or clear these bytes while live. Destroy only after callbacks and
 * snapshot readers have drained. */
typedef struct MRCallbackClock { uint64_t storage[8]; } MRCallbackClock;
_Static_assert(sizeof(MRCallbackClock) == 64 && _Alignof(MRCallbackClock) == 8, "clock storage ABI");
_Static_assert(sizeof(MRCallbackClockSnapshot) == 32 && _Alignof(MRCallbackClockSnapshot) == 8, "clock snapshot ABI");

void mr_callback_clock_init(MRCallbackClock *clock);
/* Reset after the IOProc drains; preserve publication and cumulative errors. */
void mr_callback_clock_reset(MRCallbackClock *clock);
void mr_callback_clock_record_values(MRCallbackClock *clock, double sample,
    uint64_t host, bool timestamp_valid, uint64_t frames);
bool mr_callback_clock_snapshot(const MRCallbackClock *clock, MRCallbackClockSnapshot *result);
uint64_t mr_callback_clock_discontinuities(const MRCallbackClock *clock);

/* CoreAudio ABI projection only. Validity/continuity and publication live in Rust. */
static inline void mr_callback_clock_record(MRCallbackClock *clock,
    const AudioTimeStamp *time, uint64_t frames) {
    const AudioTimeStampFlags required = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    mr_callback_clock_record_values(clock, time ? time->mSampleTime : 0,
        time ? time->mHostTime : 0, time && (time->mFlags & required) == required, frames);
}
#endif
