// Test the real listener's SIGTERM cleanup in a temporary user launchd job.
// No system job, authorization request or installed Driver file is touched.
#import "../src/removal/native.m"
#include <unistd.h>

static const char *executable;
static const char *evidence;
static int reject(void *context, bool selected) { (void)context; (void)selected; return 1; }
static void cleanup(void *context) {
    (void)context;
    unlink(executable);
    [@"idle cleanup observed" writeToFile:[NSString stringWithUTF8String:evidence]
        atomically:YES encoding:NSUTF8StringEncoding error:NULL];
    exit(0);
}

static uint32_t transition(void *context, uint32_t event, uintptr_t peer, int32_t status) {
    (void)context; (void)peer; (void)status;
    return event == MRRemovalTerminated ? MRRemovalIdle : MRRemovalIgnore;
}

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    executable = argv[1]; evidence = argv[2];
    const char *ready = argv[3];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC / 10), dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
        [@"listener ready" writeToFile:[NSString stringWithUTF8String:ready]
            atomically:YES encoding:NSUTF8StringEncoding error:NULL];
    });
    return MRRemovalServer("AB12C3D4E5", NULL, reject, cleanup, cleanup, transition);
}
