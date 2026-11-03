#ifndef MR_AUDIO_RING_H
#define MR_AUDIO_RING_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Rust-owned opaque SPSC ring. One serialized IOProc writes; one pump reads.
 * Destroy only after callback, consumer and stat-reader access has drained. */
typedef struct MRAudioRing MRAudioRing;

enum {
    MR_AUDIO_RING_MAX_CAPACITY_FRAMES = 1U << 20U,
};

MRAudioRing *mr_audio_ring_create(size_t capacity_frames, uint32_t channels);
void mr_audio_ring_destroy(MRAudioRing *ring);

size_t mr_audio_ring_write_interleaved(
    MRAudioRing *ring,
    const float *samples,
    size_t frames
);
size_t mr_audio_ring_write_planar(
    MRAudioRing *ring,
    const float *const *channel_samples,
    size_t frames
);
size_t mr_audio_ring_read_interleaved(
    MRAudioRing *ring,
    float *destination,
    size_t maximum_frames
);

size_t mr_audio_ring_capacity_frames(const MRAudioRing *ring);
/* Bounded diagnostic observation, not an atomic pair of producer/consumer cursors. */
size_t mr_audio_ring_available_frames(const MRAudioRing *ring);
uint64_t mr_audio_ring_dropped_frames(const MRAudioRing *ring);

#endif
