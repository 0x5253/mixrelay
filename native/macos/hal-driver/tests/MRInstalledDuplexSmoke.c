#include <CoreAudio/CoreAudio.h>

#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static const Float64 MR_SAMPLE_RATE = 48000.0;
static const uint32_t MR_CHANNELS = 2U;
static const uint64_t MR_WARMUP_FRAMES = 48000U;
static const useconds_t MR_TEST_DURATION_US = 3000000U;
static const float MR_SIGNAL[2] = {0.25F, 0.125F};
// Positive PCM allows fixed 10 ppm (-100 dB relative) independent-sinc rounding error.
static const float MR_MAX_RELATIVE_ERROR = 0.00001F;

typedef struct MRPCMFormat {
    uint32_t bits, bytes;
    bool floating;
    float scale;
} MRPCMFormat;

typedef struct MRSmokeState {
    _Atomic uint64_t output_frames;
    _Atomic uint64_t compared_samples;
    _Atomic uint64_t matching_samples;
    _Atomic uint64_t nonzero_samples;
    _Atomic uint64_t invalid_callbacks;
    _Atomic uint32_t last_sample_bits[2];
    _Atomic uint32_t max_error_bits[2];
    float expected_gain;
    MRPCMFormat render_format, capture_format;
} MRSmokeState;

static float mr_read_sample(const uint8_t *samples, size_t index, MRPCMFormat format) {
    const uint8_t *bytes = samples + index * format.bytes;
    if (format.floating) {
        float value;
        memcpy(&value, bytes, sizeof(value));
        return value;
    }
    uint32_t raw = 0U;
    for (uint32_t byte = 0U; byte < format.bytes; ++byte) raw |= (uint32_t)bytes[byte] << (byte * 8U);
    if (format.bits < 32U && (raw & (UINT32_C(1) << (format.bits - 1U))) != 0U) raw |= UINT32_MAX << format.bits;
    int32_t value;
    memcpy(&value, &raw, sizeof(value));
    return (float)value / format.scale;
}

static void mr_write_signal(uint8_t *samples, size_t index, uint32_t channel, MRPCMFormat format) {
    uint8_t *bytes = samples + index * format.bytes;
    if (format.floating) {
        memcpy(bytes, &MR_SIGNAL[channel], sizeof(float));
        return;
    }
    // These binary fractions are exactly representable in every supported integer depth.
    const int32_t value = (int32_t)(MR_SIGNAL[channel] * format.scale);
    uint32_t raw;
    memcpy(&raw, &value, sizeof(raw));
    for (uint32_t byte = 0U; byte < format.bytes; ++byte) bytes[byte] = (uint8_t)(raw >> (byte * 8U));
}

static bool mr_interleaved_stereo_buffer(const AudioBufferList *buffers,
                                         MRPCMFormat format,
                                         uint32_t *frame_count,
                                         uint8_t **samples) {
    const uint32_t frame_bytes = MR_CHANNELS * format.bytes;
    if (buffers == NULL || buffers->mNumberBuffers != 1U ||
        buffers->mBuffers[0].mNumberChannels != MR_CHANNELS ||
        buffers->mBuffers[0].mData == NULL ||
        buffers->mBuffers[0].mDataByteSize == 0U ||
        buffers->mBuffers[0].mDataByteSize % frame_bytes != 0U) {
        return false;
    }
    *frame_count = buffers->mBuffers[0].mDataByteSize / frame_bytes;
    *samples = buffers->mBuffers[0].mData;
    return true;
}

