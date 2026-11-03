#ifndef MR_DEVICE_IO_H
#define MR_DEVICE_IO_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "MRAggregateGroup.h"
typedef struct MRDeviceIO MRDeviceIO;
/* Set once before connecting. Rust owns the complete callback context. */
typedef struct MRIOTimeStamp {
    double sample_time;
    uint64_t host_time;
    double rate_scalar;
    uint32_t flags;
} MRIOTimeStamp;
/* The callback's current time, then its input and output stream timestamps. */
typedef struct MRIOTiming { MRIOTimeStamp now, input, output; } MRIOTiming;
_Static_assert(sizeof(MRIOTimeStamp) == 32 && sizeof(MRIOTiming) == 96
    && offsetof(MRIOTiming, input) == 32 && offsetof(MRIOTiming, output) == 64,
    "Rust callback timing ABI");
/* Borrowed for this call only. Validity flags are CoreAudio sample/host/rate bits. */
typedef bool (*MRRenderHook)(void *, const void *, void *, uint32_t, const MRIOTiming *);
enum { MRPcmPrepare = 0, MRPcmCaptured = 1 };
/* Prepare admits native PCM; Captured converts newly admitted input in this callback. */
typedef bool (*MRPcmHook)(void *, uint32_t phase);
void mr_device_io_render_hook(MRDeviceIO *io, void *context, MRRenderHook hook);
void mr_device_io_pcm_hook(MRDeviceIO *io, void *context, MRPcmHook hook);
bool mr_device_io_direct_binding(MRDeviceIO *io, uint32_t *device, uint32_t *offset, uint32_t *total);
MRDeviceIO *mr_device_io_create_group(MRAggregateGroup *group, const char *uid,
    bool input, uint32_t channels, uint32_t rate);
/* Control creates/connects/disconnects/destroys. The native PCM hook owns SRC
 * and the opposite graph-ring cursor. Control drains hook publication before
 * replacing or releasing prepared conversion; callbacks never wait for control. */
MRDeviceIO *mr_device_io_create(const char *uid, bool input, uint32_t channels, uint32_t rate);
bool mr_device_io_connect(MRDeviceIO *io);
/* 0=connected; -1/-2/-3/-5=preflight stage, other values=native OSStatus.
 * The Rust owner decides reporting/recovery; this boundary never retries. */
int32_t mr_device_io_connect_status(MRDeviceIO *io, uint32_t expected_device);
/* Control-only buffer facts; state resolves immutable UID/group metadata
 * without racing the connection worker's mutable device handle. */
typedef struct MRDeviceBufferInfo {
    uint32_t device, frames;
    double minimum, maximum;
    uint32_t settable;
} MRDeviceBufferInfo;
_Static_assert(sizeof(MRDeviceBufferInfo) == 32, "Rust buffer snapshot ABI");
bool mr_device_io_buffer_info(MRDeviceIO *io, MRDeviceBufferInfo *info);
bool mr_device_buffer_write(uint32_t device, uint32_t frames);
uint32_t mr_device_buffer_size(uint32_t device);
/* False retains registration/storage: retry on the same control owner. Never
 * free callback context merely because a vendor stop/unregister call failed. */
bool mr_device_io_disconnect(MRDeviceIO *io);
bool mr_device_io_destroy(MRDeviceIO *io);
bool mr_device_io_alive(MRDeviceIO *io); /* control-only property queries */
/* Atomic callback-observed invalidation, no property IPC. NULL is invalid.
 * Listener-only notifications are visible after the next callback observes them. */
bool mr_device_io_invalid(MRDeviceIO *io);
uint64_t mr_device_io_ticks(MRDeviceIO *io);
/* The connect/liveness owner alone queries rate; frame progress is atomic. */
uint32_t mr_device_io_native_rate(MRDeviceIO *io);
/* Control owner only. Zero means no proof: in particular, a user Aggregate
 * may contain unsynchronized subdevices despite having a single object ID. */
uint32_t mr_device_io_clock_device(MRDeviceIO *io);
uint64_t mr_device_io_clock_discontinuities(MRDeviceIO *io);
typedef struct MRDeviceClockBreak {
    uint64_t count;
    double expected_sample, actual_sample;
    uint64_t previous_host, actual_host, flags;
    uint64_t callback_host, callback_flags;
    uint64_t previous_enter, previous_exit, callback_enter;
} MRDeviceClockBreak;
typedef struct MRDeviceClockBreakRecord {
    MRDeviceClockBreak event;
    uint64_t next_cursor, missed_before;
} MRDeviceClockBreakRecord;
_Static_assert(sizeof(MRDeviceClockBreak) == 88, "Rust clock-break event ABI");
_Static_assert(sizeof(MRDeviceClockBreakRecord) == 104, "Rust clock-break record ABI");
/* One bounded atomic read; false leaves output unchanged. Start cursor at zero
 * per retained owner, then pass next_cursor. Overwrites appear in missed_before. */
bool mr_device_io_next_clock_break(const MRDeviceIO *io, uint64_t cursor,
                                 MRDeviceClockBreakRecord *record);
uint64_t mr_device_io_native_frames(MRDeviceIO *io);
uint64_t mr_device_io_graph_frames(MRDeviceIO *io);
uint64_t mr_device_io_overflows(MRDeviceIO *io);
uint32_t mr_device_io_quantum(MRDeviceIO *io, bool graph);
void mr_device_io_output_epoch(MRDeviceIO *io, uint64_t epoch);
/* Native callbacks and graph never share a ring cursor. The bridge owns the
 * opposite side of each SPSC and its SRC state. Queued values are frames in
 * that ring's clock: graph=true uses graph rate, false uses native rate. */
uint32_t mr_device_io_queued(MRDeviceIO *io, bool graph);
uint64_t mr_device_io_starvations(MRDeviceIO *io, bool graph);
void mr_device_io_bridge_discard(MRDeviceIO *io);
typedef struct MRDeviceGraphRead {
    uint32_t frames;
    bool initial_epoch;
    bool discontinuity;
} MRDeviceGraphRead;
_Static_assert(sizeof(MRDeviceGraphRead) == 8, "Rust graph read ABI");
/* Actual valid frames; initial_epoch is true only for first consumer admission.
 * discontinuity identifies a changed admitted PCM epoch, even on an empty read.
 * Recovery epochs and subsequent shortages are not startup. */
/* host is the graph cycle's host time in ticks, shared by all of its transfers. */
MRDeviceGraphRead mr_device_io_graph_read(MRDeviceIO *io, float *samples, uint32_t frames, uint64_t epoch,
    uint64_t host);
uint32_t mr_device_io_graph_write(MRDeviceIO *io, const float *samples, uint32_t frames, uint64_t host);
/* The latest callback's mean-frame host ticks, or zero before any timed callback. */
uint64_t mr_device_io_callback_host(MRDeviceIO *io);
uint32_t mr_device_io_bridge_read(MRDeviceIO *io, float *samples, uint32_t frames);
uint32_t mr_device_io_bridge_write(MRDeviceIO *io, const float *samples, uint32_t frames);
/* Actual registered CoreAudio callback. Native fixtures supply complete native
 * buffer/timestamp extents and serialize the same role ownership as the OS. */
int32_t mr_device_io_process(uint32_t device, const void *now, const void *input,
    const void *input_time, void *output, const void *output_time, void *context);
#endif
