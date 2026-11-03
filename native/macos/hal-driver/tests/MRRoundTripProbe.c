// Measures an audio path end to end on installed devices of any vendor: plays a
// deterministic signal into one device's output, records another (or the same) device's
// input, and reports latency, bit exactness, channel order, slips, dropouts and repeats,
// next to the latency the devices report. It never changes a default device or a device
// setting.
//
// Every signal sample is k/32768 with 1 <= |k| <= 8192 (exact in Float32 and every integer
// depth) and never zero, so a recorded zero where the signal is due is a dropout. Its low
// six bits name the channel; its upper eight bits are one byte of an invertible 32-bit
// code of (frame / 4, channel). Four consecutive samples therefore give the frame
// position directly, each channel carries independent content (a mix of channels never
// reproduces another channel), and any recorded channel can locate the signal.
#include "MRProbeHalTrace.h"
#include "MRProbeAuthorization.h"
#include <CoreAudio/CoreAudio.h>
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>

#include <errno.h>
#include <stddef.h>
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
    MR_MAX_CHANNELS = 64,
    MR_WINDOW = 32,  // frames that must match to lock onto the signal
    MR_HEADROOM = 8192,  // recording space beyond the run, so a final buffer always fits
};
static const uint32_t MR_UNMAPPED = UINT32_MAX;
static const double MR_LEAD_SECONDS = 0.1;  // silence before the signal
static const double MR_TAIL_SECONDS = 1.5;  // recording after the signal, for slow paths
static const double MR_TIMING_SECONDS = 0.1;

// Impulse mode measures paths that change samples (asynchronous clocks, sample-rate
// conversion, gain), where no exact copy exists. It plays 32-frame pulses whose spacing
// varies pseudo-randomly around 0.25 s, finds each arrival at half its own received peak,
// and searches for the single delay that aligns the arrival times with the played times;
// irregular spacing leaves only one delay that aligns many pulses, whatever its length.
static const double MR_IMPULSE_PERIOD_SECONDS = 0.25;
enum {
    MR_IMPULSE_WIDTH = 32,
    MR_IMPULSE_QUIET = 256,  // silent frames that must precede an arrival
    MR_IMPULSE_RISE = 16,    // frames a smoothed rise may take to reach half its peak
    MR_IMPULSE_TRACE = 512,  // all played pulses in the CLI's maximum 120-second run
};
static const float MR_IMPULSE_LEVEL = 0.5F;
static const float MR_IMPULSE_SILENCE = 0.05F;

typedef struct MRChunk {
    double sample_time, rate_scalar;
    uint64_t host_time, first;  // first: recording frame (input) or signal frame (output)
    uint32_t flags, frames;
    uint64_t callback_enter, callback_exit;
} MRChunk;

typedef struct MRSide {
    MRChunk *chunks;
    uint32_t capacity;
    _Atomic uint32_t count, dropped_chunks, inconsistent_buffers;
} MRSide;

typedef struct MRProbe {
    AudioDeviceID output, input;
    bool impulse;
    uint32_t channels;
    double rate;
    uint64_t lead, signal_frames, run_frames, capacity;
    float *recorded;  // capacity * channels, interleaved
    _Atomic uint64_t recorded_frames;
    _Atomic uint32_t overflow;
    _Atomic uint32_t latency_notifications;
    _Atomic double origin;  // first output sample time, NAN before the first output cycle
    MRSide out, in;
} MRProbe;

// ---------------------------------------------------------------------------------------
// Signal

static uint32_t mr_permute(uint32_t x) {
    x ^= x >> 16;
    x *= 0x7feb352dU;
    x ^= x >> 15;
    x *= 0x846ca68bU;
    x ^= x >> 16;
    return x;
}

static uint32_t mr_unpermute(uint32_t x) {
    x ^= x >> 16;
    x *= 0x43021123U;
    x ^= x >> 15 ^ x >> 30;
    x *= 0x1d69e2a5U;
    x ^= x >> 16;
    return x;
}

static uint32_t mr_channel_key(uint32_t channel) { return (channel + 1U) * 0x9E3779B9U; }

static float mr_signal(uint64_t frame, uint32_t channel) {
    const uint32_t code = mr_permute((uint32_t)(frame >> 2) ^ mr_channel_key(channel));
    const uint32_t payload = (code >> ((frame & 3U) * 8U)) & 0xFFU;
    int32_t k = (int32_t)((payload << 6) | channel) - 8192;
    if (k >= 0) k += 1;
    return (float)k / 32768.0F;
}

/// The (k + 8192) word of a recorded sample, or UINT32_MAX when it is not a signal value.
static uint32_t mr_word(float value) {
    const float scaled = value * 32768.0F;
    if (!(fabsf(scaled) >= 1.0F && fabsf(scaled) <= 8192.0F) || scaled != floorf(scaled)) {
        return UINT32_MAX;
    }
    int32_t k = (int32_t)scaled;
    if (k > 0) k -= 1;
    return (uint32_t)(k + 8192);
}

/// The channel a recorded sample names, or MR_UNMAPPED when it is not a signal value.
static uint32_t mr_channel_of(float value) {
    const uint32_t word = mr_word(value);
    return word == UINT32_MAX ? MR_UNMAPPED : word & 63U;
}

/// The signal frame of `samples[0]` when four consecutive samples (stride `stride`) form
/// one code block of channel `channel` starting at block phase `phase`.
static bool mr_decode(const float *samples, size_t stride, uint32_t channel, uint32_t phase,
                      int64_t *frame) {
    uint32_t code = 0U;
    for (uint32_t i = 0U; i < 4U; ++i) {
        const uint32_t word = mr_word(samples[(phase + i) * stride]);
        if (word == UINT32_MAX || (word & 63U) != channel) return false;
        code |= (word >> 6) << (8U * i);
    }
    const uint64_t block = mr_unpermute(code) ^ mr_channel_key(channel);
    *frame = (int64_t)(block << 2) - (int64_t)phase;
    return true;
}

static uint64_t mr_impulse_period(const MRProbe *probe) {
    return (uint64_t)(MR_IMPULSE_PERIOD_SECONDS * probe->rate);
}

/// Pulse k starts within its period slot, displaced by up to a quarter period.
static uint64_t mr_impulse_start(const MRProbe *probe, uint64_t k) {
    const uint64_t period = mr_impulse_period(probe);
    return probe->lead + k * period + mr_permute((uint32_t)k + 1U) % (period / 4U + 1U);
}

/// Pulses that end before the signal does; a cut pulse is neither played nor counted.
static uint64_t mr_impulse_count(const MRProbe *probe) {
    uint64_t count = 0U;
    while (mr_impulse_start(probe, count) + MR_IMPULSE_WIDTH <= probe->signal_frames) count += 1U;
    return count;
}

static float mr_expected(const MRProbe *probe, int64_t frame, uint32_t channel) {
    if (frame < (int64_t)probe->lead || frame >= (int64_t)probe->signal_frames) return 0.0F;
    if (probe->impulse) {
        const uint64_t k = ((uint64_t)frame - probe->lead) / mr_impulse_period(probe);
        const uint64_t start = mr_impulse_start(probe, k);
        const bool inside = (uint64_t)frame >= start && (uint64_t)frame < start + MR_IMPULSE_WIDTH;
        return inside && start + MR_IMPULSE_WIDTH <= probe->signal_frames ? MR_IMPULSE_LEVEL
                                                                          : 0.0F;
    }
    return mr_signal((uint64_t)frame, channel);
}

// ---------------------------------------------------------------------------------------
// IO procedures (realtime: no allocation, no locks, no output)

/// Frames in a callback when every enabled buffer agrees; 0 when none or they disagree.
static uint32_t mr_frames(const AudioBufferList *list) {
    bool any = false;
    uint32_t frames = 0U;
    for (UInt32 b = 0U; b < list->mNumberBuffers; ++b) {
        const AudioBuffer *buffer = &list->mBuffers[b];
        if (buffer->mNumberChannels == 0U || buffer->mData == NULL) continue;
        const uint32_t stride = (uint32_t)sizeof(float) * buffer->mNumberChannels;
        if (buffer->mDataByteSize % stride != 0U) return 0U;
        const uint32_t count = buffer->mDataByteSize / stride;
        if (any && count != frames) return 0U;
        any = true;
        frames = count;
    }
    return frames;
}

static void mr_note(MRSide *side, const AudioTimeStamp *time, uint32_t frames, uint64_t first) {
    const uint32_t index = atomic_load(&side->count);
    if (index >= side->capacity) {
        atomic_fetch_add(&side->dropped_chunks, 1U);
        return;
    }
    side->chunks[index] = (MRChunk){time->mSampleTime, time->mRateScalar, time->mHostTime,
                                    first, time->mFlags, frames, 0U, 0U};
    atomic_store(&side->count, index + 1U);
}

static void mr_play(MRProbe *probe, AudioBufferList *out, const AudioTimeStamp *time) {
    double origin = atomic_load(&probe->origin);
    if (isnan(origin)) {
        origin = time->mSampleTime;
        atomic_store(&probe->origin, origin);
    }
    const uint32_t frames = mr_frames(out);
    const double offset = time->mSampleTime - origin;
    const uint64_t first = offset < 0.0 ? 0U : (uint64_t)offset;
    uint32_t base = 0U;
    for (UInt32 b = 0U; b < out->mNumberBuffers; ++b) {
        AudioBuffer *buffer = &out->mBuffers[b];
        const uint32_t width = buffer->mNumberChannels;
        if (buffer->mData != NULL && frames != 0U) {
            float *samples = buffer->mData;
            for (uint32_t f = 0U; f < frames; ++f) {
                for (uint32_t c = 0U; c < width; ++c) {
                    const uint32_t channel = base + c;
                    samples[f * width + c] =
                        channel < probe->channels
                            ? mr_expected(probe, (int64_t)(first + f), channel)
                            : 0.0F;
                }
            }
        }
        base += width;  // a disabled buffer still owns its channels
    }
    if (frames == 0U) {
        atomic_fetch_add(&probe->out.inconsistent_buffers, 1U);
        return;
    }
    if (first < probe->run_frames) mr_note(&probe->out, time, frames, first);
}

static void mr_record(MRProbe *probe, const AudioBufferList *in, const AudioTimeStamp *time) {
    if (isnan(atomic_load(&probe->origin))) return;
    const uint64_t first = atomic_load(&probe->recorded_frames);
    // The audio owner closes the capture horizon independently of control-side
    // AudioDeviceStop scheduling. Keep the whole final callback in the headroom.
    if (probe->run_frames != 0U && first >= probe->run_frames) return;
    const uint32_t frames = mr_frames(in);
    if (frames == 0U) {
        atomic_fetch_add(&probe->in.inconsistent_buffers, 1U);
        return;
    }
    if (first + frames > probe->capacity) {
        atomic_store(&probe->overflow, 1U);
        return;
    }
    uint32_t base = 0U;
    for (UInt32 b = 0U; b < in->mNumberBuffers; ++b) {
        const AudioBuffer *buffer = &in->mBuffers[b];
        const uint32_t width = buffer->mNumberChannels;
        if (buffer->mData != NULL) {
            const float *samples = buffer->mData;
            for (uint32_t f = 0U; f < frames; ++f) {
                for (uint32_t c = 0U; c < width && base + c < probe->channels; ++c) {
                    probe->recorded[(first + f) * probe->channels + base + c] =
                        samples[f * width + c];
                }
            }
        }
        base += width;
    }
    mr_note(&probe->in, time, frames, first);
    atomic_store(&probe->recorded_frames, first + frames);
}

static OSStatus mr_io(AudioObjectID device, const AudioTimeStamp *now, const AudioBufferList *in,
                      const AudioTimeStamp *in_time, AudioBufferList *out,
                      const AudioTimeStamp *out_time, void *context) {
    (void)now;
    MRProbe *probe = context;
    const uint64_t entered = mach_absolute_time();
    const uint32_t out_before = atomic_load(&probe->out.count);
    const uint32_t in_before = atomic_load(&probe->in.count);
    if (device == probe->output && out != NULL && out_time != NULL) mr_play(probe, out, out_time);
    if (device == probe->input && in != NULL && in_time != NULL) mr_record(probe, in, in_time);
    const uint64_t exited = mach_absolute_time();
    if (device == probe->output && atomic_load(&probe->out.count) > out_before) {
        probe->out.chunks[out_before].callback_enter = entered;
        probe->out.chunks[out_before].callback_exit = exited;
    }
    if (device == probe->input && atomic_load(&probe->in.count) > in_before) {
        probe->in.chunks[in_before].callback_enter = entered;
        probe->in.chunks[in_before].callback_exit = exited;
    }
    return noErr;
}

