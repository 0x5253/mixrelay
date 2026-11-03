#ifndef MR_APPLICATION_CLOCK_H
#define MR_APPLICATION_CLOCK_H
#include "MRSharedAudioBuffer.h"
#include <stdatomic.h>
#include <stdint.h>
/* Current shared application clock ABI. Policy and initialization live in Rust;
 * C producers access only these atomics and the exported functions below. */
enum { MR_APPLICATION_CLOCK_VERSION = 7U, MR_APPLICATION_CLOCK_ALIGNMENT = 64U };
#define MR_APPLICATION_CLOCK_MAGIC UINT64_C(0x4d52415050434c4b)
#define MR_APPLICATION_CLOCK_FRAME_MASK UINT64_C(0x0000ffffffffffff)
typedef struct MRApplicationClock {
    uint64_t magic;
    uint32_t version;
    uint32_t header_bytes;
    _Atomic uint64_t format_word;
    _Atomic uint64_t acknowledged_format;
    _Atomic uint64_t consumer_position;
    _Atomic uint64_t consumer_ticks;
    _Atomic uint32_t consumer_running;
    _Atomic uint32_t native_sample_rate;
    _Atomic uint64_t consumer_sequence;
    _Atomic uint32_t producer_guard;
    _Atomic uint64_t native_frames;
    _Atomic uint32_t native_running;
    _Atomic uint32_t clock_master;
    _Atomic uint64_t producer_lease;
    _Atomic uint32_t target_backlog_frames;
    _Atomic uint32_t producer_burst_frames;
    _Atomic uint32_t converter_burst_frames;
    _Atomic uint32_t publication_drought_frames;
    _Atomic uint64_t starvation_episodes;
    _Atomic uint64_t starvation_frames;
    _Atomic uint64_t rebuffer_frames;
    _Atomic uint64_t overwrite_skips;
    _Atomic uint32_t consumer_burst_frames;
    _Atomic uint32_t insufficient_headroom;
    _Atomic uint64_t source_sample_faults;
    _Atomic int32_t consumer_phase;
    _Atomic uint32_t consumer_route;
    _Atomic uint64_t consumer_period;
} MRApplicationClock;
size_t mr_application_clock_required_bytes(uint32_t capacity, uint32_t channels);
MRApplicationClock *mr_application_clock_attach(void *memory, size_t size, uint32_t capacity, uint32_t channels);
MRApplicationClock *mr_application_clock_initialize(void *memory, size_t size, uint32_t capacity, uint32_t channels);
bool mr_application_clock_try_producer(MRApplicationClock *clock);
void mr_application_clock_release_producer(MRApplicationClock *clock);
uint64_t mr_application_clock_request_rate(MRApplicationClock *clock, uint32_t rate);
void mr_application_clock_publish_consumer(MRApplicationClock *clock, uint64_t epoch, uint64_t next_frame, uint32_t frames, bool running, uint32_t target_backlog_frames);
#endif
