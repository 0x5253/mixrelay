/* Opt-in owned signal source. Never uses or changes the system default output. */
#import <AVFAudio/AVFAudio.h>
#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <AudioUnit/AudioUnit.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>
#include <poll.h>
#include <errno.h>
#include <unistd.h>

#ifndef MR_TONE_FREQUENCY
#define MR_TONE_FREQUENCY 440.0
#endif
#ifndef MR_TONE_CHANNEL
#define MR_TONE_CHANNEL -1
#endif

static AudioDeviceID MRVirtualDevice(const char *device_uid) {
    static const char prefix[] = "io.mixrelay.audio.endpoint.";
    const size_t prefix_size = sizeof(prefix) - 1;
    if (strlen(device_uid) != prefix_size + 32 ||
        strncmp(device_uid, prefix, prefix_size) != 0) return kAudioObjectUnknown;
    bool nonzero = false;
    for (size_t index = prefix_size; index < prefix_size + 32; ++index) {
        const char digit = device_uid[index];
        if (!((digit >= '0' && digit <= '9') || (digit >= 'a' && digit <= 'f')))
            return kAudioObjectUnknown;
        nonzero |= digit != '0';
    }
    if (!nonzero) return kAudioObjectUnknown;
    CFStringRef uid = CFStringCreateWithCString(NULL, device_uid, kCFStringEncodingUTF8);
    if (uid == NULL) return kAudioObjectUnknown;
    AudioDeviceID device = kAudioObjectUnknown;
    AudioObjectPropertyAddress address = { kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    UInt32 size = sizeof(device);
    const OSStatus status = AudioObjectGetPropertyData(kAudioObjectSystemObject,
        &address, sizeof(uid), &uid, &size, &device);
    CFRelease(uid);
    return status == noErr && size == sizeof(device) ? device : kAudioObjectUnknown;
}

static bool MRResumeConfiguredEngine(AVAudioEngine *engine, AudioUnit unit,
        AudioDeviceID selected, uint64_t change, uint64_t *handled, uint64_t *restarts) {
    if (change == *handled) return true;
    *handled = change;
    if (engine.isRunning) return true;
    AudioDeviceID actual = kAudioObjectUnknown;
    UInt32 bytes = sizeof(actual);
    NSError *error = nil;
    if (AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global, 0, &actual, &bytes) != noErr || actual != selected ||
        *restarts >= 16 || ![engine startAndReturnError:&error]) return false;
    *restarts += 1;
    fprintf(stderr, "owned tone: resumed after configuration change %llu\n",
        (unsigned long long)change);
    return true;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 3 || strcmp(argv[1], "--mixrelay-virtual-only") != 0) return EXIT_FAILURE;
        fprintf(stderr, "owned tone: resolving exact MixRelay UID\n");
        const AudioDeviceID device = MRVirtualDevice(argv[2]);
        if (device == kAudioObjectUnknown) {
            fprintf(stderr, "exact MixRelay virtual device absent; refusing default-output fallback\n");
            return EXIT_FAILURE;
        }
        fprintf(stderr, "owned tone: configuring explicit virtual AudioUnit\n");
        AVAudioEngine *engine = [[AVAudioEngine alloc] init];
        AudioUnit unit = engine.outputNode.audioUnit;
        if (unit == NULL || AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                kAudioUnitScope_Global, 0, &device, sizeof(device)) != noErr) return EXIT_FAILURE;
        AudioDeviceID actual = kAudioObjectUnknown;
        UInt32 bytes = sizeof(actual);
        if (AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                kAudioUnitScope_Global, 0, &actual, &bytes) != noErr || actual != device) return EXIT_FAILURE;
        AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:48000 channels:2];
        __block double phase = 0;
        _Atomic uint64_t rendered_frames = 0;
        _Atomic uint64_t configuration_changes = 0;
        _Atomic uint64_t *rendered = &rendered_frames;
        _Atomic uint64_t *changes = &configuration_changes;
        id observer = [NSNotificationCenter.defaultCenter
            addObserverForName:AVAudioEngineConfigurationChangeNotification object:engine queue:nil
            usingBlock:^(NSNotification *notification) {
                (void)notification;
                atomic_fetch_add_explicit(changes, 1, memory_order_relaxed);
            }];
        AVAudioSourceNode *source = [[AVAudioSourceNode alloc] initWithFormat:format
            renderBlock:^OSStatus(BOOL *silent, const AudioTimeStamp *time,
                AVAudioFrameCount frames, AudioBufferList *output) {
                (void)time; *silent = NO;
                for (AVAudioFrameCount frame = 0; frame < frames; ++frame) {
                    const float sample = (float)(sin(phase * 2.0 * M_PI) * 0.05);
                    phase += MR_TONE_FREQUENCY / 48000.0;
                    if (phase >= 1.0) phase -= 1.0;
                    for (UInt32 channel = 0; channel < output->mNumberBuffers; ++channel)
                        ((float *)output->mBuffers[channel].mData)[frame] =
                            MR_TONE_CHANNEL < 0 || channel == (UInt32)MR_TONE_CHANNEL ? sample : 0;
                }
                atomic_fetch_add_explicit(rendered, frames, memory_order_relaxed);
                return noErr;
            }];
        [engine attachNode:source];
        [engine connect:source to:engine.mainMixerNode format:format];
        NSError *error = nil;
        fprintf(stderr, "owned tone: starting explicit virtual output\n");
        if (![engine startAndReturnError:&error]) {
            fprintf(stderr, "owned virtual tone start failed: %s\n", error.localizedDescription.UTF8String);
            return EXIT_FAILURE;
        }
        printf("ready %s\n", argv[2]);
        fflush(stdout);
        /* Only the owning fixture can move this source between exact owned UIDs. */
        char command[128];
        uint64_t handled_change = 0;
        uint64_t restarts = 0;
        AudioDeviceID selected = device;
        while (true) {
            const uint64_t change = atomic_load_explicit(changes, memory_order_relaxed);
            if (!MRResumeConfiguredEngine(engine, unit, selected, change, &handled_change, &restarts)) {
                fprintf(stderr, "owned tone: configuration recovery failed on exact virtual device\n");
                return EXIT_FAILURE;
            }
            struct pollfd input = { .fd = STDIN_FILENO, .events = POLLIN };
            const int available = poll(&input, 1, 100);
            if (available < 0) {
                if (errno == EINTR) continue;
                return EXIT_FAILURE;
            }
            if (available == 0) continue;
            if (fgets(command, sizeof(command), stdin) == NULL) break;
            command[strcspn(command, "\r\n")] = '\0';
            if (strcmp(command, "quit") == 0) break;
            if (strcmp(command, "status") == 0) {
                printf("{\"renderedFrames\":%llu,\"configurationChanges\":%llu,\"restarts\":%llu,\"running\":%s}\n",
                    (unsigned long long)atomic_load_explicit(rendered, memory_order_relaxed),
                    (unsigned long long)atomic_load_explicit(changes, memory_order_relaxed),
                    (unsigned long long)restarts,
                    engine.isRunning ? "true" : "false");
                fflush(stdout);
                continue;
            }
            if (strncmp(command, "route ", 6) != 0) {
                [engine stop];
                return EXIT_FAILURE;
            }
            const AudioDeviceID next = MRVirtualDevice(command + 6);
            if (next == kAudioObjectUnknown) {
                [engine stop];
                return EXIT_FAILURE;
            }
            [engine stop];
            if (AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                    kAudioUnitScope_Global, 0, &next, sizeof(next)) != noErr) return EXIT_FAILURE;
            actual = kAudioObjectUnknown;
            bytes = sizeof(actual);
            if (AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                    kAudioUnitScope_Global, 0, &actual, &bytes) != noErr || actual != next ||
                ![engine startAndReturnError:&error]) return EXIT_FAILURE;
            selected = next;
            printf("ready %s\n", command + 6);
            fflush(stdout);
        }
        [engine stop];
        [NSNotificationCenter.defaultCenter removeObserver:observer];
    }
    return EXIT_SUCCESS;
}