// ---------------------------------------------------------------------------------------
// Device properties

static bool mr_get(AudioObjectID object, AudioObjectPropertySelector selector,
                   AudioObjectPropertyScope scope, void *value, UInt32 size) {
    const AudioObjectPropertyAddress address = {selector, scope, kAudioObjectPropertyElementMain};
    UInt32 actual = size;
    return AudioObjectGetPropertyData(object, &address, 0U, NULL, &actual, value) == noErr &&
           actual == size;
}

static void mr_string(AudioObjectID object, AudioObjectPropertySelector selector, char *text,
                      size_t size) {
    CFStringRef value = NULL;
    text[0] = '\0';
    if (mr_get(object, selector, kAudioObjectPropertyScopeGlobal, &value, sizeof(value)) &&
        value != NULL) {
        CFStringGetCString(value, text, (CFIndex)size, kCFStringEncodingUTF8);
        CFRelease(value);
    }
}

/// Stream objects of one direction; returns how many were written (at most `limit`).
static uint32_t mr_streams(AudioDeviceID device, AudioObjectPropertyScope scope,
                           AudioObjectID *streams, uint32_t limit) {
    const AudioObjectPropertyAddress address = {kAudioDevicePropertyStreams, scope,
                                                kAudioObjectPropertyElementMain};
    UInt32 size = (UInt32)(limit * sizeof(AudioObjectID));
    if (AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, streams) != noErr) return 0U;
    return size / (uint32_t)sizeof(AudioObjectID);
}

static uint32_t mr_channels(AudioDeviceID device, AudioObjectPropertyScope scope) {
    const AudioObjectPropertyAddress address = {kAudioDevicePropertyStreamConfiguration, scope,
                                                kAudioObjectPropertyElementMain};
    UInt32 size = 0U;
    if (AudioObjectGetPropertyDataSize(device, &address, 0U, NULL, &size) != noErr || size == 0U) {
        return 0U;
    }
    AudioBufferList *list = malloc(size);
    if (list == NULL) return 0U;
    uint32_t channels = 0U;
    if (AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, list) == noErr) {
        for (UInt32 b = 0U; b < list->mNumberBuffers; ++b) {
            channels += list->mBuffers[b].mNumberChannels;
        }
    }
    free(list);
    return channels;
}

/// What a device says about one direction. A query that fails is unknown, never zero.
typedef struct MRReported {
    uint32_t latency, safety, stream_latency, buffer;
    bool latency_known, safety_known, stream_known, buffer_known;
} MRReported;

static MRReported mr_reported(AudioDeviceID device, AudioObjectPropertyScope scope) {
    MRReported reported = {0};
    reported.latency_known =
        mr_get(device, kAudioDevicePropertyLatency, scope, &reported.latency, sizeof(UInt32));
    reported.safety_known =
        mr_get(device, kAudioDevicePropertySafetyOffset, scope, &reported.safety, sizeof(UInt32));
    reported.buffer_known =
        mr_get(device, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal,
               &reported.buffer, sizeof(UInt32));
    AudioObjectID stream = kAudioObjectUnknown;
    reported.stream_known =
        mr_streams(device, scope, &stream, 1U) == 1U &&
        mr_get(stream, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal,
               &reported.stream_latency, sizeof(UInt32));
    return reported;
}

static bool mr_reported_total(const MRReported *reported, uint32_t *total) {
    *total = reported->latency + reported->safety + reported->stream_latency;
    return reported->latency_known && reported->safety_known && reported->stream_known;
}

/// Every stream of the direction must hand the IO procedure packed native Float32.
static bool mr_float_streams(AudioDeviceID device, AudioObjectPropertyScope scope, double rate) {
    AudioObjectID streams[MR_MAX_CHANNELS];
    const uint32_t count = mr_streams(device, scope, streams, MR_MAX_CHANNELS);
    if (count == 0U) return false;
    for (uint32_t s = 0U; s < count; ++s) {
        AudioStreamBasicDescription format;
        if (!mr_get(streams[s], kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal,
                    &format, sizeof(format)) ||
            format.mFormatID != kAudioFormatLinearPCM ||
            (format.mFormatFlags & kAudioFormatFlagIsFloat) == 0U ||
            (format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0U ||
            (format.mFormatFlags & kAudioFormatFlagIsBigEndian) != 0U ||
            format.mBitsPerChannel != 32U ||
            format.mBytesPerFrame != 4U * format.mChannelsPerFrame || format.mSampleRate != rate) {
            return false;
        }
    }
    return true;
}

static void mr_json_string(const char *text) {
    putchar('"');
    for (const unsigned char *c = (const unsigned char *)text; *c != '\0'; ++c) {
        if (*c == '"' || *c == '\\') {
            printf("\\%c", *c);
        } else if (*c < 0x20U) {
            printf("\\u%04x", *c);
        } else {
            putchar(*c);
        }
    }
    putchar('"');
}

static void mr_json_known(const char *key, bool known, uint32_t value) {
    if (known) {
        printf("\"%s\":%u", key, value);
    } else {
        printf("\"%s\":null", key);
    }
}

static void mr_text_known(bool known, uint32_t value) {
    if (known) {
        printf("%u", value);
    } else {
        printf("?");
    }
}

static void mr_describe_clock(AudioDeviceID device) {
    UInt32 algorithm = 0, period = 0, stable = 0;
    const bool algorithm_known = mr_get(device, kAudioDevicePropertyClockAlgorithm,
        kAudioObjectPropertyScopeGlobal, &algorithm, sizeof(algorithm));
    const bool period_known = mr_get(device, kAudioDevicePropertyZeroTimeStampPeriod,
        kAudioObjectPropertyScopeGlobal, &period, sizeof(period));
    const bool stable_known = mr_get(device, kAudioDevicePropertyClockIsStable,
        kAudioObjectPropertyScopeGlobal, &stable, sizeof(stable));
    putchar(',');
    mr_json_known("clockAlgorithm", algorithm_known, algorithm);
    putchar(',');
    mr_json_known("timestampPeriod", period_known, period);
    putchar(',');
    mr_json_known("clockStable", stable_known, stable);
}

static void mr_describe(const char *role, AudioDeviceID device, AudioObjectPropertyScope scope,
                        bool json) {
    char name[256], uid[256];
    mr_string(device, kAudioObjectPropertyName, name, sizeof(name));
    mr_string(device, kAudioDevicePropertyDeviceUID, uid, sizeof(uid));
    Float64 rate = 0.0;
    mr_get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, &rate,
           sizeof(rate));
    UInt32 transport = 0U;
    mr_get(device, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, &transport,
           sizeof(transport));
    const char code[5] = {(char)(transport >> 24), (char)(transport >> 16), (char)(transport >> 8),
                          (char)transport, '\0'};
    const MRReported reported = mr_reported(device, scope);
    const uint32_t channels = mr_channels(device, scope);
    if (json) {
        printf("\"%s\":{\"name\":", role);
        mr_json_string(name);
        printf(",\"uid\":");
        mr_json_string(uid);
        printf(",\"transport\":");
        mr_json_string(code);
        printf(",\"rate\":%.0f,\"channels\":%u,", rate, channels);
        mr_json_known("bufferFrames", reported.buffer_known, reported.buffer);
        putchar(',');
        mr_json_known("latency", reported.latency_known, reported.latency);
        putchar(',');
        mr_json_known("safetyOffset", reported.safety_known, reported.safety);
        putchar(',');
        mr_json_known("streamLatency", reported.stream_known, reported.stream_latency);
        mr_describe_clock(device);
        printf("},");
        return;
    }
    printf("%-6s %s (%s) '%s' %.0f Hz, %u ch, buffer ", role, name, uid, code, rate, channels);
    mr_text_known(reported.buffer_known, reported.buffer);
    printf("; reports latency ");
    mr_text_known(reported.latency_known, reported.latency);
    printf(" + safety ");
    mr_text_known(reported.safety_known, reported.safety);
    printf(" + stream ");
    mr_text_known(reported.stream_known, reported.stream_latency);
    printf(" frames\n");
}

static AudioDeviceID mr_find(const char *text) {
    CFStringRef uid = CFStringCreateWithCString(NULL, text, kCFStringEncodingUTF8);
    if (uid == NULL) return kAudioObjectUnknown;
    const AudioObjectPropertyAddress address = {kAudioHardwarePropertyTranslateUIDToDevice,
                                                kAudioObjectPropertyScopeGlobal,
                                                kAudioObjectPropertyElementMain};
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    const OSStatus status = AudioObjectGetPropertyData(kAudioObjectSystemObject, &address,
                                                       sizeof(uid), &uid, &size, &device);
    CFRelease(uid);
    return status == noErr ? device : kAudioObjectUnknown;
}

// Read-only query from this process: no IOProc, recording or default-device changes.
// Queries several times: the first answer may precede the HAL's identity discovery.
static int mr_transport(const char *uid) {
    const AudioObjectID device = mr_find(uid);
    for (int attempt = 0; attempt < 5; attempt += 1) {
        UInt32 transport = 0;
        if (device == kAudioObjectUnknown ||
            !mr_get(device, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal,
                    &transport, sizeof(transport))) {
            fprintf(stderr, "transport query failed for %s\n", uid);
            return 1;
        }
        printf("pid=%ld attempt=%d transport='%c%c%c%c'\n", (long)getpid(), attempt,
               (int)((transport >> 24) & 255U), (int)((transport >> 16) & 255U),
               (int)((transport >> 8) & 255U), (int)(transport & 255U));
        usleep(100000);
    }
    return 0;
}

// Read-only latency inventory; does not create an IOProc or change device settings.
static int mr_latency(const char *uid) {
    const AudioDeviceID device = mr_find(uid);
    if (device == kAudioObjectUnknown) {
        fprintf(stderr, "device not found: %s\n", uid);
        return 1;
    }
    printf("Reported values in device-rate frames:\n");
    mr_describe("input", device, kAudioObjectPropertyScopeInput, false);
    mr_describe("output", device, kAudioObjectPropertyScopeOutput, false);
    return 0;
}

static int mr_list(void) {
    const AudioObjectPropertyAddress address = {kAudioHardwarePropertyDevices,
                                                kAudioObjectPropertyScopeGlobal,
                                                kAudioObjectPropertyElementMain};
    UInt32 size = 0U;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0U, NULL, &size) !=
        noErr) {
        return 1;
    }
    AudioDeviceID *devices = malloc(size);
    if (devices == NULL ||
        AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0U, NULL, &size, devices) !=
            noErr) {
        free(devices);
        return 1;
    }
    for (UInt32 i = 0U; i < size / sizeof(AudioDeviceID); ++i) {
        if (mr_channels(devices[i], kAudioObjectPropertyScopeOutput) > 0U) {
            mr_describe("out", devices[i], kAudioObjectPropertyScopeOutput, false);
        }
        if (mr_channels(devices[i], kAudioObjectPropertyScopeInput) > 0U) {
            mr_describe("in", devices[i], kAudioObjectPropertyScopeInput, false);
        }
    }
    free(devices);
    return 0;
}

// ---------------------------------------------------------------------------------------
// Analysis: every recording frame is matched to a signal frame under a lock (offset and
// channel map). On a mismatch the signal is located again from the recording itself; a
// new offset is a slip, a new channel map a channel change, and otherwise the frame is
// damaged. Locating is O(channels), so damage never makes the analysis slow.

