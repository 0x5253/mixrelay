#include "macos-source-device.h"
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <stdbool.h>
#include <stddef.h>

static OSStatus read_property(AudioObjectID object, AudioObjectPropertySelector selector,
                              AudioObjectPropertyScope scope, void *value, UInt32 bytes) {
    const AudioObjectPropertyAddress address = {selector, scope, kAudioObjectPropertyElementMain};
    UInt32 size = bytes;
    const OSStatus status = AudioObjectGetPropertyData(object, &address, 0U, NULL, &size, value);
    return status != noErr ? status : size == bytes ? noErr : kAudioHardwareBadPropertySizeError;
}

static OSStatus admit_stream(AudioDeviceID device, AudioObjectPropertyScope scope, uint32_t rate) {
    AudioObjectID stream = 0U;
    OSStatus status = read_property(device, kAudioDevicePropertyStreams, scope, &stream, sizeof(stream));
    if (status != noErr) return status;
    const AudioObjectPropertySelector selectors[] = {kAudioStreamPropertyPhysicalFormat,
        kAudioStreamPropertyVirtualFormat};
    for (size_t i = 0U; i < sizeof(selectors) / sizeof(selectors[0]); i++) {
        AudioStreamBasicDescription format = {0};
        status = read_property(stream, selectors[i], kAudioObjectPropertyScopeGlobal,
                               &format, sizeof(format));
        if (status != noErr) return status;
        if (format.mSampleRate != rate || format.mFormatID != kAudioFormatLinearPCM ||
            format.mFormatFlags != (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked) ||
            format.mChannelsPerFrame != 2U || format.mBitsPerChannel != 32U ||
            format.mBytesPerFrame != 8U || format.mBytesPerPacket != 8U ||
            format.mFramesPerPacket != 1U) return kAudioDeviceUnsupportedFormatError;
    }
    return noErr;
}

OSStatus mr_source_device(const char *uid, MRSourceDevice *facts) {
    CFStringRef selected = CFStringCreateWithCString(NULL, uid, kCFStringEncodingUTF8);
    if (selected == NULL) return kAudioHardwareIllegalOperationError;
    const AudioObjectPropertyAddress address = {kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = sizeof(facts->device);
    OSStatus status = AudioObjectGetPropertyData(kAudioObjectSystemObject, &address,
        sizeof(selected), &selected, &size, &facts->device);
    CFRelease(selected);
    if (status != noErr || size != sizeof(facts->device) || facts->device == 0U)
        return status != noErr ? status : kAudioHardwareBadDeviceError;
    Float64 rate = 0.0;
    status = read_property(facts->device, kAudioDevicePropertyNominalSampleRate,
                          kAudioObjectPropertyScopeGlobal, &rate, sizeof(rate));
    if (status != noErr || rate != 48000.0) return status != noErr ? status : kAudioDeviceUnsupportedFormatError;
    facts->rate = 48000U;
    facts->channels = 2U;
    status = read_property(facts->device, kAudioDevicePropertyBufferFrameSize,
                          kAudioObjectPropertyScopeGlobal, &facts->quantum, sizeof(facts->quantum));
    if (status != noErr || facts->quantum != 512U)
        return status != noErr ? status : kAudioDeviceUnsupportedFormatError;
    status = read_property(facts->device, kAudioDevicePropertyZeroTimeStampPeriod,
                          kAudioObjectPropertyScopeGlobal, &facts->timestamp_period, sizeof(facts->timestamp_period));
    if (status != noErr || facts->timestamp_period == 0U)
        return status != noErr ? status : kAudioHardwareIllegalOperationError;
    status = admit_stream(facts->device, kAudioObjectPropertyScopeInput, facts->rate);
    return status != noErr ? status : admit_stream(facts->device, kAudioObjectPropertyScopeOutput, facts->rate);
}
