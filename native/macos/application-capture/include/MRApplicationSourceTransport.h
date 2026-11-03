#ifndef MR_APPLICATION_SOURCE_TRANSPORT_H
#define MR_APPLICATION_SOURCE_TRANSPORT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MRApplicationSourceTransport MRApplicationSourceTransport;
typedef struct MREngineSourceAttachment MREngineSourceAttachment;

typedef enum MRApplicationSourceTransportErrorCode {
    MR_APPLICATION_SOURCE_TRANSPORT_OK = 0,
    MR_APPLICATION_SOURCE_TRANSPORT_INVALID_ARGUMENT = 1,
    MR_APPLICATION_SOURCE_TRANSPORT_ENGINE_UNAVAILABLE = 2,
    MR_APPLICATION_SOURCE_TRANSPORT_INVALID_ATTACHMENT = 3,
    MR_APPLICATION_SOURCE_TRANSPORT_MAPPING_FAILED = 4,
} MRApplicationSourceTransportErrorCode;

typedef struct MRApplicationSourceTransportError {
    MRApplicationSourceTransportErrorCode code;
    int32_t system_error;
} MRApplicationSourceTransportError;

/* Control-worker snapshot; never exposes pointers into another process. */
typedef struct MRApplicationSourceClockSnapshot {
    uint64_t format_word;
    uint64_t epoch;
    uint64_t produced_frames;
    uint64_t consumed_frames;
    uint64_t consumer_ticks;
    uint32_t consumer_running;
    uint32_t clock_master;
    uint32_t target_backlog_frames;
    uint32_t consumer_burst_frames;
    uint64_t starvation_episodes;
    uint64_t starvation_frames;
    uint64_t rebuffer_frames;
    uint64_t overwrite_skips;
    uint32_t insufficient_headroom;

} MRApplicationSourceClockSnapshot;

void mr_application_source_transport_publish_measurements(
    MRApplicationSourceTransport *transport, uint32_t producer_burst_frames,
    uint32_t converter_burst_frames, uint32_t publication_drought_frames);

bool mr_application_source_transport_clock(
    MRApplicationSourceTransport *transport, MRApplicationSourceClockSnapshot *snapshot);
bool mr_application_source_transport_reset(
    MRApplicationSourceTransport *transport, uint64_t format_word, uint32_t native_sample_rate);
void mr_application_source_transport_publish_native(
    MRApplicationSourceTransport *transport, uint64_t native_frames, bool running);
/* End activity without rewinding PCM already committed in this epoch. */
void mr_application_source_transport_stop_native(MRApplicationSourceTransport *transport);
bool mr_application_source_transport_record_source_faults(
    MRApplicationSourceTransport *transport, uint64_t epoch, uint64_t count);

MRApplicationSourceTransport *mr_application_source_transport_connect(
    const char *source_id,
    uint32_t capacity_frames,
    uint32_t channels,
    MRApplicationSourceTransportError *error);

/* Takes and clears the attachment, closing its descriptor on every failure.
 * Successful handles are owned by exactly one pump; destroy only after it stops. */
MRApplicationSourceTransport *mr_application_source_transport_attach(
    MREngineSourceAttachment *attachment,
    uint32_t capacity_frames,
    uint32_t channels,
    MRApplicationSourceTransportError *error);

bool mr_application_source_transport_write(
    MRApplicationSourceTransport *transport,
    const float *interleaved_samples,
    uint32_t frame_count);
/* Commit timed PCM before publishing its corresponding native frame credit. */
bool mr_application_source_transport_write_clocked(MRApplicationSourceTransport *transport,
    const float *samples, const uint64_t *times, uint32_t frames);
bool mr_application_source_transport_write_timed(MRApplicationSourceTransport *transport,
    const float *interleaved_samples, const uint64_t *timestamps, uint32_t frame_count);

void mr_application_source_transport_destroy(
    MRApplicationSourceTransport *transport);

#ifdef __cplusplus
}
#endif

#endif
