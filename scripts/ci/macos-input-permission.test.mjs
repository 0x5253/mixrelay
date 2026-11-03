import { MINIMUM_MACOS } from '../macos-minimum.mjs';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

test('finite measurement retains a latency activity only while I/O runs and releases it on failure', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-probe-activity-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'activity.m');
  writeFileSync(fixture, `
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <assert.h>
#include <stdbool.h>
extern bool mr_probe_with_activity(bool (*measure)(void *), void *context);
static id token;
static bool active, refuse;
static unsigned begins, ends, measurements;
static id begin(id receiver, SEL selector, NSActivityOptions options, NSString *reason) {
    assert(!active);
    assert((options & NSActivityLatencyCritical) == NSActivityLatencyCritical);
    assert((options & NSActivityUserInitiatedAllowingIdleSystemSleep) == NSActivityUserInitiatedAllowingIdleSystemSleep);
    assert(reason.length > 0);
    begins++;
    active = !refuse;
    return active ? token : nil;
}
static void end(id receiver, SEL selector, id activity) {
    assert(active && activity == token);
    active = false; ends++;
}
static bool measure(void *context) { assert(active); measurements++; return *(bool *)context; }
int main(void) {
    @autoreleasepool {
        token = [NSObject new];
        Class processClass = object_getClass([NSProcessInfo processInfo]);
        method_setImplementation(class_getInstanceMethod(processClass, @selector(beginActivityWithOptions:reason:)), (IMP)begin);
        method_setImplementation(class_getInstanceMethod(processClass, @selector(endActivity:)), (IMP)end);
        bool result = true;
        assert(mr_probe_with_activity(measure, &result));
        assert(!active && begins == 1 && ends == 1 && measurements == 1);
        result = false;
        assert(!mr_probe_with_activity(measure, &result));
        assert(!active && begins == 2 && ends == 2 && measurements == 2);
        refuse = true;
        assert(!mr_probe_with_activity(measure, &result));
        assert(!active && begins == 3 && ends == 2 && measurements == 2);
        [token release];
    }
}
`);
  const source = fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRProbeActivity.m', import.meta.url));
  const executable = join(directory, 'activity');
  execFileSync('xcrun', ['clang', `-mmacosx-version-min=${MINIMUM_MACOS}`, source, fixture,
    '-framework', 'Foundation', '-o', executable]);
  execFileSync(executable, [], { timeout: 10000 });
});