typedef struct MRAnalysis {
    bool same_device;
    uint64_t recorded, expected_frames, received, missing, repeated, head_missing, tail_missing;
    uint64_t corrupt_frames, zero_samples, wrong_samples, nonfinite_samples;
    uint64_t locks, slips, map_changes, timed;
    int64_t slip_frames;  // net signal frames skipped (positive) or repeated (negative)
    double max_error, lag_min, lag_max, lag_sum, ms_min, ms_max, ms_sum;
    uint32_t map[MR_MAX_CHANNELS];
    uint32_t out_discontinuities, in_discontinuities;
} MRAnalysis;

static const float *mr_frame(const MRProbe *probe, uint64_t r) {
    return &probe->recorded[r * probe->channels];
}

/// True when recorded channel `c` carries signal channel `k` for a window from `r` at `n`.
static bool mr_window(const MRProbe *probe, uint64_t r, uint32_t c, uint32_t k, int64_t n) {
    if (n < (int64_t)probe->lead || n + MR_WINDOW > (int64_t)probe->signal_frames ||
        r + MR_WINDOW > atomic_load(&probe->recorded_frames)) {
        return false;
    }
    for (uint32_t i = 0U; i < MR_WINDOW; ++i) {
        if (mr_frame(probe, r + i)[c] != mr_signal((uint64_t)n + i, k)) return false;
    }
    return true;
}

/// The signal frame at recording frame r, read from any recorded channel that carries an
/// intact window there. Tries every channel and block phase, so one bad channel cannot
/// hide a good one.
static bool mr_locate(const MRProbe *probe, uint64_t r, int64_t *found) {
    if (r + MR_WINDOW > atomic_load(&probe->recorded_frames)) return false;
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        const uint32_t k = mr_channel_of(mr_frame(probe, r)[c]);
        if (k >= probe->channels) continue;
        for (uint32_t phase = 0U; phase < 4U; ++phase) {
            int64_t n = 0;
            if (mr_decode(mr_frame(probe, r) + c, probe->channels, k, phase, &n) &&
                mr_window(probe, r, c, k, n)) {
                *found = n;
                return true;
            }
        }
    }
    return false;
}

/// Which signal channel each recorded channel carries at (r, n). A channel without an
/// intact window here keeps `fallback` (its previous mapping, or itself before any lock).
static void mr_derive_map(const MRProbe *probe, uint64_t r, int64_t n, const uint32_t *fallback,
                          uint32_t *map) {
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        const uint32_t k = mr_channel_of(mr_frame(probe, r)[c]);
        map[c] = k < probe->channels && mr_window(probe, r, c, k, n) ? k : fallback[c];
    }
}

static bool mr_frame_matches(const MRProbe *probe, uint64_t r, int64_t n, const uint32_t *map) {
    const float *frame = mr_frame(probe, r);
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        if (frame[c] != mr_expected(probe, n, map[c])) return false;
    }
    return true;
}

static void mr_count_damage(const MRProbe *probe, uint64_t r, int64_t n, const uint32_t *map,
                            MRAnalysis *analysis) {
    analysis->corrupt_frames += 1U;
    const float *frame = mr_frame(probe, r);
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        const float want = mr_expected(probe, n, map[c]);
        const float got = frame[c];
        if (got == want) continue;
        if (!isfinite(got)) {
            analysis->nonfinite_samples += 1U;
        } else if (got == 0.0F) {
            analysis->zero_samples += 1U;
        } else {
            analysis->wrong_samples += 1U;
            analysis->max_error = fmax(analysis->max_error, fabs((double)got - (double)want));
        }
    }
}

/// Chunk holding `frame`; chunks are ordered by `first`.
static const MRChunk *mr_chunk(const MRSide *side, uint64_t frame) {
    uint32_t low = 0U, high = atomic_load(&side->count);
    while (low < high) {
        const uint32_t middle = low + (high - low) / 2U;
        const MRChunk *chunk = &side->chunks[middle];
        if (frame < chunk->first) {
            high = middle;
        } else if (frame >= chunk->first + chunk->frames) {
            low = middle + 1U;
        } else {
            return chunk;
        }
    }
    return NULL;
}

/// Sample and host time of a frame, interpolated with the chunk's own rate scalar.
static bool mr_time(const MRSide *side, uint64_t frame, double ticks_per_frame, double *sample,
                    double *host) {
    const MRChunk *chunk = mr_chunk(side, frame);
    const uint32_t needed = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid;
    if (chunk == NULL || (chunk->flags & needed) != needed) return false;
    const double scalar =
        (chunk->flags & kAudioTimeStampRateScalarValid) != 0U && chunk->rate_scalar > 0.0
            ? chunk->rate_scalar
            : 1.0;
    const double within = (double)(frame - chunk->first);
    *sample = chunk->sample_time + within;
    *host = (double)chunk->host_time + within * ticks_per_frame * scalar;
    return true;
}

static void mr_time_lock(const MRProbe *probe, uint64_t r, int64_t n, double ns_per_tick,
                         MRAnalysis *analysis) {
    const double ticks_per_frame = 1e9 / probe->rate / ns_per_tick;
    double out_sample = 0.0, out_host = 0.0, in_sample = 0.0, in_host = 0.0;
    if (!mr_time(&probe->out, (uint64_t)n, ticks_per_frame, &out_sample, &out_host) ||
        !mr_time(&probe->in, r, ticks_per_frame, &in_sample, &in_host)) {
        return;
    }
    analysis->timed += 1U;
    const double ms = (in_host - out_host) * ns_per_tick / 1e6;
    analysis->ms_min = fmin(analysis->ms_min, ms);
    analysis->ms_max = fmax(analysis->ms_max, ms);
    analysis->ms_sum += ms;
    // Only meaningful on one device: sample times of two devices share no origin.
    const double lag = in_sample - out_sample;
    analysis->lag_min = fmin(analysis->lag_min, lag);
    analysis->lag_max = fmax(analysis->lag_max, lag);
    analysis->lag_sum += lag;
}

static uint32_t mr_discontinuities(const MRSide *side) {
    uint32_t count = 0U;
    const uint32_t chunks = atomic_load(&side->count);
    for (uint32_t i = 1U; i < chunks; ++i) {
        const MRChunk *previous = &side->chunks[i - 1U];
        if (previous->sample_time + previous->frames != side->chunks[i].sample_time) count += 1U;
    }
    return count;
}

typedef struct MRTrack {
    bool locked;
    int64_t offset;  // signal frame = recording frame - offset
    uint32_t map[MR_MAX_CHANNELS];
    uint64_t next_timing;
} MRTrack;

/// Tries to (re)lock at r. Against an existing lock, a new offset is a slip and a new
/// channel map a channel change; when neither changes, the frame itself is damaged.
static bool mr_relock(const MRProbe *probe, uint64_t r, MRTrack *track, MRAnalysis *analysis) {
    int64_t n = 0;
    if (!mr_locate(probe, r, &n)) return false;
    uint32_t map[MR_MAX_CHANNELS];
    mr_derive_map(probe, r, n, track->map, map);
    const size_t map_bytes = sizeof(uint32_t) * probe->channels;
    const int64_t predicted = (int64_t)r - track->offset;
    if (track->locked) {
        const bool moved = n != predicted;
        const bool remapped = memcmp(map, track->map, map_bytes) != 0;
        if (!moved && !remapped) return false;
        if (moved) {
            analysis->slips += 1U;
            analysis->slip_frames += n - predicted;
        }
        if (remapped) analysis->map_changes += 1U;
    } else {
        analysis->locks += 1U;
    }
    track->locked = true;
    track->offset = (int64_t)r - n;
    memcpy(track->map, map, map_bytes);
    track->next_timing = r;
    return true;
}

static bool mr_silent(const MRProbe *probe, uint64_t r) {
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        if (mr_frame(probe, r)[c] != 0.0F) return false;
    }
    return true;
}

/// Adds a matched frame to the coverage and samples its latency every timing step.
/// True when at least one recorded channel carries its mapped signal sample at (r, n), so
/// latency is measurable even while another channel is damaged.
static bool mr_any_channel_matches(const MRProbe *probe, uint64_t r, int64_t n,
                                   const uint32_t *map) {
    if (n < (int64_t)probe->lead || n >= (int64_t)probe->signal_frames) return false;
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        if (mr_frame(probe, r)[c] == mr_signal((uint64_t)n, map[c])) return true;
    }
    return false;
}

/// Samples the latency every timing step on frames where some channel is verified.
static void mr_maybe_time(const MRProbe *probe, uint64_t r, int64_t n, MRTrack *track,
                          double ns_per_tick, MRAnalysis *analysis) {
    if (r < track->next_timing || !mr_any_channel_matches(probe, r, n, track->map)) return;
    mr_time_lock(probe, r, n, ns_per_tick, analysis);
    track->next_timing = r + (uint64_t)(MR_TIMING_SECONDS * probe->rate);
}

static void mr_coverage(const MRProbe *probe, const uint32_t *seen, MRAnalysis *analysis) {
    int64_t first = -1, last = -1;
    for (uint64_t n = probe->lead; n < probe->signal_frames; ++n) {
        if (seen[n] == 0U) {
            analysis->missing += 1U;
            continue;
        }
        analysis->received += 1U;
        analysis->repeated += seen[n] - 1U;
        if (first < 0) first = (int64_t)n;
        last = (int64_t)n;
    }
    analysis->head_missing =
        first < 0 ? analysis->expected_frames : (uint64_t)first - probe->lead;
    analysis->tail_missing = last < 0 ? 0U : probe->signal_frames - 1U - (uint64_t)last;
}

static MRAnalysis mr_analyse(const MRProbe *probe, bool same_device) {
    MRAnalysis analysis = {.same_device = same_device, .lag_min = INFINITY, .lag_max = -INFINITY,
                           .ms_min = INFINITY, .ms_max = -INFINITY};
    analysis.recorded = atomic_load(&probe->recorded_frames);
    analysis.expected_frames = probe->signal_frames - probe->lead;
    analysis.out_discontinuities = mr_discontinuities(&probe->out);
    analysis.in_discontinuities = mr_discontinuities(&probe->in);
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    const double ns_per_tick = (double)timebase.numer / (double)timebase.denom;
    uint32_t *seen = calloc((size_t)probe->signal_frames, sizeof(uint32_t));
    if (seen == NULL) return analysis;
    MRTrack track = {0};
    for (uint32_t c = 0U; c < MR_MAX_CHANNELS; ++c) track.map[c] = c;
    for (uint64_t r = 0U; r < analysis.recorded;) {
        const int64_t n = (int64_t)r - track.offset;
        if (track.locked && mr_frame_matches(probe, r, n, track.map)) {
            if (n >= (int64_t)probe->lead && n < (int64_t)probe->signal_frames) seen[n] += 1U;
            mr_maybe_time(probe, r, n, &track, ns_per_tick, &analysis);
            r += 1U;
            continue;
        }
        // Silence before the first lock is the lead-in, not damage.
        const bool first = !track.locked;
        if ((track.locked || !mr_silent(probe, r)) && mr_relock(probe, r, &track, &analysis)) {
            // The first lock also classifies the frames before it: everything from the
            // signal's first frame is received or damaged, never left unexamined.
            // A recording that began after the signal did starts at its own first frame.
            const int64_t start = track.offset + (int64_t)probe->lead;
            const uint64_t from = start < 0 ? 0U : (uint64_t)start;
            if (first && from < r) r = from;
            continue;  // evaluate from here under the new lock
        }
        if (track.locked) {
            mr_count_damage(probe, r, n, track.map, &analysis);
            mr_maybe_time(probe, r, n, &track, ns_per_tick, &analysis);
        }
        r += 1U;
    }
    mr_coverage(probe, seen, &analysis);
    memcpy(analysis.map, track.map, sizeof(track.map));
    free(seen);
    return analysis;
}

// ---------------------------------------------------------------------------------------
// Impulse analysis. Per channel, an arrival is a rise from MR_IMPULSE_QUIET silent frames
// whose level stays above half its own peak for roughly a pulse width; its time is that
// half-peak crossing, interpolated between frames, so gain and clipping do not move it.
// The channel with the most arrivals is used. A played step crosses half its level half a
// frame before its first frame. Every arrival/pulse pair proposes a delay; the delay that
// aligns the most arrivals within two frames wins, and only distinct pulses count.

