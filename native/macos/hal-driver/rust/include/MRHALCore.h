#ifndef MR_HAL_CORE_H
#define MR_HAL_CORE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* All ownership and registry policy lives in Rust. The native runtime callbacks
 * below only adapt the existing C11-atomic transport/clock layout and Apple ABI.
 * Every callback is synchronous. Native callbacks must never call the registry.
 * Host callbacks may reenter it: they are always invoked after releasing locks. */
typedef struct MRHALCore MRHALCore;
typedef struct MRHALCoreGeometry {
    uint8_t endpoint_id[16];
    uint32_t render_channels;
    uint32_t capture_channels;
} MRHALCoreGeometry;
typedef struct MRHALCoreTimestamp {
    uint64_t sample_time, host_time, seed;
} MRHALCoreTimestamp;
typedef struct MRHALCoreSnapshot {
    uint32_t device_id, input_stream_id, output_stream_id;
    uint32_t sample_rate, render_channels, capture_channels, client_count;
    bool alive, running, input_active, output_active;
    char name[128];
    char uid[80];
    uint32_t capture_format, render_format;
} MRHALCoreSnapshot;
typedef struct MRHALCoreIoOutcome {
    bool accepted;
    uint32_t available;
} MRHALCoreIoOutcome;
typedef struct MRHALCoreNativeOps {
    bool (*inspect)(int, size_t, MRHALCoreGeometry *);
    void *(*create)(const MRHALCoreGeometry *, uint32_t, int, size_t);
    void (*destroy)(void *);
    /* Control, outside the IO freeze: maps and validates an endpoint mapping. */
    void *(*prepare)(const MRHALCoreGeometry *, int, size_t);
    void (*discard)(void *);
    /* Control, after a freeze ends: releases storage the frozen operation detached. */
    void (*release)(void *);
    /* Frozen swap to a prepared mapping at a rate; always consumes it. */
    bool (*replace)(void *, void *, uint32_t);
    /* Frozen first start and rate change; each rearms client queues in place. */
    bool (*start)(void *, uint64_t);
    void (*stop)(void *);
    void (*remaining_client)(void *);
    bool (*set_rate)(void *, uint32_t);
    bool (*timestamp)(void *, uint64_t, MRHALCoreTimestamp *);
    /* Control, outside the IO freeze: a queue for a client that needs one, or NULL.
       False when it could not be mapped within the endpoint's PCM budget. */
    bool (*prepare_client)(void *, uint32_t, void **);
    /* Frozen registration, IO-start install or removal (NULL change). */
    bool (*client)(void *, uint32_t, uint32_t, void *);
    MRHALCoreIoOutcome (*io)(void *, bool, uint32_t, uint64_t, float *, uint32_t);
    bool (*requires_replacement)(void *);
    bool (*same_transport)(void *, int, size_t);
} MRHALCoreNativeOps;
enum {
    MR_HAL_CORE_OK = 0,
    MR_HAL_CORE_INVALID = -1,
    MR_HAL_CORE_NOT_FOUND = -2,
    MR_HAL_CORE_BUSY = -3,
    MR_HAL_CORE_NATIVE_ERROR = -4,
    MR_HAL_CORE_CAPACITY = -5,
    MR_HAL_CORE_CHANGE_LIST = 1,
    MR_HAL_CORE_CHANGE_NAME = 2,
    MR_HAL_CORE_CHANGE_FORMAT = 3,
    MR_HAL_CORE_CHANGE_ALIVE = 4,
    MR_HAL_CORE_CHANGE_RUNNING = 5,
    MR_HAL_CORE_CHANGE_STREAM_ACTIVE = 6,
    MR_HAL_CORE_CHANGE_OUTPUT_VOLUME = 7,
    MR_HAL_CORE_CHANGE_OUTPUT_MUTE = 8,
    MR_HAL_CORE_CHANGE_TRANSPORT = 9,
    MR_HAL_CORE_CHANGE_INPUT_LATENCY = 10,
    MR_HAL_CORE_MAX_DEVICES = 64,
};
#define MR_HAL_CORE_CONFIG_ACTION UINT64_C(0x4d526368)
typedef struct MRHALCoreHostOps {
    void *context;
    void (*changed)(void *, uint32_t, uint32_t);
    bool (*request_configuration)(void *, uint32_t, uint64_t, uint64_t);
} MRHALCoreHostOps;

/* Balanced, nestable scope; only IO-cycle/timestamp callbacks enter it. */
void mr_hal_core_enter_audio_thread(void);
void mr_hal_core_leave_audio_thread(void);
MRHALCore *mr_hal_core_create(const MRHALCoreNativeOps *, const MRHALCoreHostOps *);
MRHALCore *mr_hal_core_create_native(const MRHALCoreHostOps *);
/* Shutdown only: caller must first stop XPC/timers and all host callbacks. */
void mr_hal_core_destroy(MRHALCore *);
/* Consumes fd on EVERY path, including invalid arguments. The mapped transport
 * endpoint identity is verified before any device is published or changed. */
