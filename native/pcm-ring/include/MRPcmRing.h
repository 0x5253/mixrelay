#ifndef MR_PCM_RING_H
#define MR_PCM_RING_H
#include <stdbool.h>
#include <stdint.h>
/* Embedded Rust owner. Zeroed storage is empty. Never copy a live ring;
 * clear only after producer, consumer and diagnostic readers have stopped. */
typedef struct { uint64_t storage[15]; } MRPcmRing;
_Static_assert(sizeof(MRPcmRing) == 120 && _Alignof(MRPcmRing) == 8, "PCM owner ABI");
typedef struct {
    uint64_t read, written, overflows, starvations;
    uint32_t channels, queued;
} MRPcmRingStats;
_Static_assert(sizeof(MRPcmRingStats) == 40 && _Alignof(MRPcmRingStats) == 8, "PCM stats ABI");
bool mr_pcm_ring_init(MRPcmRing *ring, uint32_t channels, uint32_t capacity);
void mr_pcm_ring_clear(MRPcmRing *ring);
uint32_t mr_pcm_ring_write(MRPcmRing *ring, const float *samples, uint32_t frames);
/* Sole producer: publishes neutral PCM without reading an external buffer. */
uint32_t mr_pcm_ring_write_silence(MRPcmRing *ring, uint32_t frames);
uint32_t mr_pcm_ring_read(MRPcmRing *ring, float *samples, uint32_t frames);
enum MRPcmSampleFormat {
    MR_PCM_FORMAT_FLOAT32 = 1, MR_PCM_FORMAT_SIGNED16 = 2,
    MR_PCM_FORMAT_SIGNED24 = 3, MR_PCM_FORMAT_SIGNED32 = 4
};
/* Interleaved LE bytes; signed24 is packed 3-byte PCM. Buffers contain exactly
 * frames*channels*sample_bytes accessible bytes, disjoint from ring storage.
 * The same sole producer/consumer, epoch, whole-block and prefill rules apply.
 * Byte buffers need no float alignment. Invalid format IDs return zero. */
uint32_t mr_pcm_ring_write_pcm(MRPcmRing *ring, const void *input, uint32_t frames, uint32_t format);
uint32_t mr_pcm_ring_read_pcm(MRPcmRing *ring, void *output, uint32_t frames, uint32_t format);
/* Caller owns clock-crossing priming; epoch checks and whole-read silence remain. */
uint32_t mr_pcm_ring_read_available(MRPcmRing *ring, float *samples, uint32_t frames);
void mr_pcm_ring_discard(MRPcmRing *ring); /* sole consumer only */
void mr_pcm_ring_restart(MRPcmRing *ring); /* control request, no wait */
MRPcmRingStats mr_pcm_ring_stats(const MRPcmRing *ring); /* independent observations */
#endif
