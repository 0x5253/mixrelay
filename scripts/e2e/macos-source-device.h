#ifndef MR_SOURCE_DEVICE_H
#define MR_SOURCE_DEVICE_H
#include <CoreAudio/CoreAudio.h>

typedef struct MRSourceDevice {
    AudioDeviceID device;
    uint32_t rate, channels, quantum, timestamp_period;
} MRSourceDevice;

// Read-only admission of one stereo, interleaved Float32 device at its current settings.
OSStatus mr_source_device(const char *uid, MRSourceDevice *facts);
#endif
