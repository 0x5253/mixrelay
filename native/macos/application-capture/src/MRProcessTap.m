/* Objective-C compiler bridge only. Rust owns capture admission, native handles,
 * process selection, callbacks, reconciliation, and deferred resource retirement. */
#import "MRProcessTap.h"
#import <CoreAudio/AudioHardware.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>

static NSString *MRApplicationIconDataURL(NSImage *icon) {
    if (icon == nil) {
        return nil;
    }
    CGImageRef source = [icon CGImageForProposedRect:NULL context:nil hints:nil];
    if (source == NULL) {
        return nil;
    }

    static const size_t IconSize = 32;
    CGColorSpaceRef color_space = CGColorSpaceCreateDeviceRGB();
    if (color_space == NULL) {
        return nil;
    }
    const CGBitmapInfo bitmap_info =
        (CGBitmapInfo)kCGImageAlphaPremultipliedLast |
        (CGBitmapInfo)kCGBitmapByteOrder32Big;
    CGContextRef context = CGBitmapContextCreate(
        NULL,
        IconSize,
        IconSize,
        8,
        IconSize * 4,
        color_space,
        bitmap_info
    );
    CGColorSpaceRelease(color_space);
    if (context == NULL) {
        return nil;
    }
    CGContextSetInterpolationQuality(context, kCGInterpolationHigh);
    CGContextDrawImage(context, CGRectMake(0, 0, IconSize, IconSize), source);
    CGImageRef thumbnail = CGBitmapContextCreateImage(context);
    CGContextRelease(context);
    if (thumbnail == NULL) {
        return nil;
    }
    NSBitmapImageRep *representation = [[NSBitmapImageRep alloc] initWithCGImage:thumbnail];
    CGImageRelease(thumbnail);
    NSData *png = [representation representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
    if (png.length == 0) {
        return nil;
    }
    NSString *base64 = [png base64EncodedStringWithOptions:0];
    return [@"data:image/png;base64," stringByAppendingString:base64];
}

static NSImage *MRApplicationBundleIcon(NSURL *url) {
    if (url == nil) return nil;
    NSBundle *bundle = [NSBundle bundleWithURL:url];
    id name = [bundle objectForInfoDictionaryKey:@"CFBundleIconFile"];
    if (![name isKindOfClass:[NSString class]] || [name length] == 0 ||
        ![name isEqualToString:[name lastPathComponent]]) return nil;
    NSString *extension = [name pathExtension];
    NSURL *resource = [bundle URLForResource:[name stringByDeletingPathExtension]
        withExtension:extension.length == 0 ? @"icns" : extension];
    return resource == nil ? nil : [[NSImage alloc] initWithContentsOfURL:resource];
}

MRProcessTapError mr_process_tap_visit_applications(
    MRProcessTapApplicationVisitor visitor,
    void *context
) {
    if (visitor == NULL) {
        return ((MRProcessTapError){MR_PROCESS_TAP_INVALID_ARGUMENT, noErr});
    }
    for (NSRunningApplication *application in [NSWorkspace sharedWorkspace].runningApplications) {
        if (application.terminated ||
            application.activationPolicy != NSApplicationActivationPolicyRegular) {
            continue;
        }
        NSString *bundle_id = application.bundleIdentifier;
        NSURL *application_url = application.bundleURL;
        NSString *display_name = application.localizedName;
        if (display_name.length == 0 && application_url != nil) {
            display_name = [[NSFileManager defaultManager] displayNameAtPath:application_url.path];
        }
        const char *bundle_id_utf8 = bundle_id.UTF8String;
        const char *display_name_utf8 = display_name.UTF8String;
        if (bundle_id_utf8 != NULL && bundle_id_utf8[0] != '\0' &&
            display_name_utf8 != NULL && display_name_utf8[0] != '\0') {
            // NSRunningApplication.icon and NSWorkspace.iconForFile can wait
            // indefinitely for IconServices. Bundle image files need no daemon.
            NSImage *icon = MRApplicationBundleIcon(application_url);
            NSString *icon_data_url = MRApplicationIconDataURL(icon);
            visitor(
                context,
                bundle_id_utf8,
                display_name_utf8,
                icon_data_url.UTF8String,
                application.processIdentifier
            );
        }
    }
    return ((MRProcessTapError){MR_PROCESS_TAP_OK, noErr});
}


bool mr_tap_description_supports_bundle_selection(void) {
    return [CATapDescription instancesRespondToSelector:NSSelectorFromString(@"setBundleIDs:")];
}

bool mr_tap_description_supports_device_stream_selection(void) {
    return [CATapDescription instancesRespondToSelector:
        @selector(initWithProcesses:andDeviceUID:withStream:)];
}

void *mr_tap_description_create(const char *bundle, const AudioObjectID *processes,
    size_t count, const char *target_device_uid, bool select_bundle, bool mute, bool system_audio) {
    @autoreleasepool {
        NSMutableArray<NSNumber *> *ids = [NSMutableArray arrayWithCapacity:count];
        for (size_t index = 0; index < count; ++index) [ids addObject:@(processes[index])];
        CATapDescription *description;
        if (system_audio) {
            description = [[CATapDescription alloc] initStereoGlobalTapButExcludeProcesses:ids];
        } else if (target_device_uid != NULL) {
            if (count == 0 || !mr_tap_description_supports_device_stream_selection()) {
                return NULL;
            }
            NSString *uid = [NSString stringWithUTF8String:target_device_uid];
            if (uid == nil) {
                return NULL;
            }
            description = [[CATapDescription alloc] initWithProcesses:ids
                andDeviceUID:uid withStream:0];
            // The graph consumes stereo PCM. The stream-scoped tap must still
            // report stereo format before Rust admits it for capture.
            description.mixdown = YES;
            description.mono = NO;
        } else {
            description = [[CATapDescription alloc] initStereoMixdownOfProcesses:ids];
            if (select_bundle && mr_tap_description_supports_bundle_selection()) {
                [description setValue:@[[NSString stringWithUTF8String:bundle]] forKey:@"bundleIDs"];
            }
            if ([CATapDescription instancesRespondToSelector:NSSelectorFromString(@"setProcessRestoreEnabled:")]) {
                [description setValue:@YES forKey:@"processRestoreEnabled"];
            }
        }
        if (description == nil) {
            return NULL;
        }
        description.name = @"MixRelay Application Capture";
        description.private = YES;
        // Preparation does not suppress output. Only a committed owner reading
        // this exact device/process tap suppresses its original playback.
        description.muteBehavior = mute ? CATapMutedWhenTapped : CATapUnmuted;
        return (__bridge_retained void *)description;
    }
}

void mr_tap_description_release(void *description) {
    (void)(__bridge_transfer CATapDescription *)description;
}

OSStatus mr_tap_aggregate_create(void *raw_description, AudioObjectID *device) {
    @autoreleasepool {
        CATapDescription *description = (__bridge CATapDescription *)raw_description;
        NSString *(^key)(const char *) = ^NSString *(const char *value) {
            return [NSString stringWithUTF8String:value];
        };
        NSDictionary *tap = @{key(kAudioSubTapUIDKey): description.UUID.UUIDString};
        NSDictionary *aggregate = @{
            key(kAudioAggregateDeviceNameKey): @"MixRelay Application Capture",
            key(kAudioAggregateDeviceUIDKey): [@"io.mixrelay.capture." stringByAppendingString:NSUUID.UUID.UUIDString],
            key(kAudioAggregateDeviceIsPrivateKey): @YES,
            key(kAudioAggregateDeviceTapListKey): @[tap],
        };
        return AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggregate, device);
    }
}
