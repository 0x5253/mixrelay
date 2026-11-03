#include "MRProbeAuthorization.h"
#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>
#include <string.h>

extern long mr_microphone_authorization_status(void);
extern void mr_microphone_request_authorization(void);

// Finish GUI consent before I/O; a nongraphical CLI retains CoreAudio's broker.
bool mr_authorize_input(double timeout, MRInputAuthorization *result) {
    result->initial = result->final = mr_microphone_authorization_status();
    result->requested = result->initial == 0;
    if (result->requested) {
        mach_timebase_info_data_t timebase;
        mach_timebase_info(&timebase);
        const uint64_t start = mach_absolute_time();
        mr_microphone_request_authorization();
        for (;;) {
            result->final = mr_microphone_authorization_status();
            if (result->final != 0) break;
            const double elapsed = (double)(mach_absolute_time() - start) *
                (double)timebase.numer / (double)timebase.denom / 1e9;
            if (elapsed >= timeout) break;
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
        }
    }
    if (result->final == 3 || result->final == 4) return true;
    fprintf(stderr, "input recording authorization was not granted before I/O (initial %ld, final %ld)\n",
            result->initial, result->final);
    return false;
}

void mr_print_input_authorization(FILE *stream, const MRInputAuthorization *result) {
    fprintf(stream, "\"inputAuthorization\":{\"initial\":%ld,\"final\":%ld,\"requested\":%s}",
            result->initial, result->final, result->requested ? "true" : "false");
}

// This command completes consent without enumerating, opening or running devices.
int mr_authorization_command(int argc, char **argv) {
    if (argc != 3 || strcmp(argv[1], "--authorize-input") != 0 || strcmp(argv[2], "--json") != 0)
        return -1;
    MRInputAuthorization authorization = {0};
    const bool granted = mr_authorize_input(30.0, &authorization);
    printf("{");
    mr_print_input_authorization(stdout, &authorization);
    printf("}\n");
    return granted ? 0 : 2;
}
