#ifndef MR_PROBE_HAL_TRACE_H
#define MR_PROBE_HAL_TRACE_H

#include <CoreAudio/CoreAudio.h>
#include <stdbool.h>

typedef struct {
    const char *stage;
    OSStatus os_status;
    int file_error;
} MRProbeHalTraceResult;

MRProbeHalTraceResult mr_probe_hal_trace_enable(AudioDeviceID device, bool enabled);
MRProbeHalTraceResult mr_probe_hal_trace_save(AudioDeviceID device, const char *path);

#endif
