#ifndef MR_ENDPOINT_TRANSPORT_H
#define MR_ENDPOINT_TRANSPORT_H

#include "MRSharedAudioBuffer.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MR_ENDPOINT_ID_BYTES = 16,
};

typedef struct MREndpointTransport MREndpointTransport;

typedef struct MREndpointTransportConfig {
    uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES];
    uint32_t capacity_frames;
    uint32_t render_channels;
    uint32_t capture_channels;
} MREndpointTransportConfig;

/* Native callback credits; deliberately no fabricated host timestamp. */
typedef struct MREndpointClockSnapshot {
    uint64_t epoch;
    uint64_t render_through;
    uint64_t capture_through;
    uint32_t sample_rate;
    uint32_t running;
    uint32_t capture_frames;
    uint32_t render_frames;
} MREndpointClockSnapshot;

/* Enter before PCM epoch reset; snapshots stay unavailable until final commit. */
bool mr_endpoint_transport_begin_clock_transition(MREndpointTransport *transport);
/* Final commit requires an active transition and completed PCM invalidation. */
void mr_endpoint_transport_clock_state(MREndpointTransport *transport,
                                       uint64_t epoch, uint32_t sample_rate, bool running);
void mr_endpoint_transport_clock_demand(MREndpointTransport *transport,
                                        bool render, uint64_t through_frame, uint32_t frames);
bool mr_endpoint_transport_clock_snapshot(const MREndpointTransport *transport,
                                           MREndpointClockSnapshot *snapshot);
/* A terminal lifecycle tag cannot be reused while remote readers may exist. */
bool mr_endpoint_transport_requires_replacement(const MREndpointTransport *transport);
/* Stable identity of this mapping, not its endpoint or POSIX shm inode. */
bool mr_endpoint_transport_incarnation(const MREndpointTransport *transport,
                                       uint8_t incarnation[16]);

size_t mr_endpoint_transport_required_bytes(
    const MREndpointTransportConfig *config);
MREndpointTransport *mr_endpoint_transport_initialize(
    void *memory,
    size_t memory_size,
    const MREndpointTransportConfig *config);
MREndpointTransport *mr_endpoint_transport_attach(void *memory,
                                                   size_t memory_size);

bool mr_endpoint_transport_config(const MREndpointTransport *transport,
                                  MREndpointTransportConfig *config);
MRSharedAudioBuffer *mr_endpoint_transport_render_buffer(
    MREndpointTransport *transport);
MRSharedAudioBuffer *mr_endpoint_transport_capture_buffer(
    MREndpointTransport *transport);


#ifdef __cplusplus
}
#endif

#endif