int32_t mr_hal_core_upsert(MRHALCore *, const uint8_t endpoint_id[16],
                          const char *name, uint32_t nominal_rate,
                          int fd, size_t mapped_size, uint32_t *device_id);
int32_t mr_hal_core_remove(MRHALCore *, const uint8_t endpoint_id[16]);
/* Retries failed host configuration requests and reclaims drained retired slots.
 * Call on a serial control queue, never from a real-time callback. */
void mr_hal_core_tick(MRHALCore *);
size_t mr_hal_core_devices(MRHALCore *, uint32_t *, size_t capacity);
uint32_t mr_hal_core_lookup_uid(MRHALCore *, const char *);
bool mr_hal_core_snapshot(MRHALCore *, uint32_t object_id, MRHALCoreSnapshot *);
/* Bounded pinned lookup; no control mutex, allocation, or metadata copying. */
bool mr_hal_core_device_alive(const MRHALCore *, uint32_t device_id);
int32_t mr_hal_core_client(MRHALCore *, uint32_t device_id, uint32_t client_id, bool add);
/* Copies a nullable, callback-borrowed UTF-8 bundle ID (at most 127 bytes).
 * Control callbacks only; never invoke client registration from IO. */
int32_t mr_hal_core_client_identity(MRHALCore *, uint32_t device_id, uint32_t client_id, int32_t pid, const char *bundle_id, bool add);
void mr_hal_core_render_client(const MRHALCore *, uint32_t device_id, uint32_t client_id);
int32_t mr_hal_core_start(MRHALCore *, uint32_t device_id, uint32_t client_id, uint64_t host_time);
int32_t mr_hal_core_stop(MRHALCore *, uint32_t device_id, uint32_t client_id);
int32_t mr_hal_core_set_rate(MRHALCore *, uint32_t device_id, uint32_t sample_rate);
int32_t mr_hal_core_set_stream_active(MRHALCore *, uint32_t stream_id, bool active);
int32_t mr_hal_core_perform(MRHALCore *, uint32_t device_id, uint64_t action, uint64_t generation);
int32_t mr_hal_core_abort(MRHALCore *, uint32_t device_id, uint64_t action, uint64_t generation);
/* These two entry points do not lock, allocate, wait, or free. A removed device
 * is silenced; a stale object ID never resolves to a replacement device. */
bool mr_hal_core_timestamp(MRHALCore *, uint32_t device_id, uint64_t now, MRHALCoreTimestamp *);
/* Exclusive writable operation main buffer in the current native stream format,
 * including WriteMix. Never pass shared transport storage. Float32 is aligned;
 * signed16/24/32 are little-endian packed bytes. Internal transport stays Float32. */
bool mr_hal_core_io(MRHALCore *, uint32_t device_id, uint32_t stream_id,
                   bool render, uint32_t client_id, uint64_t frame, void *, uint32_t frame_count);
bool mr_hal_core_io_sample_time(MRHALCore *, uint32_t device_id, uint32_t stream_id,
                   bool render, uint32_t client_id, double sample_time, void *, uint32_t frame_count);
typedef struct MRHALCorePropertyAddress { uint32_t selector, scope, element; } MRHALCorePropertyAddress;
typedef struct MRHALCoreNotification { uint32_t object_id; MRHALCorePropertyAddress address; } MRHALCoreNotification;
bool mr_hal_core_has_property(MRHALCore *, uint32_t, const MRHALCorePropertyAddress *);
int32_t mr_hal_core_property_size(MRHALCore *, uint32_t, const MRHALCorePropertyAddress *, uint32_t *);
int32_t mr_hal_core_property_get(MRHALCore *, uint32_t, const MRHALCorePropertyAddress *, uint32_t, const void *, uint32_t, uint32_t *, void *, int32_t requester_pid);
int32_t mr_hal_core_property_settable(MRHALCore *, uint32_t, const MRHALCorePropertyAddress *, bool *);
int32_t mr_hal_core_property_set(MRHALCore *, uint32_t, const MRHALCorePropertyAddress *, uint32_t, const void *);
size_t mr_hal_core_notifications(uint32_t, uint32_t, MRHALCoreNotification *, size_t);
/* Nonzero, increasing local XPC connection identity; never a peer counter. */
bool mr_hal_core_control_connected(MRHALCore *, uint64_t authority);
bool mr_hal_core_control_message(MRHALCore *, uint64_t authority, void *xpc_message);
bool mr_hal_core_replacement_request(MRHALCore *, uint64_t authority, uint8_t endpoint_id[16], uint64_t *generation);

#endif
