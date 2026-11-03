#import <Foundation/Foundation.h>
#import <WebKit/WebKit.h>
#include "MRWebKitPreferences.h"

@protocol MRWebKitFeature <NSObject>
- (NSString *)key;
@end

// WebKit SPI declarations match WKPreferencesPrivate.h and _WKFeature.h.
@interface WKPreferences (MRFeatureBridge)
+ (NSArray<id<MRWebKitFeature>> *)_features;
- (void)_setEnabled:(BOOL)enabled forFeature:(id<MRWebKitFeature>)feature;
- (BOOL)_isEnabledForFeature:(id<MRWebKitFeature>)feature;
@end

int32_t mr_webkit_set_bool_feature(void *raw, const char *key, bool enabled) {
    if (raw == NULL || key == NULL) return 1;
    @try {
        WKPreferences *preferences = (__bridge WKPreferences *)raw;
        if (![preferences isKindOfClass:WKPreferences.class]) return 1;
        if (![WKPreferences respondsToSelector:@selector(_features)] ||
            ![preferences respondsToSelector:@selector(_setEnabled:forFeature:)] ||
            ![preferences respondsToSelector:@selector(_isEnabledForFeature:)]) return 2;
        NSString *name = [NSString stringWithUTF8String:key];
        if (name == nil) return 1;
        for (id<MRWebKitFeature> feature in [WKPreferences _features]) {
            if (![feature respondsToSelector:@selector(key)] || ![[feature key] isEqualToString:name]) continue;
            [preferences _setEnabled:enabled forFeature:feature];
            return [preferences _isEnabledForFeature:feature] == enabled ? 0 : 4;
        }
        return 3;
    } @catch (NSException *exception) {
        (void)exception;
        return 5;
    }
}
