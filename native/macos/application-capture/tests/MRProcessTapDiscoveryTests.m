#import "MRProcessTap.h"

#import <Foundation/Foundation.h>
#include <assert.h>
#include <stdio.h>

static void PrintApplication(
    void *context,
    const char *bundle_id,
    const char *display_name,
    const char *icon_data_url,
    int32_t process_id
) {
    size_t *count = context;
    *count += 1;
    printf(
        "%d\t%s\t%s\t%s\n",
        process_id,
        display_name,
        bundle_id,
        icon_data_url == NULL ? "no-icon" : "icon"
    );
}

int main(void) {
    @autoreleasepool {
        MRProcessTapError invalid = mr_process_tap_visit_applications(NULL, NULL);
        assert(invalid.code == MR_PROCESS_TAP_INVALID_ARGUMENT);

        size_t count = 0;
        MRProcessTapError result = mr_process_tap_visit_applications(
            PrintApplication,
            &count
        );
        assert(result.code == MR_PROCESS_TAP_OK);
        printf("capturable application processes: %zu\n", count);
    }
    return 0;
}
