#import <AVFAudio/AVFAudio.h>
#import <CoreAudio/CoreAudio.h>
#import <AudioUnit/AudioUnit.h>
#include <assert.h>

static AudioDeviceID observedDevice = 42;
static OSStatus MRTestGetProperty(AudioUnit unit, AudioUnitPropertyID property,
        AudioUnitScope scope, AudioUnitElement element, void *data, UInt32 *size) {
    (void)unit;
    assert(property == kAudioOutputUnitProperty_CurrentDevice);
    assert(scope == kAudioUnitScope_Global && element == 0);
    assert(*size == sizeof(AudioDeviceID));
    *(AudioDeviceID *)data = observedDevice;
    return noErr;
}

#define AudioUnitGetProperty MRTestGetProperty
#define main MRVirtualToneMain
#include "MRVirtualToneSource.m"
#undef main
#undef AudioUnitGetProperty

@interface MRStoppedEngine : NSObject
@property(nonatomic, getter=isRunning) BOOL running;
@property(nonatomic) BOOL startFails;
@property(nonatomic) unsigned starts;
- (BOOL)startAndReturnError:(NSError **)error;
@end
@implementation MRStoppedEngine
- (BOOL)startAndReturnError:(NSError **)error {
    (void)error;
    self.starts += 1;
    self.running = !self.startFails;
    return self.running;
}
@end

int main(void) {
    @autoreleasepool {
        MRStoppedEngine *fake = [MRStoppedEngine new];
        AVAudioEngine *engine = (AVAudioEngine *)fake;
        uint64_t handled = 0, restarts = 0;
        assert(MRResumeConfiguredEngine(engine, NULL, 42, 0, &handled, &restarts));
        assert(fake.starts == 0);
        assert(MRResumeConfiguredEngine(engine, NULL, 42, 1, &handled, &restarts));
        assert(fake.isRunning && fake.starts == 1 && restarts == 1);
        fake.running = NO;
        assert(MRResumeConfiguredEngine(engine, NULL, 42, 1, &handled, &restarts));
        assert(fake.starts == 1);
        observedDevice = 43;
        assert(!MRResumeConfiguredEngine(engine, NULL, 42, 2, &handled, &restarts));
        assert(fake.starts == 1);
        observedDevice = 42;
        fake.startFails = YES;
        assert(!MRResumeConfiguredEngine(engine, NULL, 42, 3, &handled, &restarts));
        assert(fake.starts == 2 && restarts == 1);
        fake.startFails = NO;
        restarts = 16;
        assert(!MRResumeConfiguredEngine(engine, NULL, 42, 4, &handled, &restarts));
        assert(fake.starts == 2);
    }
    return 0;
}