static uint32_t mr_incomplete(const MRProbe *probe);

typedef struct MRImpulses {
    uint64_t played, arrivals, matched, duplicates, rival;
    double ms_min, ms_max, ms_sum;
    double delay_frames, rival_delay_frames;
    // Diagnostic host-time coordinates, relative to the first timed output pulse.
    // These do not change the fixed-delay alignment or its acceptance thresholds.
    uint64_t traced_played, traced_arrivals;
    double played_frames[MR_IMPULSE_TRACE], arrival_frames[MR_IMPULSE_TRACE];
} MRImpulses;

static float mr_level(const MRProbe *probe, uint64_t r, uint32_t c) {
    const float value = fabsf(mr_frame(probe, r)[c]);
    return isfinite(value) ? value : 0.0F;
}

/// Arrival positions (fractional recording frames) on channel c; returns how many.
static uint64_t mr_arrivals(const MRProbe *probe, uint32_t c, double *positions,
                            uint64_t limit) {
    const uint64_t recorded = atomic_load(&probe->recorded_frames);
    uint64_t count = 0U, quiet = 0U;
    for (uint64_t r = 1U; r < recorded && count < limit; ++r) {
        if (mr_level(probe, r, c) < MR_IMPULSE_SILENCE) {
            quiet += 1U;
            continue;
        }
        const bool armed = quiet >= MR_IMPULSE_QUIET;
        quiet = 0U;
        if (!armed) continue;
        const uint64_t span = MR_IMPULSE_WIDTH + MR_IMPULSE_RISE;
        if (r + 2U * span >= recorded) break;
        float peak = 0.0F;
        for (uint64_t i = r; i < r + span; ++i) peak = fmaxf(peak, mr_level(probe, i, c));
        const float half = peak * 0.5F;
        uint64_t cross = r;
        while (cross < r + MR_IMPULSE_RISE && mr_level(probe, cross, c) < half) cross += 1U;
        uint64_t above = 0U, end = cross;
        while (end < r + 2U * span && mr_level(probe, end, c) >= half) {
            above += 1U;
            end += 1U;
        }
        // A pulse: clearly above silence, a sharp rise, and about one pulse width long.
        const bool pulse = half >= MR_IMPULSE_SILENCE && cross < r + MR_IMPULSE_RISE &&
                           above + 4U >= MR_IMPULSE_WIDTH && above <= MR_IMPULSE_WIDTH + 8U;
        if (pulse) {
            const float a = mr_level(probe, cross - 1U, c), b = mr_level(probe, cross, c);
            positions[count++] = (double)(cross - 1U) + (b > a ? (double)(half - a) / (double)(b - a) : 1.0);
        }
        // Skip this event; the next arrival needs fresh silence.
        while (r + 1U < recorded && mr_level(probe, r + 1U, c) >= MR_IMPULSE_SILENCE) r += 1U;
    }
    return count;
}

/// Host time of a fractional frame position on one side.
static bool mr_host_at(const MRSide *side, double position, double ticks_per_frame,
                       double *host) {
    double sample = 0.0;
    if (position < 0.0) return false;
    const uint64_t frame = (uint64_t)position;
    if (!mr_time(side, frame, ticks_per_frame, &sample, host)) return false;
    *host += (position - (double)frame) * ticks_per_frame;
    return true;
}

/// Index of the played time nearest `time` within `tolerance`, or -1.
static int64_t mr_nearest(const double *played, uint64_t count, double time, double tolerance) {
    uint64_t low = 0U, high = count;
    while (low < high) {
        const uint64_t middle = low + (high - low) / 2U;
        if (played[middle] < time) {
            low = middle + 1U;
        } else {
            high = middle;
        }
    }
    int64_t best = -1;
    double distance = tolerance;
    for (uint64_t i = low > 0U ? low - 1U : 0U; i < count && i <= low; ++i) {
        if (fabs(played[i] - time) <= distance) {
            distance = fabs(played[i] - time);
            best = (int64_t)i;
        }
    }
    return best;
}

/// Arrivals aligned to some played pulse when every arrival is shifted back by `delay`.
static uint64_t mr_aligned(const double *played, uint64_t count, const double *arrived,
                           uint64_t arrivals, double delay, double tolerance) {
    uint64_t aligned = 0U;
    for (uint64_t i = 0U; i < arrivals; ++i) {
        if (mr_nearest(played, count, arrived[i] - delay, tolerance) >= 0) aligned += 1U;
    }
    return aligned;
}

typedef struct MRAlignment {
    double delay, rival_delay;
    uint64_t matched, rival;
} MRAlignment;

static MRAlignment mr_align_impulses(const double *played, uint64_t timed,
                                     const double *arrived, uint64_t arrivals, double tolerance) {
    MRAlignment result = {0};
    for (uint64_t i = 0U; i < arrivals; ++i) {
        for (uint64_t j = 0U; j < timed; ++j) {
            const double candidate = arrived[i] - played[j];
            if (candidate < 0.0) continue;
            const uint64_t aligned = mr_aligned(played, timed, arrived, arrivals, candidate, tolerance);
            if (aligned > result.matched) {
                result.matched = aligned;
                result.delay = candidate;
            }
        }
    }
    // Find rivals against the final best delay. Two +/- tolerance windows must
    // be disjoint to represent different alignments; overlapping windows can
    // describe the same jittered pulse train, not a competing echo/path.
    for (uint64_t i = 0U; i < arrivals; ++i) {
        for (uint64_t j = 0U; j < timed; ++j) {
            const double candidate = arrived[i] - played[j];
            if (candidate < 0.0 || fabs(candidate - result.delay) <= 2.0 * tolerance) continue;
            const uint64_t aligned = mr_aligned(played, timed, arrived, arrivals, candidate, tolerance);
            if (aligned > result.rival) {
                result.rival = aligned;
                result.rival_delay = candidate;
            }
        }
    }
    return result;
}

static MRImpulses mr_analyse_impulses(const MRProbe *probe) {
    MRImpulses result = {.ms_min = INFINITY, .ms_max = -INFINITY};
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    const double ns_per_tick = (double)timebase.numer / (double)timebase.denom;
    const double ticks_per_frame = 1e9 / probe->rate / ns_per_tick;
    const double tolerance = 2.0 * ticks_per_frame;
    result.played = mr_impulse_count(probe);
    const uint64_t limit = result.played * 4U + 16U;
    double *played = calloc((size_t)result.played + 1U, sizeof(double));
    double *positions = calloc((size_t)limit, sizeof(double));
    double *arrived = calloc((size_t)limit, sizeof(double));
    bool *used = calloc((size_t)result.played + 1U, sizeof(bool));
    if (played == NULL || positions == NULL || arrived == NULL || used == NULL) goto done;
    uint64_t timed = 0U;
    for (uint64_t k = 0U; k < result.played; ++k) {
        if (mr_host_at(&probe->out, (double)mr_impulse_start(probe, k) - 0.5, ticks_per_frame,
                       &played[timed])) {
            timed += 1U;
        }
    }
    // The channel with the most arrivals; channels are never combined.
    uint64_t best_channel = 0U, best_count = 0U;
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        const uint64_t count = mr_arrivals(probe, c, positions, limit);
        if (count > best_count) {
            best_count = count;
            best_channel = c;
        }
    }
    const uint64_t found = mr_arrivals(probe, (uint32_t)best_channel, positions, limit);
    for (uint64_t i = 0U; i < found; ++i) {
        if (mr_host_at(&probe->in, positions[i], ticks_per_frame, &arrived[result.arrivals])) {
            result.arrivals += 1U;
        }
    }
    if (timed > 0U) {
        result.traced_played = timed < MR_IMPULSE_TRACE ? timed : MR_IMPULSE_TRACE;
        result.traced_arrivals = result.arrivals < MR_IMPULSE_TRACE ? result.arrivals : MR_IMPULSE_TRACE;
        for (uint64_t i = 0U; i < result.traced_played; ++i)
            result.played_frames[i] = (played[i] - played[0]) / ticks_per_frame;
        for (uint64_t i = 0U; i < result.traced_arrivals; ++i)
            result.arrival_frames[i] = (arrived[i] - played[0]) / ticks_per_frame;
    }
    const MRAlignment alignment = mr_align_impulses(played, timed, arrived, result.arrivals, tolerance);
    result.rival = alignment.rival;
    result.delay_frames = alignment.delay / ticks_per_frame;
    result.rival_delay_frames = alignment.rival_delay / ticks_per_frame;
    for (uint64_t i = 0U; i < result.arrivals && alignment.matched > 0U; ++i) {
        const int64_t j = mr_nearest(played, timed, arrived[i] - alignment.delay, tolerance);
        if (j < 0) continue;
        if (used[j]) {
            result.duplicates += 1U;
            continue;
        }
        used[j] = true;
        result.matched += 1U;
        const double ms = (arrived[i] - played[j]) * ns_per_tick / 1e6;
        result.ms_min = fmin(result.ms_min, ms);
        result.ms_max = fmax(result.ms_max, ms);
        result.ms_sum += ms;
    }
done:
    free(played);
    free(positions);
    free(arrived);
    free(used);
    return result;
}

/// Most played pulses arrived once each at one delay that clearly beats any other.
static bool mr_impulses_measured(const MRImpulses *impulses) {
    return impulses->matched > 0U && impulses->matched * 2U >= impulses->played &&
           impulses->rival * 2U < impulses->matched;
}

/// Exit status of an impulse measurement. A recording that overflowed, dropped chunks or
/// saw inconsistent buffers is never reported as measured, whatever its alignment.
static int mr_impulse_status(const MRImpulses *impulses, uint32_t incomplete) {
    if (incomplete != 0U) return 4;
    return mr_impulses_measured(impulses) ? 0 : 3;
}

static void mr_report_impulses(const MRProbe *probe, const MRImpulses *a, const MRReported *out,
                               const MRReported *in, bool json) {
    uint32_t out_total = 0U, in_total = 0U;
    const bool known = mr_reported_total(out, &out_total) && mr_reported_total(in, &in_total);
    const double ms_to_frames = probe->rate / 1000.0;
    const double mean = a->matched > 0U ? a->ms_sum / (double)a->matched : 0.0;
    if (json) {
        printf("\"mode\":\"impulse\",\"impulses\":%llu,\"arrivals\":%llu,\"matched\":%llu,"
               "\"duplicates\":%llu,\"rivalAlignment\":%llu,\"measured\":%s,",
               (unsigned long long)a->played, (unsigned long long)a->arrivals,
               (unsigned long long)a->matched, (unsigned long long)a->duplicates,
               (unsigned long long)a->rival, mr_impulses_measured(a) ? "true" : "false");
        printf("\"alignmentDelayFrames\":%.3f,\"rivalDelayFrames\":%.3f,",
               a->delay_frames, a->rival_delay_frames);
        printf("\"pulseTimeline\":{\"playedFrames\":[");
        for (uint64_t i = 0U; i < a->traced_played; ++i)
            printf("%s%.3f", i == 0U ? "" : ",", a->played_frames[i]);
        printf("],\"arrivalFrames\":[");
        for (uint64_t i = 0U; i < a->traced_arrivals; ++i)
            printf("%s%.3f", i == 0U ? "" : ",", a->arrival_frames[i]);
        printf("],\"complete\":%s},",
               a->traced_played == a->played && a->traced_arrivals == a->arrivals ? "true" : "false");
        if (a->matched > 0U) {
            printf("\"latencyMs\":{\"min\":%.3f,\"max\":%.3f,\"mean\":%.3f},"
                   "\"latencyFrames\":{\"min\":%.1f,\"max\":%.1f,\"mean\":%.2f},",
                   a->ms_min, a->ms_max, mean, a->ms_min * ms_to_frames,
                   a->ms_max * ms_to_frames, mean * ms_to_frames);
        }
        mr_json_known("reportedLatencyFrames", known, out_total + in_total);
        printf(",\"timestampDiscontinuities\":{\"output\":%u,\"input\":%u},\"complete\":%s}\n",
               mr_discontinuities(&probe->out), mr_discontinuities(&probe->in),
               mr_incomplete(probe) == 0U ? "true" : "false");
        return;
    }
    printf("impulses %llu played, %llu arrivals, %llu matched once, %llu duplicates, "
           "best rival delay aligns %llu\n",
           (unsigned long long)a->played, (unsigned long long)a->arrivals,
           (unsigned long long)a->matched, (unsigned long long)a->duplicates,
           (unsigned long long)a->rival);
    if (a->matched > 0U) {
        printf("latency %.3f..%.3f ms (mean %.3f, %.1f frames) by host time%s\n", a->ms_min,
               a->ms_max, mean, mean * ms_to_frames,
               mr_impulses_measured(a) ? "" : " (not conclusive)");
    }
    if (known) {
        printf("devices report %u frames (%.3f ms)\n", out_total + in_total,
               (double)(out_total + in_total) * 1000.0 / probe->rate);
    } else {
        printf("devices do not report their full latency\n");
    }
    if (mr_incomplete(probe) != 0U) {
        printf("INCOMPLETE: the recording overflowed or lost audio; not a measurement\n");
    }
}