static OSStatus mr_smoke_io(AudioDeviceID device,
                            const AudioTimeStamp *now,
                            const AudioBufferList *input_data,
                            const AudioTimeStamp *input_time,
                            AudioBufferList *output_data,
                            const AudioTimeStamp *output_time,
                            void *context) {
    (void)device;
    (void)now;
    (void)input_time;
    (void)output_time;
    MRSmokeState *const state = context;
    uint32_t frame_count = 0U;
    uint8_t *output_samples = NULL;
    if (!mr_interleaved_stereo_buffer(
            output_data, state->render_format, &frame_count, &output_samples)) {
        atomic_fetch_add_explicit(
            &state->invalid_callbacks, UINT64_C(1), memory_order_relaxed);
        return noErr;
    }
    for (uint32_t frame = 0U; frame < frame_count; ++frame) {
        for (uint32_t channel = 0U; channel < MR_CHANNELS; ++channel) {
            mr_write_signal(output_samples, (size_t)frame * MR_CHANNELS + channel,
                channel, state->render_format);
        }
    }

    const uint64_t output_frames = atomic_fetch_add_explicit(
        &state->output_frames, frame_count, memory_order_relaxed);
    uint32_t input_frames = 0U;
    uint8_t *input_samples = NULL;
    if (!mr_interleaved_stereo_buffer(
            input_data, state->capture_format, &input_frames, &input_samples) || input_frames != frame_count) {
        atomic_fetch_add_explicit(
            &state->invalid_callbacks, UINT64_C(1), memory_order_relaxed);
        return noErr;
    }
    // One second of real output primes the route; no frame-offset or latency claim.
    if (output_frames < MR_WARMUP_FRAMES) {
        return noErr;
    }

    uint64_t compared = 0U;
    uint64_t matching = 0U;
    uint64_t nonzero = 0U;
    float max_error[2];
    for (uint32_t channel = 0U; channel < MR_CHANNELS; ++channel) {
        const uint32_t bits = atomic_load_explicit(
            &state->max_error_bits[channel], memory_order_relaxed);
        memcpy(&max_error[channel], &bits, sizeof(bits));
    }
    for (uint32_t frame = 0U; frame < frame_count; ++frame) {
        for (uint32_t channel = 0U; channel < MR_CHANNELS; ++channel) {
            const float actual = mr_read_sample(input_samples,
                (size_t)frame * MR_CHANNELS + channel, state->capture_format);
            const float expected =
                MR_SIGNAL[channel] * state->expected_gain;
            const float error = fabsf(actual - expected);
            if (error > max_error[channel] || !isfinite(error)) {
                max_error[channel] = error;
            }
            ++compared;
            // Integer capture adds at most one truncation LSB to the fixed SRC error.
            const bool matches = state->expected_gain == 0.0F
                ? memcmp(&actual, &expected, sizeof(actual)) == 0
                : isfinite(actual) && error <= expected * MR_MAX_RELATIVE_ERROR +
                    (state->capture_format.floating ? 0.0F : 1.0F / state->capture_format.scale);
            if (matches) {
                ++matching;
            }
            if (actual != 0.0F) {
                ++nonzero;
            }
        }
    }
    // This IOProc is the diagnostics writer; the control thread only loads atomics.
    for (uint32_t channel = 0U; channel < MR_CHANNELS; ++channel) {
        const float last = mr_read_sample(input_samples,
            (size_t)(frame_count - 1U) * MR_CHANNELS + channel, state->capture_format);
        uint32_t last_bits = 0U, error_bits = 0U;
        memcpy(&last_bits, &last, sizeof(last_bits));
        memcpy(&error_bits, &max_error[channel], sizeof(error_bits));
        atomic_store_explicit(&state->last_sample_bits[channel], last_bits, memory_order_relaxed);
        atomic_store_explicit(&state->max_error_bits[channel], error_bits, memory_order_relaxed);
    }
    atomic_fetch_add_explicit(
        &state->compared_samples, compared, memory_order_relaxed);
    atomic_fetch_add_explicit(
        &state->matching_samples, matching, memory_order_relaxed);
    atomic_fetch_add_explicit(
        &state->nonzero_samples, nonzero, memory_order_relaxed);
    return noErr;
}

static bool mr_canonical_uid(const char *uid) {
    static const char prefix[] = "io.mixrelay.audio.endpoint.";
    const size_t prefix_size = sizeof(prefix) - 1U;
    if (strlen(uid) != prefix_size + 32U || strncmp(uid, prefix, prefix_size) != 0) {
        return false;
    }
    bool nonzero = false;
    for (size_t index = prefix_size; index < prefix_size + 32U; ++index) {
        const char digit = uid[index];
        if (!((digit >= '0' && digit <= '9') || (digit >= 'a' && digit <= 'f'))) {
            return false;
        }
        nonzero |= digit != '0';
    }
    return nonzero;
}

