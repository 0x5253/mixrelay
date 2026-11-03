#import <Foundation/Foundation.h>
#include <stdbool.h>

bool mr_probe_with_activity(bool (*measure)(void *), void *context) {
    @autoreleasepool {
        NSProcessInfo *process = [NSProcessInfo processInfo];
        id activity = [[process beginActivityWithOptions:
            NSActivityUserInitiatedAllowingIdleSystemSleep | NSActivityLatencyCritical
            reason:@"Finite native audio measurement"] retain];
        if (activity == nil) return false;
        @try {
            return measure(context);
        } @finally {
            [process endActivity:activity];
            [activity release];
        }
    }
}
