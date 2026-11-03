// One finite duplex capture plays the supplied PCM file; source transitions never reopen IO.
#include "macos-source-device.h"
#include "macos-source-pcm.h"
#include "../../native/macos/hal-driver/tests/MRProbeAuthorization.h"
#include "../../native/macos/hal-driver/tests/MRProbeHalTrace.h"
#include <errno.h>
#include <mach/mach_time.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>

extern bool mr_probe_with_activity(bool (*measure)(void *), void *context);

typedef struct Run {
    MRSourceDevice facts;
    MRSourcePcm pcm;
    OSStatus created, started, stopped, destroyed;
    bool registered, called;
} Run;

static bool run(void *context) {
    Run *state = context;
    state->called = true;
    AudioDeviceIOProcID procedure = NULL;
    state->created = AudioDeviceCreateIOProcID(state->facts.device, mr_source_io, &state->pcm, &procedure);
    state->registered = procedure != NULL;
    if (state->created == noErr && procedure != NULL) {
        state->started = AudioDeviceStart(state->facts.device, procedure);
        if (state->started == noErr) {
            const uint64_t nanos = state->pcm.source_frames * 1000000000U / state->facts.rate;
            struct timespec remaining = {.tv_sec = (time_t)(nanos / 1000000000U),
                .tv_nsec = (long)(nanos % 1000000000U)};
            while (nanosleep(&remaining, &remaining) != 0 && errno == EINTR) {}
            state->stopped = AudioDeviceStop(state->facts.device, procedure);
        }
    }
    if (procedure != NULL) {
        state->destroyed = AudioDeviceDestroyIOProcID(state->facts.device, procedure);
        if (state->destroyed == noErr) state->registered = false;
    }
    return state->created == noErr && state->started == noErr &&
           state->stopped == noErr && state->destroyed == noErr;
}

static void release(Run *state) {
    free((void *)state->pcm.source); free(state->pcm.recorded);
    free(state->pcm.input); free(state->pcm.output); free(state);
}

static bool load(Run *state, const char *path) {
    struct stat info;
    if (stat(path, &info) != 0 || info.st_size <= 0 || info.st_size > 16 * 1024 * 1024 ||
        info.st_size % 8 != 0) return false;
    const size_t bytes = (size_t)info.st_size;
    float *source = malloc(bytes);
    state->pcm.source = source;
    state->pcm.source_frames = bytes / 8U;
    state->pcm.capacity = state->pcm.source_frames + 8192U;
    state->pcm.chunk_capacity = 4096U;
    state->pcm.recorded = calloc((size_t)state->pcm.capacity * 2U, sizeof(float));
    state->pcm.input = calloc(state->pcm.chunk_capacity, sizeof(MRSourceChunk));
    state->pcm.output = calloc(state->pcm.chunk_capacity, sizeof(MRSourceChunk));
    state->pcm.quantum = state->facts.quantum;
    state->pcm.clock = mach_absolute_time;
    if (source == NULL || state->pcm.recorded == NULL || state->pcm.input == NULL ||
        state->pcm.output == NULL) return false;
    FILE *file = fopen(path, "rb");
    if (file == NULL) return false;
    const bool loaded = fread(source, 1U, bytes, file) == bytes && !ferror(file);
    return fclose(file) == 0 && loaded;
}

static bool save_pcm(const MRSourcePcm *pcm, const char *path) {
    FILE *file = fopen(path, "wbx");
    if (file == NULL) return false;
    const size_t count = (size_t)pcm->recorded_frames * 2U;
    const bool saved = fwrite(pcm->recorded, sizeof(float), count, file) == count;
    return fclose(file) == 0 && saved;
}

