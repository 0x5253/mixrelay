#import "MRProcessTap.h"
#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <assert.h>

@interface MRFixtureApplication : NSObject
@property NSURL *bundleURL;
@end
@implementation MRFixtureApplication
- (BOOL)isTerminated { return NO; }
- (NSApplicationActivationPolicy)activationPolicy { return NSApplicationActivationPolicyRegular; }
- (NSString *)bundleIdentifier { return @"io.mixrelay.fixture"; }
- (NSString *)localizedName { return @"Fixture"; }
- (pid_t)processIdentifier { return 42; }
- (NSImage *)icon { [NSException raise:@"IconServicesMustNotBeCalled" format:@"application.icon"]; return nil; }
@end

static NSArray *applications;
static id RunningApplications(id object, SEL selector) { (void)object; (void)selector; return applications; }
static id ForbiddenIcon(id object, SEL selector, NSString *path) {
    (void)object; (void)selector;
    [NSException raise:@"IconServicesMustNotBeCalled" format:@"iconForFile:%@", path];
    return nil;
}
struct Observation { size_t count; bool expect_icon; };
static void Observe(void *context, const char *bundle, const char *name, const char *icon, int32_t pid) {
    struct Observation *observation = context;
    observation->count += 1;
    assert(strcmp(bundle, "io.mixrelay.fixture") == 0 && strcmp(name, "Fixture") == 0 && pid == 42);
    if (!observation->expect_icon) { assert(icon == NULL); return; }
    assert(icon != NULL);
    NSString *url = [NSString stringWithUTF8String:icon];
    assert([url hasPrefix:@"data:image/png;base64,"]);
    NSData *png = [[NSData alloc] initWithBase64EncodedString:[url componentsSeparatedByString:@","].lastObject options:0];
    NSBitmapImageRep *image = [[NSBitmapImageRep alloc] initWithData:png];
    assert(image.pixelsWide == 32 && image.pixelsHigh == 32);
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        assert(argc == 2);
        NSString *path = [NSString stringWithUTF8String:argv[1]];
        NSString *contents = [path stringByAppendingPathComponent:@"Fixture.app/Contents"];
        NSString *resources = [contents stringByAppendingPathComponent:@"Resources"];
        assert([[NSFileManager defaultManager] createDirectoryAtPath:resources withIntermediateDirectories:YES attributes:nil error:nil]);
        assert(([@{@"CFBundleIdentifier": @"io.mixrelay.fixture", @"CFBundleIconFile": @"Icon.png"}
            writeToFile:[contents stringByAppendingPathComponent:@"Info.plist"] atomically:YES]));
        NSBitmapImageRep *image = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:2 pixelsHigh:2
            bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:8 bitsPerPixel:32];
        memset(image.bitmapData, 255, 16);
        assert([[image representationUsingType:NSBitmapImageFileTypePNG properties:@{}]
            writeToFile:[resources stringByAppendingPathComponent:@"Icon.png"] atomically:YES]);
        MRFixtureApplication *application = [MRFixtureApplication new];
        application.bundleURL = [NSURL fileURLWithPath:[contents stringByDeletingLastPathComponent]];
        applications = @[application];
        method_setImplementation(class_getInstanceMethod([NSWorkspace class], @selector(runningApplications)), (IMP)RunningApplications);
        method_setImplementation(class_getInstanceMethod([NSWorkspace class], @selector(iconForFile:)), (IMP)ForbiddenIcon);
        struct Observation observation = {0, true};
        assert(mr_process_tap_visit_applications(Observe, &observation).code == MR_PROCESS_TAP_OK);
        assert(observation.count == 1);
        application.bundleURL = nil;
        observation = (struct Observation){0, false};
        assert(mr_process_tap_visit_applications(Observe, &observation).code == MR_PROCESS_TAP_OK);
        assert(observation.count == 1);
    }
    return 0;
}