static AudioDeviceID mr_find_device(const char *canonical_uid) {
    AudioObjectPropertyAddress address = {
        .mSelector = kAudioHardwarePropertyTranslateUIDToDevice,
        .mScope = kAudioObjectPropertyScopeGlobal,
        .mElement = kAudioObjectPropertyElementMain,
    };
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    CFStringRef uid = CFStringCreateWithCString(NULL, canonical_uid, kCFStringEncodingUTF8);
    if (uid == NULL) return kAudioObjectUnknown;
    const OSStatus status = AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &address,
            sizeof(uid),
            &uid,
            &size,
            &device);
    CFRelease(uid);
    if (status != noErr || size != sizeof(device)) {
        return kAudioObjectUnknown;
    }
    return device;
}

static bool mr_validate_format(AudioDeviceID device, MRSmokeState *state) {
    AudioObjectPropertyAddress address = {
        .mSelector = kAudioDevicePropertyNominalSampleRate,
        .mScope = kAudioObjectPropertyScopeGlobal,
        .mElement = kAudioObjectPropertyElementMain,
    };
    Float64 rate = 0.0;
    UInt32 size = sizeof(rate);
    if (AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, &rate) != noErr ||
        size != sizeof(rate) || rate != MR_SAMPLE_RATE) return false;
    const AudioObjectPropertyScope scopes[2] = {
        kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput
    };
    for (size_t index = 0U; index < 2U; ++index) {
        address.mSelector = kAudioDevicePropertyStreamConfiguration;
        address.mScope = scopes[index];
        AudioBufferList configuration = {0};
        size = sizeof(configuration);
        if (AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, &configuration) != noErr ||
            size != sizeof(configuration) || configuration.mNumberBuffers != 1U ||
            configuration.mBuffers[0].mNumberChannels != MR_CHANNELS) return false;
        address.mSelector = kAudioDevicePropertyStreams;
        AudioStreamID stream = kAudioObjectUnknown;
        size = sizeof(stream);
        if (AudioObjectGetPropertyData(device, &address, 0U, NULL, &size, &stream) != noErr ||
            size != sizeof(stream) || stream == kAudioObjectUnknown) return false;
        AudioObjectPropertyAddress property = {kAudioStreamPropertyPhysicalFormat,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        AudioStreamBasicDescription native = {0}, virtual = {0};
        size = sizeof(native);
        if (AudioObjectGetPropertyData(stream, &property, 0U, NULL, &size, &native) != noErr || size != sizeof(native)) return false;
        property.mSelector = kAudioStreamPropertyVirtualFormat;
        size = sizeof(virtual);
        if (AudioObjectGetPropertyData(stream, &property, 0U, NULL, &size, &virtual) != noErr ||
            size != sizeof(virtual) || memcmp(&native, &virtual, sizeof(native)) != 0) return false;
        const bool floating = native.mFormatFlags == (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked);
        const bool integer = native.mFormatFlags == (kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked);
        if (native.mFormatID != kAudioFormatLinearPCM || native.mSampleRate != MR_SAMPLE_RATE ||
            native.mChannelsPerFrame != MR_CHANNELS || native.mFramesPerPacket != 1U || native.mReserved != 0U ||
            (floating ? native.mBitsPerChannel != 32U : (!integer ||
                (native.mBitsPerChannel != 16U && native.mBitsPerChannel != 24U && native.mBitsPerChannel != 32U))) ||
            native.mBytesPerFrame != MR_CHANNELS * (native.mBitsPerChannel / 8U) ||
            native.mBytesPerPacket != native.mBytesPerFrame) return false;
        const MRPCMFormat format = {native.mBitsPerChannel, native.mBitsPerChannel / 8U, floating,
            native.mBitsPerChannel == 16U ? 32768.0F : (native.mBitsPerChannel == 24U ? 8388608.0F : 2147483648.0F)};
        if (index == 0U) state->capture_format = format;
        else state->render_format = format;
    }
    return true;
}