// ---------------------------------------------------------------------------------------
// Report

static bool mr_identity(const MRProbe *probe, const MRAnalysis *analysis) {
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        if (analysis->map[c] != c) return false;
    }
    return true;
}

static uint32_t mr_incomplete(const MRProbe *probe) {
    return atomic_load(&probe->overflow) + atomic_load(&probe->out.dropped_chunks) +
           atomic_load(&probe->in.dropped_chunks) + atomic_load(&probe->out.inconsistent_buffers) +
           atomic_load(&probe->in.inconsistent_buffers);
}

static void mr_report_json(const MRProbe *probe, const MRAnalysis *a, const MRReported *out,
                           const MRReported *in) {
    uint32_t out_total = 0U, in_total = 0U;
    const bool out_known = mr_reported_total(out, &out_total);
    const bool in_known = mr_reported_total(in, &in_total);
    printf("\"sameDevice\":%s,\"recordedFrames\":%llu,\"signalFrames\":%llu,"
           "\"receivedFrames\":%llu,\"missingFrames\":%llu,\"headMissing\":%llu,"
           "\"tailMissing\":%llu,\"repeatedFrames\":%llu,\"corruptFrames\":%llu,"
           "\"zeroSamples\":%llu,\"wrongSamples\":%llu,\"nonfiniteSamples\":%llu,",
           a->same_device ? "true" : "false", (unsigned long long)a->recorded,
           (unsigned long long)a->expected_frames, (unsigned long long)a->received,
           (unsigned long long)a->missing, (unsigned long long)a->head_missing,
           (unsigned long long)a->tail_missing, (unsigned long long)a->repeated,
           (unsigned long long)a->corrupt_frames, (unsigned long long)a->zero_samples,
           (unsigned long long)a->wrong_samples, (unsigned long long)a->nonfinite_samples);
    printf("\"maxError\":%.9g,\"locks\":%llu,\"slips\":%llu,\"slipFrames\":%lld,"
           "\"channelChanges\":%llu,\"channelOrder\":%s,\"timedPoints\":%llu,",
           a->max_error, (unsigned long long)a->locks, (unsigned long long)a->slips,
           (long long)a->slip_frames, (unsigned long long)a->map_changes,
           mr_identity(probe, a) ? "true" : "false", (unsigned long long)a->timed);
    printf("\"captureIntegrity\":{\"recordingOverflow\":%u,\"outputDroppedChunks\":%u,"
           "\"inputDroppedChunks\":%u,\"outputInconsistentBuffers\":%u,\"inputInconsistentBuffers\":%u},",
           atomic_load(&probe->overflow), atomic_load(&probe->out.dropped_chunks),
           atomic_load(&probe->in.dropped_chunks), atomic_load(&probe->out.inconsistent_buffers),
           atomic_load(&probe->in.inconsistent_buffers));
    if (a->timed > 0U) {
        printf("\"latencyMs\":{\"min\":%.3f,\"max\":%.3f,\"mean\":%.3f},", a->ms_min, a->ms_max,
               a->ms_sum / (double)a->timed);
        if (a->same_device) {
            printf("\"latencyFrames\":{\"min\":%.0f,\"max\":%.0f,\"mean\":%.2f},", a->lag_min,
                   a->lag_max, a->lag_sum / (double)a->timed);
        }
    }
    mr_json_known("reportedLatencyFrames", out_known && in_known, out_total + in_total);
    printf(",\"timestampDiscontinuities\":{\"output\":%u,\"input\":%u},\"complete\":%s}\n",
           a->out_discontinuities, a->in_discontinuities,
           mr_incomplete(probe) == 0U ? "true" : "false");
}

static void mr_report_text(const MRProbe *probe, const MRAnalysis *a, const MRReported *out,
                           const MRReported *in) {
    printf("signal %llu frames: received %llu, missing %llu (head %llu, tail %llu), "
           "repeated %llu\n",
           (unsigned long long)a->expected_frames, (unsigned long long)a->received,
           (unsigned long long)a->missing, (unsigned long long)a->head_missing,
           (unsigned long long)a->tail_missing, (unsigned long long)a->repeated);
    printf("damage: %llu frames; samples zero %llu, wrong %llu (max error %.3g), "
           "non-finite %llu\n",
           (unsigned long long)a->corrupt_frames, (unsigned long long)a->zero_samples,
           (unsigned long long)a->wrong_samples, a->max_error,
           (unsigned long long)a->nonfinite_samples);
    printf("slips %llu (net %+lld frames), channel order %s, channel changes %llu\n",
           (unsigned long long)a->slips, (long long)a->slip_frames,
           mr_identity(probe, a) ? "preserved" : "CHANGED", (unsigned long long)a->map_changes);
    for (uint32_t c = 0U; c < probe->channels && !mr_identity(probe, a); ++c) {
        if (a->map[c] == MR_UNMAPPED) {
            printf("  input %u carries no output channel exactly\n", c + 1U);
        } else if (a->map[c] != c) {
            printf("  input %u carries output %u\n", c + 1U, a->map[c] + 1U);
        }
    }
    if (a->timed > 0U) {
        printf("latency %.3f..%.3f ms (mean %.3f) by host time", a->ms_min, a->ms_max,
               a->ms_sum / (double)a->timed);
        if (a->same_device) printf(", %.0f..%.0f frames by device time", a->lag_min, a->lag_max);
        printf("\n");
    }
    uint32_t out_total = 0U, in_total = 0U;
    const bool out_known = mr_reported_total(out, &out_total);
    const bool in_known = mr_reported_total(in, &in_total);
    if (out_known && in_known) {
        printf("devices report %u frames (%.3f ms)\n", out_total + in_total,
               (double)(out_total + in_total) * 1000.0 / probe->rate);
    } else {
        printf("devices do not report their full latency\n");
    }
    printf("timestamp discontinuities: output %u, input %u\n", a->out_discontinuities,
           a->in_discontinuities);
    if (mr_incomplete(probe) != 0U) {
        printf("INCOMPLETE: recording overflow %u, dropped chunks %u, inconsistent buffers %u\n",
               atomic_load(&probe->overflow),
               atomic_load(&probe->out.dropped_chunks) + atomic_load(&probe->in.dropped_chunks),
               atomic_load(&probe->out.inconsistent_buffers) +
                   atomic_load(&probe->in.inconsistent_buffers));
    }
}

// ---------------------------------------------------------------------------------------
// Set-up and self test

// Called only after callback drain; retain every received frame, including damage.
static bool mr_write_capture(FILE *file, const MRProbe *probe) {
    const uint64_t frames = atomic_load(&probe->recorded_frames);
    if (frames > probe->capacity || frames > SIZE_MAX / sizeof(float) / probe->channels) return false;
    const size_t samples = (size_t)frames * probe->channels;
    return fwrite(probe->recorded, sizeof(float), samples, file) == samples && fflush(file) == 0;
}

static bool mr_allocate(MRProbe *probe, double seconds) {
    probe->lead = (uint64_t)(MR_LEAD_SECONDS * probe->rate);
    probe->signal_frames = probe->lead + (uint64_t)(seconds * probe->rate);
    probe->run_frames = probe->signal_frames + (uint64_t)(MR_TAIL_SECONDS * probe->rate);
    probe->capacity = probe->run_frames + MR_HEADROOM;
    // At most one chunk per 16 frames, the smallest buffer CoreAudio offers.
    const uint64_t chunks = probe->capacity / 16U + 1024U;
    probe->out.capacity = probe->in.capacity = (uint32_t)chunks;
    probe->recorded = calloc((size_t)(probe->capacity * probe->channels), sizeof(float));
    probe->out.chunks = calloc((size_t)chunks, sizeof(MRChunk));
    probe->in.chunks = calloc((size_t)chunks, sizeof(MRChunk));
    atomic_store(&probe->origin, NAN);
    return probe->recorded != NULL && probe->out.chunks != NULL && probe->in.chunks != NULL;
}

/// Timestamps for a synthetic recording of `run_frames`: 512-frame cycles on both sides;
/// the input's host time trails by `host_extra` frames.
static void mr_synthesize(MRProbe *probe, uint64_t host_extra) {
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    const double ticks_per_frame =
        1e9 / probe->rate / ((double)timebase.numer / (double)timebase.denom);
    const uint32_t chunks = (uint32_t)(probe->run_frames / 512U);
    const uint32_t flags = kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid |
                           kAudioTimeStampRateScalarValid;
    for (uint32_t i = 0U; i < chunks; ++i) {
        const uint64_t first = (uint64_t)i * 512U;
        probe->out.chunks[i] = (MRChunk){(double)first, 1.0,
                                         (uint64_t)((double)first * ticks_per_frame), first,
                                         flags, 512U, 0U, 0U};
        probe->in.chunks[i] =
            (MRChunk){(double)first, 1.0,
                      (uint64_t)((double)(first + host_extra) * ticks_per_frame), first, flags,
                      512U, 0U, 0U};
    }
    atomic_store(&probe->out.count, chunks);
    atomic_store(&probe->in.count, chunks);
    atomic_store(&probe->recorded_frames, (uint64_t)chunks * 512U);
}

static void mr_put(MRProbe *probe, uint64_t r, int64_t n, const uint32_t *route) {
    for (uint32_t c = 0U; c < probe->channels; ++c) {
        probe->recorded[r * probe->channels + c] = mr_expected(probe, n, route[c]);
    }
}

static float *mr_sample(MRProbe *probe, uint64_t r, uint32_t c) {
    return &probe->recorded[r * probe->channels + c];
}

static bool mr_expect(bool condition, const char *what) {
    if (!condition) printf("self test: %s FAILED\n", what);
    return condition;
}

static MRProbe *mr_synthetic(uint32_t channels, double seconds) {
    MRProbe *probe = calloc(1U, sizeof(MRProbe));
    if (probe == NULL) return NULL;
    probe->rate = 48000.0;
    probe->channels = channels;
    return mr_allocate(probe, seconds) ? probe : NULL;
}

static MRAnalysis mr_synthetic_analysis(MRProbe *probe, uint64_t host_extra, bool same) {
    mr_synthesize(probe, host_extra);
    const MRAnalysis analysis = mr_analyse(probe, same);
    const MRReported none = {0};
    mr_report_text(probe, &analysis, &none, &none);
    return analysis;
}

