// Query the installed HAL. Optional setters affect only the explicit canonical UID.
#include <CoreAudio/CoreAudio.h>
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <stdbool.h>
#include <unistd.h>

typedef struct OutputControls { AudioObjectID volume, mute; } OutputControls;
typedef struct NativeStreamControl { AudioStreamID stream; AudioStreamBasicDescription format; } NativeStreamControl;

static bool format_encoding(const char *name, UInt32 *bits, UInt32 *flags) {
    if (strcmp(name, "float32") == 0) { *bits = 32U; *flags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked; }
    else {
        *flags = kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked;
        if (strcmp(name, "signed16") == 0) *bits = 16U;
        else if (strcmp(name, "signed24") == 0) *bits = 24U;
        else if (strcmp(name, "signed32") == 0) *bits = 32U;
        else return false;
    }
    return true;
}

static void require(OSStatus status) {
    if (status != noErr) {
        fprintf(stderr, "CoreAudio property failed: %d\n", (int)status);
        exit(2);
    }
}

static int inspect_driver(void) {
    CFStringRef bundle = CFSTR("io.mixrelay.audio.driver");
    AudioObjectID plugin = kAudioObjectUnknown;
    AudioObjectPropertyAddress property = {kAudioHardwarePropertyTranslateBundleIDToPlugIn,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = sizeof(plugin);
    require(AudioObjectGetPropertyData(kAudioObjectSystemObject, &property,
        sizeof(bundle), &bundle, &size, &plugin));
    if (size != sizeof(plugin)) return 2;

    // The current inventory transport supports at most 64 HAL devices.
    AudioDeviceID devices[64] = {0};
    UInt32 count = 0U;
    if (plugin != kAudioObjectUnknown) {
        property.mSelector = kAudioPlugInPropertyDeviceList;
        require(AudioObjectGetPropertyDataSize(plugin, &property, 0U, NULL, &size));
        if (size > sizeof(devices) || size % sizeof(devices[0]) != 0U) return 2;
        if (size != 0U) {
            require(AudioObjectGetPropertyData(plugin, &property, 0U, NULL, &size, devices));
            if (size > sizeof(devices) || size % sizeof(devices[0]) != 0U) return 2;
            count = size / (UInt32)sizeof(devices[0]);
        }
    }
    CFMutableArrayRef ids = CFArrayCreateMutable(NULL, (CFIndex)count,
        &kCFTypeArrayCallBacks);
    if (ids == NULL) return 2;
    for (UInt32 index = 0U; index < count; ++index) {
        int64_t value = devices[index];
        CFNumberRef number = CFNumberCreate(NULL, kCFNumberSInt64Type, &value);
        if (number == NULL) { CFRelease(ids); return 2; }
        CFArrayAppendValue(ids, number);
        CFRelease(number);
    }
    int64_t plugin_value = plugin, count_value = count;
    CFNumberRef plugin_number = CFNumberCreate(NULL, kCFNumberSInt64Type, &plugin_value);
    CFNumberRef count_number = CFNumberCreate(NULL, kCFNumberSInt64Type, &count_value);
    if (plugin_number == NULL || count_number == NULL) {
        if (plugin_number != NULL) CFRelease(plugin_number);
        if (count_number != NULL) CFRelease(count_number);
        CFRelease(ids);
        return 2;
    }
    const void *keys[] = {CFSTR("bundleIdentifier"), CFSTR("pluginID"),
        CFSTR("deviceCount"), CFSTR("deviceIDs")};
    const void *values[] = {bundle, plugin_number, count_number, ids};
    CFDictionaryRef result = CFDictionaryCreate(NULL, keys, values, 4,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFDataRef data = result == NULL ? NULL : CFPropertyListCreateData(NULL, result,
        kCFPropertyListXMLFormat_v1_0, 0, NULL);
    if (data != NULL) {
        fwrite(CFDataGetBytePtr(data), 1, (size_t)CFDataGetLength(data), stdout);
        CFRelease(data);
    }
    if (result != NULL) CFRelease(result);
    CFRelease(plugin_number); CFRelease(count_number); CFRelease(ids);
    return data == NULL ? 2 : 0;
}

static UInt32 control_uint(AudioObjectID object, AudioObjectPropertySelector selector) {
    AudioObjectPropertyAddress address = {selector, kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain};
    UInt32 value = 0, size = sizeof(value);
    require(AudioObjectGetPropertyData(object, &address, 0, NULL, &size, &value));
    return value;
}

static NativeStreamControl native_stream(AudioDeviceID device, AudioObjectPropertyScope scope) {
    NativeStreamControl result = {0};
    AudioObjectPropertyAddress property = {kAudioDevicePropertyStreams, scope, kAudioObjectPropertyElementMain};
    UInt32 size = 0U;
    require(AudioObjectGetPropertyDataSize(device, &property, 0U, NULL, &size));
    if (size == 0U) return result;
    if (size != sizeof(result.stream)) exit(2);
    require(AudioObjectGetPropertyData(device, &property, 0U, NULL, &size, &result.stream));
    property.mSelector = kAudioStreamPropertyPhysicalFormat;
    property.mScope = kAudioObjectPropertyScopeGlobal;
    size = sizeof(result.format);
    require(AudioObjectGetPropertyData(result.stream, &property, 0U, NULL, &size, &result.format));
    if (size != sizeof(result.format)) exit(2);
    return result;
}

static AudioStreamBasicDescription requested_format(NativeStreamControl stream, const char *name) {
    if (stream.stream == kAudioObjectUnknown) exit(2);
    AudioStreamBasicDescription target = stream.format;
    if (!format_encoding(name, &target.mBitsPerChannel, &target.mFormatFlags)) exit(2);
    target.mFormatID = kAudioFormatLinearPCM;
    target.mBytesPerFrame = target.mChannelsPerFrame * (target.mBitsPerChannel / 8U);
    target.mBytesPerPacket = target.mBytesPerFrame;
    target.mFramesPerPacket = 1U;
    target.mReserved = 0U;
    AudioObjectPropertyAddress property = {kAudioStreamPropertyAvailablePhysicalFormats,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioStreamRangedDescription formats[24];
    UInt32 size = sizeof(formats);
    require(AudioObjectGetPropertyData(stream.stream, &property, 0U, NULL, &size, formats));
    if (size > sizeof(formats) || size % sizeof(formats[0]) != 0U) exit(2);
    for (size_t index = 0U; index < size / sizeof(formats[0]); ++index) {
        if (memcmp(&target, &formats[index].mFormat, sizeof(target)) == 0) return target;
    }
    fputs("Requested native format is not advertised for the selected stream\n", stderr);
    exit(2);
}

static void set_native_format(NativeStreamControl stream, AudioStreamBasicDescription target) {
    AudioObjectPropertyAddress property = {kAudioStreamPropertyPhysicalFormat,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    require(AudioObjectSetPropertyData(stream.stream, &property, 0U, NULL, sizeof(target), &target));
    for (unsigned int attempt = 0U; attempt < 60U; ++attempt) {
        AudioStreamBasicDescription current = {0};
        UInt32 size = sizeof(current);
        require(AudioObjectGetPropertyData(stream.stream, &property, 0U, NULL, &size, &current));
        if (size == sizeof(current) && memcmp(&current, &target, sizeof(current)) == 0) return;
        (void)usleep(50000U);
    }
    fputs("Native format did not reach requested physical readback\n", stderr);
    exit(2);
}

static CFStringRef native_format_name(NativeStreamControl stream) {
    if (stream.stream == kAudioObjectUnknown) return NULL;
    if (stream.format.mFormatFlags == (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked) && stream.format.mBitsPerChannel == 32U) return CFSTR("float32");
    if (stream.format.mFormatFlags != (kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked)) exit(2);
    if (stream.format.mBitsPerChannel == 16U) return CFSTR("signed16");
    if (stream.format.mBitsPerChannel == 24U) return CFSTR("signed24");
    if (stream.format.mBitsPerChannel == 32U) return CFSTR("signed32");
    exit(2);
}

static OutputControls output_controls(AudioDeviceID device) {
    AudioObjectPropertyAddress address = {kAudioObjectPropertyControlList,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    require(AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size));
    OutputControls result = {kAudioObjectUnknown, kAudioObjectUnknown};
    if (size == 0) return result;
    AudioObjectID *controls = malloc(size);
    if (controls == NULL) exit(2);
    require(AudioObjectGetPropertyData(device, &address, 0, NULL, &size, controls));
    for (UInt32 i = 0; i < size / sizeof(*controls); i++) {
        if (control_uint(controls[i], kAudioControlPropertyScope) != kAudioObjectPropertyScopeOutput ||
            control_uint(controls[i], kAudioControlPropertyElement) != kAudioObjectPropertyElementMain) continue;
        UInt32 kind = control_uint(controls[i], kAudioObjectPropertyClass);
        if (kind == kAudioVolumeControlClassID) result.volume = controls[i];
        if (kind == kAudioMuteControlClassID) result.mute = controls[i];
    }
    free(controls);
    return result;
}

static UInt32 channels(AudioDeviceID device, AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress address = {
        kAudioDevicePropertyStreamConfiguration, scope, kAudioObjectPropertyElementMain
    };
    UInt32 size = 0;
    require(AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size));
    if (size < offsetof(AudioBufferList, mBuffers) || size > 65536U) exit(2);
    AudioBufferList *buffers = calloc(1, size);
    if (buffers == NULL) exit(2);
    require(AudioObjectGetPropertyData(device, &address, 0, NULL, &size, buffers));
    if (buffers->mNumberBuffers > (size - offsetof(AudioBufferList, mBuffers)) / sizeof(AudioBuffer)) exit(2);
    UInt32 count = 0;
    for (UInt32 i = 0; i < buffers->mNumberBuffers; i++) count += buffers->mBuffers[i].mNumberChannels;
    free(buffers);
    return count;
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--driver") == 0) return inspect_driver();
    static const char prefix[] = "io.mixrelay.audio.endpoint.";
    if (argc < 2 || strlen(argv[1]) != sizeof(prefix) - 1U + 32U ||
        strncmp(argv[1], prefix, sizeof(prefix) - 1U) != 0) return 2;
    for (size_t i = sizeof(prefix) - 1U; i < sizeof(prefix) - 1U + 32U; i++) {
        char digit = argv[1][i];
        if (!((digit >= '0' && digit <= '9') || (digit >= 'a' && digit <= 'f'))) return 2;
    }
    if (strcmp(argv[1] + sizeof(prefix) - 1U, "00000000000000000000000000000000") == 0) return 2;
    bool set_volume = false, set_mute = false;
    Float32 volume = 1.0f;
    UInt32 muted = 0;
    const char *render_format = NULL, *capture_format = NULL;
    // Validate every argument before resolving or mutating the selected device.
    for (int i = 2; i < argc; i += 2) {
        if (i + 1 >= argc) return 2;
        if (strcmp(argv[i], "--output-volume") == 0 && !set_volume) {
            char *end = NULL;
            volume = strtof(argv[i + 1], &end);
            if (end == argv[i + 1] || *end != '\0' || !isfinite(volume) || volume < 0.0f || volume > 1.0f) return 2;
            set_volume = true;
        } else if (strcmp(argv[i], "--output-mute") == 0 && !set_mute) {
            if (strcmp(argv[i + 1], "0") != 0 && strcmp(argv[i + 1], "1") != 0) return 2;
            muted = strcmp(argv[i + 1], "1") == 0 ? 1U : 0U;
            set_mute = true;
        } else if ((strcmp(argv[i], "--render-format") == 0 && render_format == NULL) ||
                   (strcmp(argv[i], "--capture-format") == 0 && capture_format == NULL)) {
            UInt32 bits = 0U, flags = 0U;
            if (!format_encoding(argv[i + 1], &bits, &flags)) return 2;
            if (strcmp(argv[i], "--render-format") == 0) render_format = argv[i + 1];
            else capture_format = argv[i + 1];
        } else return 2;
    }
    CFStringRef uid = CFStringCreateWithCString(NULL, argv[1], kCFStringEncodingUTF8);
    if (uid == NULL) return 2;
    AudioDeviceID device = kAudioObjectUnknown;
    AudioObjectPropertyAddress address = {
        kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain
    };
    UInt32 size = sizeof(device);
    require(AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, sizeof(uid), &uid, &size, &device));
    CFRelease(uid);
    if (device == kAudioObjectUnknown) return 2;
    const OutputControls controls = output_controls(device);
    NativeStreamControl render_stream = native_stream(device, kAudioDevicePropertyScopeOutput);
    NativeStreamControl capture_stream = native_stream(device, kAudioDevicePropertyScopeInput);
    AudioStreamBasicDescription render_target = {0}, capture_target = {0};
    if (render_format != NULL) render_target = requested_format(render_stream, render_format);
    if (capture_format != NULL) capture_target = requested_format(capture_stream, capture_format);
    // Resolve both requested controls before the first write, so unsupported
    // arguments cannot leave only half of a requested control update applied.
    if ((set_volume && controls.volume == kAudioObjectUnknown) ||
        (set_mute && controls.mute == kAudioObjectUnknown)) return 2;
    if (render_format != NULL) set_native_format(render_stream, render_target);
    if (capture_format != NULL) set_native_format(capture_stream, capture_target);
    render_stream = native_stream(device, kAudioDevicePropertyScopeOutput);
    capture_stream = native_stream(device, kAudioDevicePropertyScopeInput);
    if (set_volume) {
        address.mSelector = kAudioLevelControlPropertyScalarValue;
        require(AudioObjectSetPropertyData(controls.volume, &address, 0, NULL, sizeof(volume), &volume));
    }
    if (set_mute) {
        address.mSelector = kAudioBooleanControlPropertyValue;
        require(AudioObjectSetPropertyData(controls.mute, &address, 0, NULL, sizeof(muted), &muted));
    }
    if (controls.volume != kAudioObjectUnknown) {
        address.mSelector = kAudioLevelControlPropertyScalarValue;
        size = sizeof(volume);
        require(AudioObjectGetPropertyData(controls.volume, &address, 0, NULL, &size, &volume));
    }
    if (controls.mute != kAudioObjectUnknown) muted = control_uint(controls.mute, kAudioBooleanControlPropertyValue);
    address.mSelector = kAudioObjectPropertyName;
    CFStringRef name = NULL;
    size = sizeof(name);
    require(AudioObjectGetPropertyData(device, &address, 0, NULL, &size, &name));
    if (name == NULL) return 2;
    // CoreFoundation preserves localized names without hand-written escaping.
    int render = (int)channels(device, kAudioDevicePropertyScopeOutput);
    int capture = (int)channels(device, kAudioDevicePropertyScopeInput);
    CFNumberRef r = CFNumberCreate(NULL, kCFNumberIntType, &render);
    CFNumberRef c = CFNumberCreate(NULL, kCFNumberIntType, &capture);
    CFNumberRef level = NULL;
    const void *keys[9] = {CFSTR("displayName"), CFSTR("renderChannels"), CFSTR("captureChannels"),
        CFSTR("outputVolumeAvailable"), CFSTR("outputMuteAvailable")};
    const void *values[9] = {name, r, c,
        controls.volume != kAudioObjectUnknown ? kCFBooleanTrue : kCFBooleanFalse,
        controls.mute != kAudioObjectUnknown ? kCFBooleanTrue : kCFBooleanFalse};
    CFIndex count = 5;
    if (controls.volume != kAudioObjectUnknown) {
        level = CFNumberCreate(NULL, kCFNumberFloat32Type, &volume);
        keys[count] = CFSTR("outputVolumeScalar"); values[count++] = level;
    }
    if (controls.mute != kAudioObjectUnknown) {
        keys[count] = CFSTR("outputMuted"); values[count++] = muted != 0 ? kCFBooleanTrue : kCFBooleanFalse;
    }
    if (render_stream.stream != kAudioObjectUnknown) {
        keys[count] = CFSTR("renderNativeFormat"); values[count++] = native_format_name(render_stream);
    }
    if (capture_stream.stream != kAudioObjectUnknown) {
        keys[count] = CFSTR("captureNativeFormat"); values[count++] = native_format_name(capture_stream);
    }
    CFDictionaryRef result = CFDictionaryCreate(NULL, keys, values, count,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    // Property lists preserve Unicode and are straightforward to consume with plutil.
    CFDataRef data = CFPropertyListCreateData(NULL, result, kCFPropertyListXMLFormat_v1_0, 0, NULL);
    if (data == NULL) return 2;
    fwrite(CFDataGetBytePtr(data), 1, (size_t)CFDataGetLength(data), stdout);
    CFRelease(data); CFRelease(result); CFRelease(r); CFRelease(c); CFRelease(name);
    if (level != NULL) CFRelease(level);
    return 0;
}
