/* Lab-only substitution for comparing the two public CoreAudio registration APIs.
 * Loaded into the owned smoke child only; never linked into the product. */
#import <CoreAudio/CoreAudio.h>
#include <stdio.h>

static OSStatus MRBlockRegistration(AudioObjectID device, AudioDeviceIOProc callback,
    void *context, AudioDeviceIOProcID *registration) {
    fprintf(stderr, "registration comparison: block API begin device=%u\n", device);
    const OSStatus result = AudioDeviceCreateIOProcIDWithBlock(registration, device, NULL,
        ^(const AudioTimeStamp *now, const AudioBufferList *input,
          const AudioTimeStamp *input_time, AudioBufferList *output,
          const AudioTimeStamp *output_time) {
            (void)callback(device, now, input, input_time, output, output_time, context);
        });
    fprintf(stderr, "registration comparison: block API end status=%d\n", result);
    return result;
}

typedef OSStatus (*MRRegistration)(AudioObjectID, AudioDeviceIOProc, void *, AudioDeviceIOProcID *);
__attribute__((used, section("__DATA,__interpose")))
static const struct { MRRegistration replacement, original; } MRInterposition = {
    MRBlockRegistration, AudioDeviceCreateIOProcID,
};