/// 300 frames late (+48 frames of host time), the first 100 signal frames lost, a 2-frame
/// skip at 2 s (lag 298), 100 zero frames at 3 s, a 1-frame repeat at 3.5 s (lag 299),
/// and one zero sample on channel 2 at 3.7 s.
static bool mr_self_test_damage(void) {
    MRProbe *probe = mr_synthetic(2U, 4.0);
    if (probe == NULL) return false;
    const uint32_t route[2] = {0U, 1U};
    int64_t n = -300;
    for (uint64_t r = 0U; r < probe->run_frames; ++r, ++n) {
        if (r == 96000U) n += 2;
        if (r == 168000U) n -= 1;
        mr_put(probe, r, n, route);
        const bool head = n >= (int64_t)probe->lead && n < (int64_t)probe->lead + 100;
        const bool dropout = r >= 144000U && r < 144100U;
        if (head || dropout) *mr_sample(probe, r, 0U) = *mr_sample(probe, r, 1U) = 0.0F;
    }
    *mr_sample(probe, 177600U, 1U) = 0.0F;
    const MRAnalysis a = mr_synthetic_analysis(probe, 48U, true);
    bool ok = mr_expect(a.head_missing == 100U, "lost prefix");
    ok &= mr_expect(a.missing == 100U + 2U + 100U + 1U, "missing frames");
    ok &= mr_expect(a.repeated == 1U, "repeated frame");
    ok &= mr_expect(a.slips == 2U && a.slip_frames == 1, "slips");
    // The lost prefix, the dropout and one zero sample, all as zeros where signal was due.
    ok &= mr_expect(a.corrupt_frames == 201U && a.zero_samples == 401U && a.wrong_samples == 0U,
                    "lost prefix, dropout and one zero sample are the only damage");
    ok &= mr_expect(a.lag_min == 298.0 && a.lag_max == 300.0, "device-time latency");
    ok &= mr_expect(fabs(a.ms_max - (300.0 + 48.0) / 48.0) < 0.001, "host-time latency");
    ok &= mr_expect(mr_identity(probe, &a) && a.map_changes == 0U, "channel order");
    return ok;
}

/// Three channels: 1 and 2 swapped from the start, a 100-frame dropout at 1 s, rotated
/// again right after it, and channel 2 replaced by the mean of 1 and 3 from 2.5 s.
static bool mr_self_test_channels(void) {
    MRProbe *probe = mr_synthetic(3U, 3.0);
    if (probe == NULL) return false;
    const uint32_t first_route[3] = {1U, 0U, 2U}, second_route[3] = {1U, 2U, 0U};
    for (uint64_t r = 0U; r < probe->run_frames; ++r) {
        mr_put(probe, r, (int64_t)r - 64, r < 48100U ? first_route : second_route);
        if (r >= 48000U && r < 48100U) {
            for (uint32_t c = 0U; c < 3U; ++c) *mr_sample(probe, r, c) = 0.0F;
        }
        if (r >= 120000U && r < probe->signal_frames + 64U) {
            *mr_sample(probe, r, 1U) =
                (*mr_sample(probe, r, 0U) + *mr_sample(probe, r, 2U)) * 0.5F;
        }
    }
    const MRAnalysis a = mr_synthetic_analysis(probe, 0U, false);
    bool ok = mr_expect(a.missing == 100U + (probe->signal_frames - 120000U + 64U),
                        "only the dropout and the mixed span are missing");
    ok &= mr_expect(a.map_changes == 1U && a.slips == 0U, "one channel change, no slip");
    ok &= mr_expect(a.map[0] == 1U && a.map[1] == 2U && a.map[2] == 0U, "final channel map");
    ok &= mr_expect(a.wrong_samples >= probe->signal_frames - 120000U - 64U,
                    "a mix of channels is damage");
    return ok;
}

/// Channel 1 stuck at one valid signal value while channel 2 is exact: the healthy channel
/// still locates the signal, and the stuck one is damage on every frame.
static bool mr_self_test_anchor(void) {
    MRProbe *probe = mr_synthetic(2U, 1.0);
    if (probe == NULL) return false;
    const uint32_t route[2] = {0U, 1U};
    const float stuck = mr_signal(probe->lead + 7U, 0U);
    for (uint64_t r = 0U; r < probe->run_frames; ++r) {
        mr_put(probe, r, (int64_t)r - 10, route);
        if (r >= probe->lead + 10U) *mr_sample(probe, r, 0U) = stuck;
    }
    const MRAnalysis a = mr_synthetic_analysis(probe, 0U, true);
    bool ok = mr_expect(a.locks == 1U && a.slips == 0U, "locks on the healthy channel");
    ok &= mr_expect(a.wrong_samples + 400U > a.expected_frames && a.map[1] == 1U,
                    "the stuck channel is damage");
    return ok;
}

/// One zeroed sample before the first intact window is one damaged frame, not a lost
/// prefix; and a channel that stays silent still lets the other one time the latency.
static bool mr_self_test_early_damage(void) {
    MRProbe *probe = mr_synthetic(2U, 1.0);
    MRProbe *silent = mr_synthetic(2U, 1.0);
    if (probe == NULL || silent == NULL) return false;
    const uint32_t route[2] = {0U, 1U};
    for (uint64_t r = 0U; r < probe->run_frames; ++r) {
        mr_put(probe, r, (int64_t)r - 10, route);
        mr_put(silent, r, (int64_t)r - 10, route);
        *mr_sample(silent, r, 0U) = 0.0F;
    }
    *mr_sample(probe, probe->lead + 10U + 16U, 1U) = 0.0F;
    const MRAnalysis a = mr_synthetic_analysis(probe, 0U, true);
    bool ok = mr_expect(a.head_missing == 0U && a.missing == 1U && a.corrupt_frames == 1U,
                        "early damage is one damaged frame");
    const MRAnalysis b = mr_synthetic_analysis(silent, 0U, true);
    // Recording began 10 frames into the signal; recording frame 16 is zeroed.
    MRProbe *late = mr_synthetic(1U, 1.0);
    if (late == NULL) return false;
    for (uint64_t r = 0U; r < late->run_frames; ++r) {
        mr_put(late, r, (int64_t)(r + late->lead + 10U), route);
    }
    *mr_sample(late, 16U, 0U) = 0.0F;
    const MRAnalysis c = mr_synthetic_analysis(late, 0U, true);
    ok &= mr_expect(c.missing == 11U && c.corrupt_frames == 1U && c.head_missing == 10U,
                    "a late recording is examined from its first frame");
    ok &= mr_expect(b.timed > 0U && b.lag_min == 10.0 && b.lag_max == 10.0,
                    "latency through the healthy channel");
    return ok;
}

/// A 32-frame block repeated 300 times: every repeat counts.
static bool mr_self_test_repeats(void) {
    MRProbe *probe = mr_synthetic(2U, 1.0);
    if (probe == NULL) return false;
    const uint32_t route[2] = {0U, 1U};
    int64_t n = -10;
    for (uint64_t r = 0U; r < probe->run_frames; ++r, ++n) {
        if (r >= 20000U && r < 20000U + 300U * 32U) {
            mr_put(probe, r, 19990 + (int64_t)((r - 20000U) % 32U), route);
            if (r == 20000U + 300U * 32U - 1U) n = 19990 + 31;
            continue;
        }
        mr_put(probe, r, n, route);
    }
    const MRAnalysis a = mr_synthetic_analysis(probe, 0U, true);
    bool ok = mr_expect(a.repeated == 299U * 32U && a.missing == 0U, "every repeat counted");
    ok &= mr_expect(a.slips == 299U && a.slip_frames == -299 * 32, "each repeat is a slip back");
    return ok;
}

static bool mr_self_test_signal(void) {
    for (uint32_t frame = 0U; frame < 100000U; ++frame) {
        for (uint32_t c = 0U; c < MR_MAX_CHANNELS; ++c) {
            const float value = mr_signal(frame, c);
            if (value == 0.0F || mr_channel_of(value) != c || fabsf(value) > 0.25F) {
                return mr_expect(false, "signal values");
            }
        }
    }
    float block[8];
    for (uint32_t i = 0U; i < 8U; ++i) block[i] = mr_signal(1000003U + i, 5U);
    bool decoded = true;
    for (uint32_t phase = 0U; phase < 4U; ++phase) {
        int64_t frame = 0;
        const bool aligned = (1000003U + phase) % 4U == 0U;
        decoded &= mr_decode(block, 1U, 5U, phase, &frame) && (!aligned || frame == 1000003);
    }
    return mr_expect(decoded, "frame decoding") &&
           mr_expect(mr_channel_of(0.0F) == MR_UNMAPPED && mr_channel_of(0.3F) == MR_UNMAPPED &&
                         mr_channel_of(1.0F / 65536.0F) == MR_UNMAPPED,
                     "non-signal values");
}

/// A synthetic impulse recording: pulses `delay` frames late (plus 48 frames of host time)
/// at `gain`, clipped to 1, each edge smoothed over `smooth` frames, on channel 0 and on
/// channel 1 `skew` frames later, with low noise.
static MRProbe *mr_impulse_recording(double seconds, uint64_t delay, float gain, uint32_t smooth,
                                     int64_t skew) {
    MRProbe *probe = mr_synthetic(2U, seconds);
    if (probe == NULL) return NULL;
    probe->impulse = true;
    for (uint64_t r = 0U; r < probe->run_frames; ++r) {
        for (uint32_t c = 0U; c < 2U; ++c) {
            const int64_t n = (int64_t)r - (int64_t)delay - (c == 1U ? skew : 0);
            float value = 0.0F;
            // A moving average over `smooth` frames rounds both edges symmetrically.
            const uint32_t taps = smooth == 0U ? 1U : smooth;
            for (uint32_t t = 0U; t < taps; ++t) value += mr_expected(probe, n - (int64_t)t, 0U);
            value = fminf(value / (float)taps * gain, 1.0F);
            *mr_sample(probe, r, c) = value + (r % 7U == 0U ? 0.01F : 0.0F);
        }
    }
    mr_synthesize(probe, 48U);
    return probe;
}

static MRImpulses mr_impulse_analysis(const MRProbe *probe) {
    const MRImpulses a = mr_analyse_impulses(probe);
    const MRReported none = {0};
    mr_report_impulses(probe, &a, &none, &none, false);
    return a;
}

static bool mr_impulse_expect(const MRImpulses *a, double ms, const char *what) {
    return mr_expect(mr_impulses_measured(a) && a->matched == a->played &&
                         fabs(a->ms_min - ms) < 0.1 && fabs(a->ms_max - ms) < 0.1,
                     what);
}

static bool mr_self_test_impulse_jitter(void) {
    const double played[] = {0, 100, 207, 311, 419, 526, 638, 751, 869, 990,
                             1114, 1241, 1371, 1504, 1640, 1779, 1921, 2066, 2214, 2365};
    double arrived[20];
    const double jitter[] = {0.0, 0.9, 2.1, 3.7};
    for (uint32_t i = 0U; i < 20U; ++i) arrived[i] = played[i] + 300.0 + jitter[i % 4U];
    const MRAlignment window = mr_align_impulses(played, 20U, arrived, 20U, 2.0);
    bool ok = mr_expect(window.matched == 15U && window.rival * 2U < window.matched,
                        "overlapping pulse windows are not independent rivals");
    MRProbe *probe = mr_impulse_recording(5.0, 300U, 1.0F, 0U, 0);
    if (probe == NULL) return false;
    mach_timebase_info_data_t timebase;
    mach_timebase_info(&timebase);
    const double ticks_per_frame =
        1e9 / probe->rate / ((double)timebase.numer / (double)timebase.denom);
    const uint32_t count = atomic_load(&probe->in.count);
    for (uint32_t i = 0U; i < count; ++i) {
        probe->in.chunks[i].host_time += (uint64_t)(jitter[i % 4U] * ticks_per_frame);
    }
    const MRImpulses a = mr_impulse_analysis(probe);
    ok &= mr_expect(mr_impulses_measured(&a) && a.rival * 2U < a.matched,
                        "overlapping jitter windows are one impulse alignment");
    mr_synthesize(probe, 48U);
    for (uint32_t i = 0U; i < count; ++i)
        probe->in.chunks[i].host_time += (uint64_t)((double)i * 0.05 * ticks_per_frame);
    const MRImpulses drift = mr_impulse_analysis(probe);
    ok &= mr_expect(!mr_impulses_measured(&drift) && drift.traced_played == drift.played &&
                        drift.traced_arrivals == drift.arrivals && drift.traced_arrivals > 1U,
                    "changing content delay retains every event without passing a fixed delay");
    if (drift.traced_arrivals > 1U && drift.traced_arrivals == drift.traced_played) {
        const uint64_t last = drift.traced_arrivals - 1U;
        const double first_delay = drift.arrival_frames[0] - drift.played_frames[0];
        const double last_delay = drift.arrival_frames[last] - drift.played_frames[last];
        ok &= mr_expect(last_delay - first_delay > 15.0,
                        "diagnostic pulse timeline preserves drift outside the winning window");
    }
    MRProbe *echo = mr_impulse_recording(5.0, 300U, 1.0F, 0U, 0);
    if (echo == NULL) return false;
    for (uint64_t r = 6300U; r < echo->run_frames; ++r) {
        const float copy = mr_expected(echo, (int64_t)r - 6300, 0U);
        for (uint32_t c = 0U; c < 2U; ++c) *mr_sample(echo, r, c) += copy;
    }
    const MRImpulses rival = mr_impulse_analysis(echo);
    ok &= mr_expect(!mr_impulses_measured(&rival) && rival.rival * 2U >= rival.matched,
                    "a genuinely separate echo remains ambiguous");
    return ok;
}

