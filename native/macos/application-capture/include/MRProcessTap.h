#ifndef MR_PROCESS_TAP_H
#define MR_PROCESS_TAP_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct MRProcessTapSession MRProcessTapSession;

typedef enum MRProcessTapErrorCode {
    MR_PROCESS_TAP_OK = 0,
    MR_PROCESS_TAP_INVALID_ARGUMENT = 1,
    MR_PROCESS_TAP_UNSUPPORTED_SYSTEM = 2,
    MR_PROCESS_TAP_APPLICATION_NOT_FOUND = 3,
    MR_PROCESS_TAP_CORE_AUDIO_FAILURE = 4,
    MR_PROCESS_TAP_UNSUPPORTED_FORMAT = 5,
    MR_PROCESS_TAP_OUT_OF_MEMORY = 6,
    MR_PROCESS_TAP_ALREADY_RUNNING = 7,
    MR_PROCESS_TAP_NOT_RUNNING = 8,
    MR_PROCESS_TAP_RESOURCE_BUSY = 9
} MRProcessTapErrorCode;

typedef struct MRProcessTapError {
    MRProcessTapErrorCode code;
    int32_t os_status;
} MRProcessTapError;

typedef struct MRProcessTapConfig {
    const char *bundle_id;
    bool mute_while_captured;
    size_t ring_capacity_frames;
    /* Required only for mute: exact MixRelay-owned output device UID. */
    const char *target_device_uid;
    bool system_audio;
    const char *const *excluded_bundle_ids;
    size_t excluded_bundle_count;
} MRProcessTapConfig;

typedef struct MRProcessTapStats {
    uint64_t callback_count;
    uint64_t captured_frames;
    uint64_t dropped_frames;
    size_t available_frames;
    uint32_t channels;
    double sample_rate;
    bool running;
    /* False during invalidation or a mixed format snapshot; retry without PCM/clock credit. */
    bool format_ready;
    uint64_t format_generation;
    uint64_t clock_discontinuities;
    double sample_time;
    uint64_t host_time;
    bool clock_valid;
} MRProcessTapStats;

_Static_assert(sizeof(MRProcessTapConfig) == 56, "tap config ABI");
_Static_assert(sizeof(MRProcessTapError) == 8, "tap error ABI");
_Static_assert(sizeof(MRProcessTapStats) == 96, "tap stats ABI");

typedef void (*MRProcessTapApplicationVisitor)(
    void *context,
    const char *bundle_id,
    const char *display_name,
    const char *icon_data_url,
    int32_t process_id
);

MRProcessTapError mr_process_tap_visit_applications(
    MRProcessTapApplicationVisitor visitor,
    void *context
);

MRProcessTapSession *mr_process_tap_create(
    const MRProcessTapConfig *config,
    MRProcessTapError *error
);
MRProcessTapError mr_process_tap_start(MRProcessTapSession *session);
MRProcessTapError mr_process_tap_stop(MRProcessTapSession *session);
/* Control-owner only, serialized with read/start/stop. Stats/peaks may run concurrently.
 * A failed refresh suspends delivery; incompatible geometry requires recreation. */
MRProcessTapError mr_process_tap_refresh_format(MRProcessTapSession *session);
/* Consumes ownership without waiting for Core Audio. A bounded native owner retries
 * failed unregister/removal; callback storage is never freed on an error. */
void mr_process_tap_destroy(MRProcessTapSession *session);

size_t mr_process_tap_read(
    MRProcessTapSession *session,
    float *destination,
    size_t maximum_frames
);
typedef struct MRProcessTapReadGeneration { uint64_t format; uint64_t clock_breaks; } MRProcessTapReadGeneration;
/* PCM acquisition coordinates in host nanoseconds; stale generations return no PCM. */
size_t mr_process_tap_read_timed(MRProcessTapSession *session, float *destination,
    uint64_t *timestamps, size_t maximum_frames, MRProcessTapReadGeneration expected);
size_t mr_process_tap_take_peaks(
    MRProcessTapSession *session,
    float *destination,
    size_t maximum_channels
);
MRProcessTapStats mr_process_tap_stats(const MRProcessTapSession *session);

#endif