int main(int argc, char **argv) {
    if (argc != 3 || !mr_canonical_uid(argv[1]) ||
        (strcmp(argv[2], "0") != 0 && strcmp(argv[2], "0.5") != 0 && strcmp(argv[2], "1") != 0)) {
        fputs("usage: MRInstalledDuplexSmoke <canonical-uid> <expected-gain: 0|0.5|1>\n", stderr);
        return 1;
    }
    const float expected_gain = strcmp(argv[2], "0") == 0 ? 0.0F :
        (strcmp(argv[2], "0.5") == 0 ? 0.5F : 1.0F);
    const AudioDeviceID device = mr_find_device(argv[1]);
    if (device == kAudioObjectUnknown) {
        fputs("MixRelay device is not registered with Core Audio\n", stderr);
        return 2;
    }
    MRSmokeState state = {0};
    state.expected_gain = expected_gain;
    if (!mr_validate_format(device, &state)) {
        fputs("Exact MixRelay device must already have 2/2 channels at 48000 Hz; no settings were changed\n", stderr);
        return 3;
    }

    AudioDeviceIOProcID io_proc = NULL;
    if (AudioDeviceCreateIOProcID(
            device, mr_smoke_io, &state, &io_proc) != noErr) {
        fputs("Could not create MixRelay device IO callback\n", stderr);
        return 4;
    }
    if (AudioDeviceStart(device, io_proc) != noErr) {
        (void)AudioDeviceDestroyIOProcID(device, io_proc);
        fputs("Could not start MixRelay device IO\n", stderr);
        return 5;
    }
    (void)usleep(MR_TEST_DURATION_US);
    const OSStatus stop_result = AudioDeviceStop(device, io_proc);
    const OSStatus destroy_result =
        AudioDeviceDestroyIOProcID(device, io_proc);

    const uint64_t compared = atomic_load_explicit(
        &state.compared_samples, memory_order_relaxed);
    const uint64_t matching = atomic_load_explicit(
        &state.matching_samples, memory_order_relaxed);
    const uint64_t nonzero = atomic_load_explicit(
        &state.nonzero_samples, memory_order_relaxed);
    const uint64_t invalid = atomic_load_explicit(
        &state.invalid_callbacks, memory_order_relaxed);
    printf(
        "MixRelay duplex PCM: uid=%s channels=2/2 rate=48000 expected_gain=%.1f compared=%llu matching=%llu nonzero=%llu invalid_callbacks=%llu\n",
        argv[1],
        (double)expected_gain,
        (unsigned long long)compared,
        (unsigned long long)matching,
        (unsigned long long)nonzero,
        (unsigned long long)invalid);
    printf("MixRelay native PCM: render=%s%u capture=%s%u\n",
        state.render_format.floating ? "float" : "signed", (unsigned int)state.render_format.bits,
        state.capture_format.floating ? "float" : "signed", (unsigned int)state.capture_format.bits);
    for (uint32_t channel = 0U; channel < MR_CHANNELS; ++channel) {
        const uint32_t last_bits = atomic_load_explicit(
            &state.last_sample_bits[channel], memory_order_relaxed);
        const uint32_t error_bits = atomic_load_explicit(
            &state.max_error_bits[channel], memory_order_relaxed);
        float last = 0.0F, max_error = 0.0F;
        memcpy(&last, &last_bits, sizeof(last));
        memcpy(&max_error, &error_bits, sizeof(max_error));
        printf("MixRelay duplex PCM ch%u: last=%.9g expected=%.9g max_abs_error=%.9g\n",
            (unsigned int)(channel + 1U), (double)last,
            (double)(MR_SIGNAL[channel] * expected_gain), (double)max_error);
    }
    if (stop_result != noErr || destroy_result != noErr ||
        compared < (uint64_t)MR_SAMPLE_RATE || matching != compared ||
        nonzero != (expected_gain == 0.0F ? 0U : compared) || invalid != 0U) {
        return 6;
    }
    puts("MixRelay installed Core Audio duplex smoke passed");
    return 0;
}