/// Exact, smoothed, amplified and clipped pulses; a delay longer than the spacing; skewed
/// channels; a cut final pulse; repeated copies and noise that must not measure.
static bool mr_self_test_impulses(void) {
    bool ok = true;
    // 300 frames of signal delay plus 48 of host time is 7.25 ms at 48 kHz; a moving
    // average over s frames moves the half-peak crossing by (s - 1) / 2 frames.
    MRProbe *exact = mr_impulse_recording(2.0, 300U, 1.0F, 0U, 0);
    MRProbe *smooth = mr_impulse_recording(2.0, 300U, 1.0F, 5U, 0);
    MRProbe *clipped = mr_impulse_recording(2.0, 300U, 3.0F, 5U, 0);
    MRProbe *late = mr_impulse_recording(2.0, 14400U, 1.0F, 5U, 0);
    MRProbe *skewed = mr_impulse_recording(2.0, 300U, 1.0F, 0U, 8);
    MRProbe *cut = mr_impulse_recording(1.001, 300U, 1.0F, 0U, 0);
    if (!exact || !smooth || !clipped || !late || !skewed || !cut) return false;
    MRImpulses a = mr_impulse_analysis(exact);
    ok &= mr_impulse_expect(&a, 7.25, "impulse latency");
    ok &= mr_expect(a.traced_played == a.played && a.traced_arrivals == a.arrivals &&
                        a.played_frames[0] == 0.0 && fabs(a.arrival_frames[0] - 348.0) < 0.05,
                    "diagnostic pulse timeline uses the common host-time origin");
    a = mr_impulse_analysis(smooth);
    ok &= mr_impulse_expect(&a, 7.25 + 2.0 / 48.0, "smoothed impulse latency");
    a = mr_impulse_analysis(clipped);
    // Three times the gain clips at 1; the half-peak crossing moves less than a frame.
    ok &= mr_expect(mr_impulses_measured(&a) && a.matched == a.played &&
                        fabs(a.ms_min - 7.25) < 0.05 && fabs(a.ms_max - 7.29) < 0.05,
                    "amplified, clipped impulse latency");
    a = mr_impulse_analysis(late);
    ok &= mr_impulse_expect(&a, 301.0 + 2.0 / 48.0, "latency beyond the pulse spacing");
    a = mr_impulse_analysis(skewed);
    ok &= mr_impulse_expect(&a, 7.25, "skewed channels are measured one at a time");
    a = mr_impulse_analysis(cut);
    ok &= mr_impulse_expect(&a, 7.25, "a cut final pulse is not played");
    // Ten copies of the first pulse only, then silence: one distinct pulse arrived.
    MRProbe *repeated = mr_impulse_recording(5.0, 300U, 1.0F, 0U, 0);
    if (repeated == NULL) return false;
    const uint64_t first = mr_impulse_start(repeated, 0U) + 300U;
    for (uint64_t r = first + MR_IMPULSE_WIDTH; r < repeated->run_frames; ++r) {
        const uint64_t phase = (r - first) % 6000U;
        for (uint32_t c = 0U; c < 2U; ++c) {
            *mr_sample(repeated, r, c) = r < first + 10U * 6000U && phase < MR_IMPULSE_WIDTH
                                             ? MR_IMPULSE_LEVEL
                                             : 0.0F;
        }
    }
    a = mr_impulse_analysis(repeated);
    ok &= mr_expect(!mr_impulses_measured(&a) && a.matched == 1U, "repeated copies do not measure");
    // No pulse returns: a lone spike, a slow swell and steady background are not pulses.
    MRProbe *noise = mr_impulse_recording(2.0, 0U, 0.0F, 0U, 0);
    if (noise == NULL) return false;
    *mr_sample(noise, 20000U, 1U) = 0.3F;
    for (uint64_t r = 40000U; r < 41000U; ++r) {
        *mr_sample(noise, r, 0U) = 0.5F * (float)(r - 40000U) / 1000.0F;
    }
    for (uint64_t r = 60000U; r < 70000U; ++r) *mr_sample(noise, r, 0U) = 0.4F;
    a = mr_impulse_analysis(noise);
    ok &= mr_expect(a.arrivals == 0U && !mr_impulses_measured(&a), "noise is not a pulse");
    // A measured recording that overflowed is not a measurement; neither is an unmatched one.
    a = mr_impulse_analysis(exact);
    ok &= mr_expect(mr_impulse_status(&a, mr_incomplete(exact)) == 0, "complete pulses exit 0");
    atomic_store(&exact->overflow, 1U);
    ok &= mr_expect(mr_impulse_status(&a, mr_incomplete(exact)) == 4,
                    "an incomplete recording never exits 0");
    a = mr_impulse_analysis(noise);
    ok &= mr_expect(mr_impulse_status(&a, mr_incomplete(noise)) == 3, "no aligned pulses exit 3");
    return ok;
}

static bool mr_self_test_capture_horizon(void) {
    float captured[24] = {0};
    const float pcm[8] = {0.25F, -0.25F, 0.5F, -0.5F, 0.75F, -0.75F, 1.0F, -1.0F};
    MRChunk chunks[4] = {0};
    MRProbe probe = {.channels = 2U, .capacity = 12U, .run_frames = 6U,
                     .recorded = captured};
    probe.in.chunks = chunks;
    probe.in.capacity = 4U;
    atomic_store(&probe.origin, 0.0);
    const AudioBufferList input = {.mNumberBuffers = 1U,
        .mBuffers = {{.mNumberChannels = 2U, .mDataByteSize = sizeof(pcm), .mData = (void *)pcm}}};
    const AudioTimeStamp time = {.mFlags = kAudioTimeStampSampleTimeValid};
    // Control-side scheduling can delay Stop after the requested capture horizon.
    // Retain the complete final callback, then stop recording on the audio owner.
    for (unsigned callback = 0U; callback < 20U; ++callback) mr_record(&probe, &input, &time);
    bool ok = mr_expect(atomic_load(&probe.recorded_frames) == 8U &&
        atomic_load(&probe.in.count) == 2U && mr_incomplete(&probe) == 0U,
        "late control stop cannot overflow an already complete capture");
    ok &= mr_expect(memcmp(captured, pcm, sizeof(pcm)) == 0 &&
        memcmp(captured + 8U, pcm, sizeof(pcm)) == 0,
        "the complete final callback remains in the capture");
    probe.run_frames = 32U;
    mr_record(&probe, &input, &time);
    mr_record(&probe, &input, &time);
    ok &= mr_expect(atomic_load(&probe.overflow) != 0U,
        "overflow before the requested horizon still invalidates the measurement");
    return ok;
}

static int mr_self_test(void) {
    const bool impulses = mr_self_test_impulses();
    const bool jitter = mr_self_test_impulse_jitter();
    const bool damage = mr_self_test_damage();
    const bool channels = mr_self_test_channels();
    const bool anchor = mr_self_test_anchor();
    const bool repeats = mr_self_test_repeats();
    const bool early = mr_self_test_early_damage();
    const bool signal = mr_self_test_signal();
    float captured[] = {0.25F, -0.125F, 0.0F, 0.0F, -0.5F, 0.5F};
    MRProbe recording = {.channels = 2U, .capacity = 3U, .recorded = captured};
    atomic_store(&recording.recorded_frames, 3U);
    FILE *file = tmpfile();
    float recovered[6] = {0};
    bool capture = file != NULL && mr_write_capture(file, &recording);
    if (file != NULL) {
        rewind(file);
        capture = capture && fread(recovered, sizeof(float), 6U, file) == 6U &&
            memcmp(captured, recovered, sizeof(captured)) == 0 && fgetc(file) == EOF;
        capture = fclose(file) == 0 && capture;
    }
    capture = mr_expect(capture, "raw capture preserves all frames and damaged silence");
    const bool horizon = mr_self_test_capture_horizon();
    const bool passed = impulses && jitter && damage && channels && anchor && repeats && early && signal && capture && horizon;
    printf("self test %s\n", passed ? "passed" : "FAILED");
    return passed ? 0 : 1;
}

// ---------------------------------------------------------------------------------------
// Command line

static void mr_usage(FILE *stream) {
    fprintf(stream,
            "usage: MRRoundTripProbe --list | --self-test\n"
            "       MRRoundTripProbe --authorize-input --json (consent only; no device I/O)\n"
            "       MRRoundTripProbe --transport DEVICE_UID\n"
            "       MRRoundTripProbe --latency DEVICE_UID (read-only device/stream/safety)\n"
            "       MRRoundTripProbe [--json] [--impulse] [--timing] [--seconds 1-120] [--channels 1-64] "
            "OUTPUT_UID [INPUT_UID]\n"
            "Plays a deterministic signal into OUTPUT_UID and records INPUT_UID (default: the\n"
            "same device). Reports latency (host time; also device frames on one device),\n"
            "received, missing and repeated signal frames, damaged samples, slips, channel\n"
            "order and the latency the devices report. Changes no default device or setting.\n"
            "--timing writes the retained callback timeline to stderr after IO stops.\n"
            "--hal-trace-file PATH enables MixRelay HAL evidence for this measurement, saves it\n"
            "after IO stops, then disables it; other device settings stay unchanged.\n"
            "--capture-file PATH saves all captured interleaved native Float32 frames after IO stops.\n"
            "--impulse measures paths that change samples (asynchronous clocks, sample-rate\n"
            "conversion): it plays pulses at irregular ~0.25 s spacing and finds the one delay\n"
            "that aligns their arrivals.\n"
            "Exit status: 0 measured, 1 device error, 2 usage or recording permission error,\n"
            "3 no exact copy of the signal\n"
            "(with --impulse: no single delay aligned most pulses), 4 (--impulse) the\n"
            "recording overflowed, dropped chunks or saw inconsistent buffers.\n");
}

static bool mr_parse_number(const char *text, double low, double high, double *value) {
    char *end = NULL;
    errno = 0;
    const double parsed = strtod(text, &end);
    if (errno != 0 || end == text || *end != '\0' || !(parsed >= low && parsed <= high)) {
        return false;
    }
    *value = parsed;
    return true;
}

typedef struct MROptions {
    bool json, impulse, timing;
    double seconds, channels;
    const char *uids[2];
    const char *capture_file;
    const char *hal_trace_file;
    int found;
} MROptions;

/// Parses the measurement options; returns -1 to continue, otherwise an exit status.
static int mr_options(int argc, char **argv, MROptions *options) {
    if (argc == 3 && strcmp(argv[1], "--latency") == 0) return mr_latency(argv[2]);
    if (argc == 3 && strcmp(argv[1], "--transport") == 0) return mr_transport(argv[2]);
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--list") == 0) return mr_list();
        if (strcmp(argv[i], "--self-test") == 0) return mr_self_test();
        if (strcmp(argv[i], "--help") == 0) {
            mr_usage(stdout);
            return 0;
        }
        bool ok = true;
        if (strcmp(argv[i], "--json") == 0) {
            options->json = true;
        } else if (strcmp(argv[i], "--impulse") == 0) {
            options->impulse = true;
        } else if (strcmp(argv[i], "--timing") == 0) {
            options->timing = true;
        } else if (strcmp(argv[i], "--hal-trace-file") == 0 && i + 1 < argc) {
            options->hal_trace_file = argv[++i];
            ok = options->hal_trace_file[0] != '\0';
        } else if (strcmp(argv[i], "--capture-file") == 0 && i + 1 < argc) {
            options->capture_file = argv[++i];
            ok = options->capture_file[0] != '\0';
        } else if (strcmp(argv[i], "--seconds") == 0 && i + 1 < argc) {
            ok = mr_parse_number(argv[++i], 1.0, 120.0, &options->seconds);
        } else if (strcmp(argv[i], "--channels") == 0 && i + 1 < argc) {
            ok = mr_parse_number(argv[++i], 1.0, MR_MAX_CHANNELS, &options->channels) &&
                 options->channels == floor(options->channels);
        } else if (argv[i][0] != '-' && options->found < 2) {
            options->uids[options->found++] = argv[i];
        } else {
            ok = false;
        }
        if (!ok) {
            mr_usage(stderr);
            return 1;
        }
    }
    if (options->found == 0) {
        mr_usage(stderr);
        return 1;
    }
    return -1;
}

