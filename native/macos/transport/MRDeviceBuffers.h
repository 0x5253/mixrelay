#ifndef MR_DEVICE_BUFFERS_H
#define MR_DEVICE_BUFFERS_H
#include <CoreAudio/CoreAudioTypes.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
/* Immutable channel selection; Rust validates metadata and owns PCM projection. */
typedef struct { uint32_t channels, offset, total, limit; } MRBufferSelection;
_Static_assert(sizeof(MRBufferSelection) == 16, "selection ABI");
_Static_assert(sizeof(AudioBuffer) == 16 && offsetof(AudioBufferList, mBuffers) == 8, "native buffer ABI");
uint32_t mr_device_buffers_frames(const AudioBufferList *, MRBufferSelection);
bool mr_device_buffers_transfer(const AudioBufferList *, MRBufferSelection, float *, bool input);
#endif
