#import <AppKit/AppKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdio.h>

int main(void) {
    @autoreleasepool {
        NSImage *image = [NSWorkspace.sharedWorkspace iconForContentType:UTTypePlainText];
        NSData *pixels = image.TIFFRepresentation;
        if (pixels.length == 0) return 1;
        printf("%lu\n", (unsigned long)pixels.length);
    }
    return 0;
}