/// Finds and checks the devices and sizes the recording; NULL after printing why not.
static MRProbe *mr_prepare(const MROptions *options) {
    MRProbe *probe = calloc(1U, sizeof(MRProbe));
    if (probe == NULL) return NULL;
    probe->output = mr_find(options->uids[0]);
    probe->input = mr_find(options->uids[options->found == 2 ? 1 : 0]);
    if (probe->output == kAudioObjectUnknown || probe->input == kAudioObjectUnknown) {
        fprintf(stderr, "device not found\n");
        return NULL;
    }
    Float64 out_rate = 0.0, in_rate = 0.0;
    mr_get(probe->output, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal,
           &out_rate, sizeof(out_rate));
    mr_get(probe->input, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal,
           &in_rate, sizeof(in_rate));
    if (out_rate <= 0.0 || out_rate != in_rate) {
        fprintf(stderr, "both devices must run at the same nominal rate (%.0f vs %.0f Hz)\n",
                out_rate, in_rate);
        return NULL;
    }
    if (!mr_float_streams(probe->output, kAudioObjectPropertyScopeOutput, out_rate) ||
        !mr_float_streams(probe->input, kAudioObjectPropertyScopeInput, in_rate)) {
        fprintf(stderr, "every stream must offer packed native Float32 at the device rate\n");
        return NULL;
    }
    const uint32_t out_channels = mr_channels(probe->output, kAudioObjectPropertyScopeOutput);
    const uint32_t in_channels = mr_channels(probe->input, kAudioObjectPropertyScopeInput);
    probe->channels = (uint32_t)options->channels;
    if (probe->channels > out_channels) probe->channels = out_channels;
    if (probe->channels > in_channels) probe->channels = in_channels;
    probe->rate = out_rate;
    probe->impulse = options->impulse;
    if (probe->channels == 0U || !mr_allocate(probe, options->seconds)) {
        fprintf(stderr, "no common channels, or not enough memory for the recording\n");
        return NULL;
    }
    return probe;
}

/// Turns off this IOProc's streams in one direction. A recorder that left its output
/// streams on would also write to that device, which can change what the device reports
/// (MixRelay gates reported input latency to zero while another client writes).
static bool mr_disable_streams(AudioDeviceID device, AudioDeviceIOProcID proc,
                               AudioObjectPropertyScope scope) {
    if (mr_channels(device, scope) == 0U) return true;
    const AudioObjectPropertyAddress address = {kAudioDevicePropertyIOProcStreamUsage, scope,
                                                kAudioObjectPropertyElementMain};
    UInt32 size = 0U;
    if (AudioObjectGetPropertyDataSize(device, &address, 0U, NULL, &size) != noErr ||
        size < offsetof(AudioHardwareIOProcStreamUsage, mStreamIsOn)) {
        return false;
    }
    AudioHardwareIOProcStreamUsage *usage = calloc(1U, size);
    if (usage == NULL) return false;
    _Static_assert(sizeof(usage->mIOProc) == sizeof(proc), "IOProc identifier size");
    memcpy(&usage->mIOProc, &proc, sizeof(proc));
    OSStatus status = AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, usage);
    const size_t capacity =
        (size - offsetof(AudioHardwareIOProcStreamUsage, mStreamIsOn)) / sizeof(UInt32);
    if (status == noErr && usage->mNumberStreams <= capacity) {
        for (UInt32 stream = 0U; stream < usage->mNumberStreams; stream++) {
            usage->mStreamIsOn[stream] = 0U;
        }
        status = AudioObjectSetPropertyData(device, &address, 0U, NULL, size, usage);
    } else if (status == noErr) {
        status = kAudioHardwareBadPropertySizeError;
    }
    free(usage);
    return status == noErr;
}

static OSStatus mr_latency_changed(AudioObjectID object, UInt32 count,
                                    const AudioObjectPropertyAddress *addresses, void *context) {
    (void)object;
    MRProbe *probe = context;
    for (UInt32 index = 0; index < count; ++index) {
        if (addresses[index].mSelector == kAudioDevicePropertyLatency &&
            addresses[index].mScope == kAudioObjectPropertyScopeInput)
            atomic_fetch_add_explicit(&probe->latency_notifications, 1U, memory_order_relaxed);
    }
    return noErr;
}

/// Runs the devices for the signal and its tail. False when they could not start.
extern bool mr_probe_with_activity(bool (*measure)(void *), void *context);

static bool mr_run(void *context) {
    MRProbe *probe = context;
    const bool same = probe->output == probe->input;
    AudioDeviceIOProcID out_proc = NULL, in_proc = NULL;
    if (AudioDeviceCreateIOProcID(probe->output, mr_io, probe, &out_proc) != noErr ||
        (!same && AudioDeviceCreateIOProcID(probe->input, mr_io, probe, &in_proc) != noErr)) {
        fprintf(stderr, "could not create the IO procedures\n");
        return false;
    }
    // Separate devices: the player only plays and the recorder only records.
    if (!same && (!mr_disable_streams(probe->output, out_proc, kAudioObjectPropertyScopeInput) ||
                  !mr_disable_streams(probe->input, in_proc, kAudioObjectPropertyScopeOutput))) {
        fprintf(stderr, "could not limit the IO procedures to one direction each\n");
        return false;
    }
    const AudioObjectPropertyAddress latency = {kAudioDevicePropertyLatency,
        kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain};
    if (AudioObjectAddPropertyListener(probe->input, &latency, mr_latency_changed, probe) != noErr) {
        fprintf(stderr, "could not observe input latency notifications\n");
        return false;
    }
    // The recorder starts first so the first played frames are captured.
    if ((!same && AudioDeviceStart(probe->input, in_proc) != noErr) ||
        AudioDeviceStart(probe->output, out_proc) != noErr) {
        fprintf(stderr, "could not start the devices\n");
        return false;
    }
    usleep((useconds_t)((double)probe->run_frames / probe->rate * 1e6));
    OSStatus stopped = AudioDeviceStop(probe->output, out_proc);
    if (!same) stopped |= AudioDeviceStop(probe->input, in_proc);
    stopped |= AudioDeviceDestroyIOProcID(probe->output, out_proc);
    if (!same) stopped |= AudioDeviceDestroyIOProcID(probe->input, in_proc);
    stopped |= AudioObjectRemovePropertyListener(probe->input, &latency, mr_latency_changed, probe);
    if (stopped != noErr) fprintf(stderr, "warning: stopping the devices reported an error\n");
    return true;
}

static int mr_failure(bool json, const char *stage, OSStatus status, int error) {
    fprintf(stderr, "native probe failure: stage=%s osStatus=%d errno=%d\n", stage, (int)status, error);
    if (json) printf("\"error\":{\"stage\":\"%s\",\"osStatus\":%d,\"errno\":%d}}\n", stage, (int)status, error);
    return 1;
}

int main(int argc, char **argv) {
    const int authorization_status = mr_authorization_command(argc, argv);
    if (authorization_status >= 0) return authorization_status;
    MROptions options = {.seconds = 5.0, .channels = MR_MAX_CHANNELS};
    const int status = mr_options(argc, argv, &options);
    if (status >= 0) return status;
    MRInputAuthorization authorization = {0};
    if (!mr_authorize_input(30.0, &authorization)) return 2;
    MRProbe *probe = mr_prepare(&options);
    if (probe == NULL) return 1;
    const MRReported out_reported = mr_reported(probe->output, kAudioObjectPropertyScopeOutput);
    const MRReported in_reported = mr_reported(probe->input, kAudioObjectPropertyScopeInput);
    if (options.json) printf("{");
    if (options.json) {
        mr_print_input_authorization(stdout, &authorization);
        printf(",");
    }
    mr_describe("output", probe->output, kAudioObjectPropertyScopeOutput, options.json);
    mr_describe("input", probe->input, kAudioObjectPropertyScopeInput, options.json);
    if (options.json) printf("\"channels\":%u,\"seconds\":%.1f,", probe->channels, options.seconds);
    if (options.hal_trace_file != NULL) {
        const MRProbeHalTraceResult result = mr_probe_hal_trace_enable(probe->output, true);
        if (result.stage != NULL) return mr_failure(options.json, result.stage, result.os_status, result.file_error);
    }
    const bool ran = mr_probe_with_activity(mr_run, probe);
    MRProbeHalTraceResult trace = {0};
    if (options.hal_trace_file != NULL) {
        trace = mr_probe_hal_trace_save(probe->output, options.hal_trace_file);
        const MRProbeHalTraceResult disabled = mr_probe_hal_trace_enable(probe->output, false);
        if (trace.stage == NULL) trace = disabled;
        else if (disabled.stage != NULL)
            fprintf(stderr, "native probe cleanup: stage=%s osStatus=%d errno=%d\n", disabled.stage, (int)disabled.os_status, disabled.file_error);
    }
    if (trace.stage != NULL) return mr_failure(options.json, trace.stage, trace.os_status, trace.file_error);
    if (!ran) return mr_failure(options.json, "native_io", noErr, 0);
    if (options.capture_file != NULL) {
        FILE *file = fopen(options.capture_file, "wbx");
        bool saved = file != NULL && mr_write_capture(file, probe);
        if (file != NULL) saved = fclose(file) == 0 && saved;
        if (!saved) {
            return mr_failure(options.json, "capture_file", noErr, errno);
        }
    }
    if (options.json) printf("\"activityProtection\":true,");
    if (options.json) printf("\"inputLatencyNotifications\":%u,", atomic_load(&probe->latency_notifications));
    if (options.timing) {
        mach_timebase_info_data_t timebase;
        mach_timebase_info(&timebase);
        fprintf(stderr, "timebase,%u,%u\n", timebase.numer, timebase.denom);
        fprintf(stderr, "side,first,sampleTime,hostTime,frames,flags,callbackEnter,callbackExit\n");
        const MRSide *sides[] = {&probe->out, &probe->in};
        for (size_t side = 0; side < 2; ++side) {
            const uint32_t count = atomic_load(&sides[side]->count);
            for (uint32_t index = 0; index < count; ++index) {
                const MRChunk *chunk = &sides[side]->chunks[index];
                fprintf(stderr, "%s,%llu,%.17g,%llu,%u,%u,%llu,%llu\n", side == 0 ? "output" : "input",
                    (unsigned long long)chunk->first, chunk->sample_time,
                    (unsigned long long)chunk->host_time, chunk->frames, chunk->flags,
                    (unsigned long long)chunk->callback_enter, (unsigned long long)chunk->callback_exit);
            }
        }
    }
    if (probe->impulse) {
        const MRImpulses impulses = mr_analyse_impulses(probe);
        mr_report_impulses(probe, &impulses, &out_reported, &in_reported, options.json);
        return mr_impulse_status(&impulses, mr_incomplete(probe));
    }
    const MRAnalysis analysis = mr_analyse(probe, probe->output == probe->input);
    if (options.json) {
        mr_report_json(probe, &analysis, &out_reported, &in_reported);
    } else {
        mr_report_text(probe, &analysis, &out_reported, &in_reported);
    }
    if (analysis.locks == 0U) {
        if (!options.json) {
            printf("no exact copy of the signal was found: the path changes samples (gain, "
                   "conversion) or delivers nothing (all-zero input usually means the terminal "
                   "lacks microphone permission)\n");
        }
        return 3;
    }
    return 0;
}