static void timing(const MRSourcePcm *pcm) {
    mach_timebase_info_data_t base;
    mach_timebase_info(&base);
    fprintf(stderr, "timebase,%u,%u\nside,first,sampleTime,hostTime,frames,flags,callbackEnter,callbackExit\n",
            base.numer, base.denom);
    const MRSourceChunk *sides[] = {pcm->output, pcm->input};
    const uint32_t counts[] = {pcm->output_count, pcm->input_count};
    for (size_t side = 0U; side < 2U; side++) for (uint32_t i = 0U; i < counts[side]; i++) {
        const MRSourceChunk *row = &sides[side][i];
        fprintf(stderr, "%s,%llu,%llu,%llu,%u,%u,%llu,%llu\n", side == 0U ? "output" : "input",
            (unsigned long long)row->first, (unsigned long long)row->sample,
            (unsigned long long)row->host, row->frames, row->flags,
            (unsigned long long)row->entered, (unsigned long long)row->exited);
    }
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--self-test") == 0) return mr_source_pcm_self_test();
    const int authorization_command = mr_authorization_command(argc, argv);
    if (authorization_command >= 0) return authorization_command;
    if (argc != 5) {
        fputs("usage: macos-source-control UID SOURCE.f32 CAPTURE.f32 HAL-TRACE.json\n", stderr);
        return 2;
    }
    MRInputAuthorization authorization = {0};
    if (!mr_authorize_input(30.0, &authorization)) return 2;
    Run *state = calloc(1U, sizeof(Run));
    if (state == NULL) return 1;
    state->created = state->started = state->stopped = state->destroyed = kAudioHardwareIllegalOperationError;
    const OSStatus admitted = mr_source_device(argv[1], &state->facts);
    if (admitted != noErr || !load(state, argv[2])) {
        fprintf(stderr, "source preparation failed: osStatus=%d errno=%d\n", (int)admitted, errno);
        release(state);
        return 1;
    }
    MRProbeHalTraceResult trace = mr_probe_hal_trace_enable(state->facts.device, true);
    if (trace.stage != NULL) { release(state); return 1; }
    const bool completed = mr_probe_with_activity(run, state);
    // Do not inspect callback-owned memory without native drain acknowledgement.
    if (state->registered) {
        printf("{\"error\":\"IOProc destruction failed\",\"osStatus\":%d}\n", (int)state->destroyed);
        return 1;
    }
    trace = mr_probe_hal_trace_save(state->facts.device, argv[4]);
    const MRProbeHalTraceResult disabled = mr_probe_hal_trace_enable(state->facts.device, false);
    const bool saved = save_pcm(&state->pcm, argv[3]);
    timing(&state->pcm);
    printf("{"); mr_print_input_authorization(stdout, &authorization);
    printf(",\"device\":%u,\"rate\":%u,\"channels\":%u,\"bufferFrames\":%u,"
           "\"timestampPeriod\":%u,"
           "\"sourceFrames\":%llu,\"recordedFrames\":%llu,\"origin\":%llu,"
           "\"inputCallbacks\":%u,\"outputCallbacks\":%u,\"invalidBuffers\":%u,"
           "\"invalidTimestamps\":%u,\"overflow\":%u,\"droppedChunks\":%u,"
           "\"nativeStatus\":{\"create\":%d,\"start\":%d,\"stop\":%d,\"destroy\":%d},"
           "\"activityProtection\":%s,\"completed\":%s,\"hasOrigin\":%s,"
           "\"captureSaved\":%s,\"traceSaved\":%s,\"traceDisabled\":%s}\n",
        state->facts.device, state->facts.rate, state->facts.channels, state->facts.quantum, state->facts.timestamp_period,
        (unsigned long long)state->pcm.source_frames, (unsigned long long)state->pcm.recorded_frames,
        (unsigned long long)state->pcm.origin, state->pcm.input_count, state->pcm.output_count,
        state->pcm.invalid_buffers, state->pcm.invalid_timestamps, state->pcm.overflow, state->pcm.dropped_chunks,
        (int)state->created, (int)state->started, (int)state->stopped, (int)state->destroyed,
        state->called ? "true" : "false", completed ? "true" : "false", state->pcm.has_origin ? "true" : "false",
        saved ? "true" : "false",
        trace.stage == NULL ? "true" : "false", disabled.stage == NULL ? "true" : "false");
    release(state);
    return completed && saved && trace.stage == NULL && disabled.stage == NULL ? 0 : 1;
}
