#import <AVFAudio/AVFAudio.h>
#import <Foundation/Foundation.h>
#include <stdlib.h>
#include <unistd.h>

int main(void) {
    @autoreleasepool {
        AVAudioEngine *engine = [[AVAudioEngine alloc] init];
        AVAudioFormat *format = [[AVAudioFormat alloc]
            initStandardFormatWithSampleRate:48000
            channels:2
        ];
        __block float phase = 0;
        AVAudioSourceNode *source = [[AVAudioSourceNode alloc]
            initWithFormat:format
            renderBlock:^OSStatus(
                BOOL *is_silence,
                const AudioTimeStamp *timestamp,
                AVAudioFrameCount frame_count,
                AudioBufferList *output_data
            ) {
                (void)is_silence;
                (void)timestamp;
                for (AVAudioFrameCount frame = 0; frame < frame_count; ++frame) {
                    phase += 440.0F / 48000.0F;
                    phase = phase >= 1 ? phase - 1 : phase;
                    const float sample = (phase * 2 - 1) * 0.05F;
                    for (UInt32 buffer = 0; buffer < output_data->mNumberBuffers; ++buffer) {
                        ((float *)output_data->mBuffers[buffer].mData)[frame] = sample;
                    }
                }
                return noErr;
            }
        ];
        [engine attachNode:source];
        [engine connect:source to:engine.mainMixerNode format:format];
        [engine prepare];
        NSError *error = nil;
        if (![engine startAndReturnError:&error]) {
            fprintf(stderr, "tone source failed: %s\n", error.localizedDescription.UTF8String);
            return EXIT_FAILURE;
        }
        sleep(5);
        [engine stop];
    }
    return EXIT_SUCCESS;
}
