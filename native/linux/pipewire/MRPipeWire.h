#ifndef MR_PIPEWIRE_H
#define MR_PIPEWIRE_H
#include <stdbool.h>
#include <stdint.h>
#include "MRPipeWireConnection.h"

typedef struct MRPWStream MRPWStream;
typedef struct MRPWDriver MRPWDriver;
/* Fixed C ABI values; native output means PCM leaves the engine-side ring. */
enum MRPWStreamMode {
    MR_PW_VIRTUAL_RENDER = 0,
    MR_PW_VIRTUAL_CAPTURE = 1,
    MR_PW_APPLICATION_CAPTURE = 2,
    MR_PW_DEVICE_INPUT = 3,
    MR_PW_DEVICE_OUTPUT = 4,
    /* Graph contribution crosses the native mixer in the next cycle. */
    MR_PW_GRAPH_CONTRIBUTION = 5,
};
typedef struct {
    uint64_t position, monotonic_ns;
    uint32_t clock_id, rate, frames;
} MRPWCycle;
typedef void (*MRPWProcess)(void *userdata, const MRPWCycle *cycle);

/* Prepare on the unstarted parent connection. It must outlive this stream.
 * Control thread only. mode uses MRPWStreamMode.
 * target is a registry object.serial, not a recyclable object id.
 * manual_output selects DSP-cycle releases; independent publishers use false.
 * ring_frames is preallocated retention, independent of node latency/quantum. */
MRPWStream *mr_pw_stream_open(const MRPWConnectionHandles *connection, const char *name, const char *description,
                             const char *target, uint32_t mode, uint32_t channels,
                             uint32_t sample_rate, uint32_t block_frames, bool manual_output,
                             uint32_t ring_frames);
/* Exact PCM encoding (MR_PCM_FORMAT_*); zero retains advertised alternatives. */
MRPWStream *mr_pw_stream_open_pcm(const MRPWConnectionHandles *connection, const char *name, const char *description,
                                 const char *target, uint32_t mode, uint32_t channels,
                                 uint32_t sample_rate, uint32_t block_frames, bool manual_output,
                                 uint32_t sample_format, uint32_t ring_frames);
void mr_pw_stream_close(MRPWStream *stream);
/* Control thread only; caller retains the stream throughout this call. Updates
 * metadata without reconnecting, changing format or restarting the PCM ring. */
int mr_pw_stream_set_description(MRPWStream *stream, const char *description);
/* The exact acquired stream's live registry object ID, never a name lookup.
 * UINT32_MAX means that the stream has not published a node. Control thread only. */
uint32_t mr_pw_stream_node_id(MRPWStream *stream);
/* Control only, after successful Rust-owned DSP monitor PortConfig preparation.
 * Native outputs are rejected. No RT path may invoke this activation boundary. */
int mr_pw_stream_activate_capture(MRPWStream *stream);
/* First native buffer-return failure, latched until this acquisition is retired.
 * Atomic observation; zero means no such failure. */
int mr_pw_stream_return_error(MRPWStream *stream);
int mr_pw_stream_ready(MRPWStream *stream);
/* Atomic control-side terminal state observation; PAUSED/CONNECTING are not failures. */
int mr_pw_stream_failed(MRPWStream *stream);
int mr_pw_stream_connected(MRPWStream *stream);
/* One native state snapshot: 0 unknown/unavailable, 1 paused, 2 streaming. */
int mr_pw_stream_input_activity(MRPWStream *stream);
/* Actual callback driver ID (high32) and rate (low32); zero means unobserved. */
uint64_t mr_pw_stream_clock(MRPWStream *stream);
/* Negotiated PCM rate (high32) and channels (low32); zero means unobserved. */
uint64_t mr_pw_stream_format(MRPWStream *stream);
/* One coherent negotiated tuple: rate high32, MR_PCM_FORMAT_* bits16..23,
 * channels low16. Zero means invalid/unavailable. Packed S24 uses three bytes. */
uint64_t mr_pw_stream_native_format(MRPWStream *stream);
/* Immutable mask of actually advertised fixed-rate/channel EnumFormat pods.
 * Bit (MR_PCM_FORMAT_* - 1) identifies each encoding; caller pins the owner. */
uint32_t mr_pw_stream_supported_formats(MRPWStream *stream);
/* Last callback graph frames (high32) and stream PCM frames (low32).
 * Variable quantum observation, NOT a negotiated fixed BufferFrameSize setting. */
uint64_t mr_pw_stream_quantum(MRPWStream *stream);
void mr_pw_stream_discard_input(MRPWStream *stream);
typedef struct { uint64_t overflows, starvations; } MRPWTransferCounters;
MRPWTransferCounters mr_pw_stream_counters(MRPWStream *stream);
/* Nonconsuming source-local discontinuity generation. Capture-only; native
 * ingress/ring failures, buffer-return failure and requested stream/clock
 * restarts advance it. Output transfer failures do not. Acquire observes
 * producer/control publications without reading borrowed IO data. */
uint64_t mr_pw_stream_capture_failures(MRPWStream *stream);
/* Independent diagnostic observations, not a transactional queue snapshot.
 * Each failure group has one serialized writer. Counts wrap unsigned and live
 * until stream destruction; state/format/ring restarts do not reset them.
 * Clock packs native ID high32/rate low32. Engine failures use the most recent
 * atomic native position copy, never the borrowed IO pointer. Zero clock is
 * unobserved; equal Hz alone does not permit joining different clocks.
 * Groups: native capture ingress discard (malformed required buffer/format/
 * geometry or ring write admission failure), native output read, engine capture
 * read, engine output write. Capture discard counts unknown geometry as a call
 * with zero known frames; dequeue-null and declared empty chunks are no-data,
 * not rejected writes. First/last positions belong to that stream only. */
typedef struct {
    uint64_t rejected_calls, rejected_frames;
    uint64_t first_position, last_position, first_clock, last_clock;
} MRPWRejectedTransfer;
typedef struct {
    uint64_t read, written, queued;
    MRPWRejectedTransfer failures[4];
    /* read is a cursor and includes discards/epoch skips; engine_read_frames
     * counts only successful engine delivery. Discard frames use the exclusive
     * consumer's cursor advance. Restart calls request an epoch, not an exact
     * discarded frame count. All values remain independent atomic observations. */
    uint64_t engine_read_frames, consumer_discard_calls, consumer_discard_frames;
    uint64_t native_restart_calls;
    /* Failed output scheduling releases, not rejected PCM frames or clock gaps. */
    uint64_t native_output_trigger_rejections;
} MRPWTransferSnapshot;
MRPWTransferSnapshot mr_pw_stream_transfer_snapshot(MRPWStream *stream);
/* DSP thread only: no locks, allocation, file I/O or PipeWire control calls. */
uint32_t mr_pw_stream_read(MRPWStream *stream, float *samples, uint32_t frames);
uint32_t mr_pw_stream_write(MRPWStream *stream, const float *samples, uint32_t frames);
/* DSP publisher only, once after all slices of one successful native cycle.
 * Input/automatic modes return -EINVAL; non-streaming output owners are a no-op.
 * Negative releases are counted separately in the transfer snapshot. PCM stays
 * queued and the next native cycle retries; this does not drive a new clock. */
int mr_pw_stream_trigger_output(MRPWStream *stream);

/* Own and start a complete Rust graph driver/context for a native probe.
 * userdata remains pinned and its process role is exclusive until close drains
 * callbacks. No external connection or callback may outlive that ownership. */
MRPWDriver *mr_pw_driver_open(MRPWProcess process, void *userdata);
void mr_pw_driver_close(MRPWDriver *driver);
#endif