test('daemon consent does not query AVFoundation while GUI denial remains authoritative', (t) => {
  assert.equal(process.platform, 'darwin', 'Run the native permission boundary on macOS');
  const directory = mkdtempSync(join(tmpdir(), 'mr-input-consent-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'permission.m');
  writeFileSync(fixture, `
#import <AVFoundation/AVFoundation.h>
#import <Security/AuthSession.h>
#import <objc/runtime.h>
#include <assert.h>
extern long mr_microphone_authorization_status(void);
static SessionAttributeBits session_attributes;
static long status = AVAuthorizationStatusDenied;
static unsigned queries;
OSStatus mr_test_session_get_info(SecuritySessionId session, SecuritySessionId *identifier, SessionAttributeBits *attributes) {
    assert(session == callerSecuritySession);
    *attributes = session_attributes;
    return errSecSuccess;
}
static long fake_status(id receiver, SEL selector, id media) {
    queries++;
    return status;
}
int main(void) {
    @autoreleasepool {
        Method method = class_getClassMethod([AVCaptureDevice class], @selector(authorizationStatusForMediaType:));
        assert(method);
        method_setImplementation(method, (IMP)fake_status);
        session_attributes = sessionIsRoot;
        assert(mr_microphone_authorization_status() == 4);
        assert(queries == 0);
        session_attributes = sessionHasGraphicAccess;
        assert(mr_microphone_authorization_status() == AVAuthorizationStatusDenied);
        assert(queries == 1);
        status = AVAuthorizationStatusAuthorized;
        assert(mr_microphone_authorization_status() == AVAuthorizationStatusAuthorized);
        assert(queries == 2);
    }
}
`);
  const executable = join(directory, 'permission');
  const source = fileURLToPath(new URL('../../native/macos/service/src/MRMicrophonePermission.m', import.meta.url));
  execFileSync('xcrun', ['clang', '-fobjc-arc', '-fblocks', `-mmacosx-version-min=${MINIMUM_MACOS}`,
    '-DSessionGetInfo=mr_test_session_get_info', source, fixture,
    '-framework', 'AVFoundation', '-framework', 'Security', '-framework', 'Foundation', '-o', executable]);
  execFileSync(executable, [], { timeout: 10000 });
});

test('measurement consent completes before device preparation and denial or timeout never starts a probe', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-probe-consent-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRRoundTripProbe.c', import.meta.url));
  const fixture = join(directory, 'probe-consent.c');
  writeFileSync(fixture, `
#define main mr_probe_measurement_main
#include ${JSON.stringify(source)}
#undef main
#include <assert.h>
static long current, decision;
static unsigned queries, requests, measurements;
long mr_microphone_authorization_status(void) { queries++; return current; }
void mr_microphone_request_authorization(void) { requests++; if (decision >= 0) current = decision; }
bool mr_probe_with_activity(bool (*measure)(void *), void *context) { measurements++; return measure(context); }
static bool authorize(long initial, long answer, MRInputAuthorization *result) {
    current = initial; decision = answer; queries = requests = 0;
    return mr_authorize_input(0, result);
}
int main(void) {
    MRInputAuthorization result;
    assert(authorize(3, -1, &result));
    assert(queries == 1 && requests == 0 && !result.requested);
    for (long refused = 1; refused <= 2; refused++) {
        assert(!authorize(refused, -1, &result));
        assert(requests == 0 && result.final == refused);
    }
    assert(authorize(4, -1, &result));
    assert(requests == 0 && result.final == 4);
    assert(authorize(0, 3, &result));
    assert(queries == 2 && requests == 1 && result.requested && result.initial == 0 && result.final == 3);
    assert(!authorize(0, 2, &result));
    assert(requests == 1 && result.final == 2);
    assert(!authorize(0, -1, &result));
    assert(requests == 1 && result.final == 0);
    current = 2; requests = queries = 0;
    char *arguments[] = {"probe", "io.mixrelay.test-device-must-not-be-opened"};
    assert(mr_probe_measurement_main(2, arguments) == 2);
    assert(queries == 1 && requests == 0);
    assert(measurements == 0);
    char *consent[] = {"probe", "--authorize-input", "--json"};
    current = 3; requests = queries = 0;
    assert(mr_probe_measurement_main(3, consent) == 0);
    assert(queries == 1 && requests == 0 && measurements == 0);
    current = 0; decision = 3; requests = queries = 0;
    assert(mr_probe_measurement_main(3, consent) == 0);
    assert(queries == 2 && requests == 1 && measurements == 0);
    current = 2; requests = queries = 0;
    assert(mr_probe_measurement_main(3, consent) == 2);
    assert(queries == 1 && requests == 0 && measurements == 0);
    MROptions options = {0};
    char *help[] = {"probe", "--help"};
    assert(mr_options(2, help, &options) == 0);
    assert(queries == 1 && requests == 0);
    assert(measurements == 0);
}
`);
  const executable = join(directory, 'probe-consent');
  execFileSync('xcrun', ['clang', '-std=c17', '-O2', `-mmacosx-version-min=${MINIMUM_MACOS}`,
    '-Wall', '-Wextra', '-Wpedantic', '-Wconversion', '-Wsign-conversion', '-Wshadow', '-Werror',
    fixture, fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRProbeHalTrace.c', import.meta.url)),
    fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRProbeAuthorization.c', import.meta.url)),
    '-framework', 'CoreAudio', '-framework', 'CoreFoundation', '-o', executable]);
  execFileSync(executable, [], { timeout: 10000 });
});


test('HAL trace persistence accepts only the current native schema and exact extent', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-hal-trace-schema-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'trace.c');
  const source = fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRProbeHalTrace.c', import.meta.url));
  writeFileSync(fixture, `
#include ${JSON.stringify(fileURLToPath(new URL('../../native/macos/hal-driver/tests/MRProbeHalTrace.h', import.meta.url)))}
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static uint64_t words[17 + 8192 * 11] = {5, 10, 8192, 1};
static CFIndex payload_size = sizeof(words);
OSStatus AudioObjectGetPropertyDataSize(AudioObjectID object, const AudioObjectPropertyAddress *address,
    UInt32 qualifier_size, const void *qualifier, UInt32 *size) {
    (void)object; (void)address; (void)qualifier_size; (void)qualifier;
    *size = sizeof(CFDataRef); return noErr;
}
OSStatus AudioObjectGetPropertyData(AudioObjectID object, const AudioObjectPropertyAddress *address,
    UInt32 qualifier_size, const void *qualifier, UInt32 *size, void *value) {
    (void)object; (void)address; (void)qualifier_size; (void)qualifier;
    *(CFDataRef *)value = CFDataCreate(NULL, (const UInt8 *)words, payload_size);
    *size = sizeof(CFDataRef); return noErr;
}
OSStatus AudioObjectSetPropertyData(AudioObjectID object, const AudioObjectPropertyAddress *address,
    UInt32 qualifier_size, const void *qualifier, UInt32 size, const void *value) {
    (void)object; (void)address; (void)qualifier_size; (void)qualifier; (void)size; (void)value;
    return noErr;
}
int main(int argc, char **argv) {
    assert(argc == 2);
    for (uint64_t version = 1; version <= 6; version++) {
        if (version == 5) continue;
        words[0] = version;
        MRProbeHalTraceResult result = mr_probe_hal_trace_save(1, argv[1]);
        assert(result.stage && strcmp(result.stage, "hal_trace_payload_version") == 0);
        assert(access(argv[1], F_OK) != 0);
    }
    words[0] = 5;
    payload_size -= sizeof(uint64_t);
    assert(strcmp(mr_probe_hal_trace_save(1, argv[1]).stage, "hal_trace_payload_size") == 0);
    assert(access(argv[1], F_OK) != 0);
    payload_size += sizeof(uint64_t);
    words[5] = 1; words[14] = UINT64_C(1) << 33; words[15] = 1;
    words[8] = 1;
    words[17] = 1; words[19] = 100; words[20] = 8; words[24] = UINT64_C(0x1234567890abcdef);
    words[25] = 42; words[26] = 1000; words[27] = 1001;
    assert(mr_probe_hal_trace_save(1, argv[1]).stage == NULL);
    assert(strcmp(mr_probe_hal_trace_save(1, argv[1]).stage, "hal_trace_file_open") == 0);
}
`);
  const executable = join(directory, 'trace'), output = join(directory, 'trace.json');
  execFileSync('xcrun', ['clang', '-std=c17', '-O2', `-mmacosx-version-min=${MINIMUM_MACOS}`,
    '-Wall', '-Wextra', '-Wpedantic', '-Wconversion', '-Wsign-conversion', '-Wshadow', '-Werror',
    fixture, source, '-framework', 'CoreAudio', '-framework', 'CoreFoundation', '-o', executable]);
  execFileSync(executable, [output], { timeout: 10000 });
  const trace = JSON.parse(readFileSync(output, 'utf8'));
  assert.equal(trace.version, 5);
  assert.equal(trace.capacity, 8192);
  assert.equal(trace.admission.extent, 1);
  assert.equal(trace.admission.nullBuffer, 1);
  assert.deepEqual(trace.admission.lastRejectedExtent, { frames: 0, bufferPresent: false, render: true });
  assert.deepEqual(trace.rows, [[1, 0, 100, 8, 0, 0, 0, '1234567890abcdef', 42, 1000, 1001]]);
});
