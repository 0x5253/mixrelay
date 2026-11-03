#ifndef MR_SHARED_AUDIO_BUFFER_H
#define MR_SHARED_AUDIO_BUFFER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MR_SHARED_AUDIO_BUFFER_MAX_CHANNELS = 64,
    MR_SHARED_AUDIO_BUFFER_MAX_CAPACITY_FRAMES = 1U << 20U,
};

typedef struct MRSharedAudioBuffer MRSharedAudioBuffer;

/* Explicit producer evidence; Unknown means no current producer observation. */
typedef enum MRProducerActivity {
    MR_PRODUCER_ACTIVITY_UNKNOWN = 0,
    MR_PRODUCER_ACTIVITY_INACTIVE = 1,
    MR_PRODUCER_ACTIVITY_AWAITING_RENDER = 2,
    MR_PRODUCER_ACTIVITY_ACTIVE = 3,
} MRProducerActivity;

void mr_shared_audio_buffer_set_producer_activity(MRSharedAudioBuffer *buffer,
                                                  MRProducerActivity activity);
MRProducerActivity mr_shared_audio_buffer_producer_activity(
    const MRSharedAudioBuffer *buffer);

typedef struct MRSharedAudioBufferStats {
    uint64_t written_frames;
    uint64_t read_frames;
    uint64_t missing_frames;
    uint64_t epoch;
    uint64_t published_through_frame;
} MRSharedAudioBufferStats;

size_t mr_shared_audio_buffer_required_bytes(uint32_t capacity_frames,
                                             uint32_t channels);

/*
 * Initializes a caller-owned, suitably aligned memory region. The region may
 * be backed by anonymous memory for tests or by a shared mmap in production.
 * The current schema has an 80-byte header (version 4), including a bounded
 * producer claim. Allocate via required_bytes; no prior wire schema is accepted.
 */
MRSharedAudioBuffer *mr_shared_audio_buffer_initialize(void *memory,
                                                       size_t memory_size,
                                                       uint32_t capacity_frames,
                                                       uint32_t channels);

/*
 * Validates and attaches to an initialized region without allocating or
 * storing process-local pointers inside the shared layout.
 */
MRSharedAudioBuffer *mr_shared_audio_buffer_attach(void *memory,
                                                   size_t memory_size);

bool mr_shared_audio_buffer_is_lock_free(const MRSharedAudioBuffer *buffer);

uint32_t mr_shared_audio_buffer_capacity(const MRSharedAudioBuffer *buffer);
uint32_t mr_shared_audio_buffer_channels(const MRSharedAudioBuffer *buffer);
uint64_t mr_shared_audio_buffer_published_through(
    const MRSharedAudioBuffer *buffer);

/* Bounded writer-side invalidation. One producer claim and one epoch attempt,
 * no spinning or storage clearing. A busy producer is marked for rearm.
 * Returns zero on contention or epoch exhaustion; retry on a later callback or
 * replace the transport on control. No generation is ever reused. */
uint64_t mr_shared_audio_buffer_try_begin_epoch(MRSharedAudioBuffer *buffer);

/* Reader-side restart request; one atomic mark, no clearing or generation reuse.
 * Marked transports read silence until their sole writer rearms or replaces them. */
uint64_t mr_shared_audio_buffer_request_epoch(MRSharedAudioBuffer *buffer);
bool mr_shared_audio_buffer_epoch_pending(const MRSharedAudioBuffer *buffer);
/* True after generation exhaustion or retirement of a dead producer. The control
 * owner must replace the mapping; normal pending rearm is not retirement. */
bool mr_shared_audio_buffer_requires_replacement(const MRSharedAudioBuffer *buffer);
/* Sole writer/control ownership is required: call only after all its callbacks
 * have drained or its predecessor process exited. Retires a remaining busy
 * claim without clearing/reusing storage. Never use against a live remote writer. */
bool mr_shared_audio_buffer_retire_busy_producer_after_drain(MRSharedAudioBuffer *buffer);

/*
 * Reads and writes interleaved Float32 PCM at an absolute sample-frame index.
 * Both functions are allocation-free and never wait. A read always initializes
 * the complete destination; frames that have not been published for the
 * current epoch and absolute frame index are returned as silence. Write claims
 * the producer before comparing expected_epoch; an observation invalidated by
 * rearm cannot publish its PCM into a new epoch. A failed claim/write is retried
 * only with a fresh valid snapshot by the caller, never via an unchecked path.
 */
bool mr_shared_audio_buffer_write(MRSharedAudioBuffer *buffer,
                                  uint64_t expected_epoch,
                                  uint64_t start_frame,
                                  const float *interleaved_samples,
                                  uint32_t frame_count);
uint32_t mr_shared_audio_buffer_read(MRSharedAudioBuffer *buffer,
                                     uint64_t start_frame,
                                     float *interleaved_samples,
                                     uint32_t frame_count);

MRSharedAudioBufferStats mr_shared_audio_buffer_stats(
    const MRSharedAudioBuffer *buffer);

#ifdef __cplusplus
}
#endif

#endif
