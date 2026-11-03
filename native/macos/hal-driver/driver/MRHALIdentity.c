/* Best-effort presentation identity. Called only by Rust's control/resolver
 * paths, never by an audio callback. Sandbox denial is an ordinary miss. */
#include <CoreFoundation/CoreFoundation.h>
#include <libproc.h>
#include <os/log.h>
#include <dispatch/dispatch.h>
#include <sys/proc_info.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

bool mr_hal_identity_start(int32_t pid, uint64_t *seconds, uint64_t *micros) {
    struct proc_bsdinfo info = {0};
    if (pid <= 0 || proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info)) != sizeof(info)) return false;
    *seconds = info.pbi_start_tvsec;
    *micros = info.pbi_start_tvusec;
    return true;
}

/* The identifier of the innermost .app enclosing an executable path. Reads only the
 * bundle's Info.plist; nothing is launched. */
bool mr_hal_identity_bundle_at(const char *executable, char *output, size_t capacity) {
    char path[PROC_PIDPATHINFO_MAXSIZE] = {0};
    if (strlcpy(path, executable, sizeof(path)) >= sizeof(path)) return false;
    char *end = path + strlen(path);
    while (end > path) {
        *end = '\0';
        const size_t length = (size_t)(end - path);
        if (length >= 4 && strcmp(end - 4, ".app") == 0) {
            CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)path, (CFIndex)length, true);
            if (url == NULL) return false;
            CFBundleRef bundle = CFBundleCreate(NULL, url);
            CFRelease(url);
            if (bundle == NULL) return false;
            CFTypeRef value = CFBundleGetValueForInfoDictionaryKey(bundle, kCFBundleIdentifierKey);
            const bool found = value != NULL && CFGetTypeID(value) == CFStringGetTypeID()
                && CFStringGetCString(value, output, (CFIndex)capacity, kCFStringEncodingUTF8);
            CFRelease(bundle);
            return found;
        }
        end = strrchr(path, '/');
        if (end == NULL) break;
    }
    return false;
}

bool mr_hal_identity_bundle(int32_t pid, char *output, size_t capacity) {
    char path[PROC_PIDPATHINFO_MAXSIZE] = {0};
    if (proc_pidpath(pid, path, sizeof(path)) <= 0) return false;
    return mr_hal_identity_bundle_at(path, output, capacity);
}

void mr_hal_log_usb_transport(int32_t pid) {
    static os_log_t log;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ log = os_log_create("io.mixrelay.audio.driver", "transport"); });
    os_log(log, "presenting USB transport to pid %d", (int)pid);
}
