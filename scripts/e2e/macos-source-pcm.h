#ifndef MR_SOURCE_PCM_H
#define MR_SOURCE_PCM_H

#include <CoreAudio/CoreAudio.h>
#include <stdbool.h>
#include <stdint.h>

typedef struct MRSourceChunk {
    uint64_t first, sample, host, entered, exited;
    uint32_t frames, flags;
} MRSourceChunk;

// Prepared storage belongs to control until IOProc destruction acknowledges drain.
// The serialized callback is its sole writer while registered.
typedef struct MRSourcePcm {
    const float *source;
    uint64_t source_frames, capacity, recorded_frames, origin;
    float *recorded;
    MRSourceChunk *input, *output;
    uint32_t chunk_capacity, input_count, output_count, quantum;
    uint32_t invalid_buffers, invalid_timestamps, overflow, dropped_chunks;
    bool has_origin;
    uint64_t (*clock)(void);
} MRSourcePcm;

OSStatus mr_source_io(AudioDeviceID device, const AudioTimeStamp *now,
                     const AudioBufferList *input, const AudioTimeStamp *input_time,
                     AudioBufferList *output, const AudioTimeStamp *output_time, void *context);
int mr_source_pcm_self_test(void);

#endif
