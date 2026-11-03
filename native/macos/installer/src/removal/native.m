#include "../../MRInstallContract.h"
// Authorization is for one named removal operation. XPC authenticates both peers.
#import <Foundation/Foundation.h>
#import <Security/Authorization.h>
#import <Security/AuthorizationTags.h>
#import <Security/AuthorizationDB.h>
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <stdatomic.h>
#include <signal.h>

static const char *const MRRight = "io.mixrelay.driver.remove";
static const char *const MRService = "io.mixrelay.driver.removal";
static const uint64_t MRRequestTimeout = 300;
typedef struct {
    AuthorizationRef reference;
    AuthorizationExternalForm external;
} MRRemovalAuthorization;

int MRRemovalRightRegister(void) {
    AuthorizationRef reference = NULL;
    OSStatus status = AuthorizationCreate(NULL, NULL, kAuthorizationFlagDefaults, &reference);
    if (status != errAuthorizationSuccess) return status;
    NSURL *url = [NSURL fileURLWithPath:@MR_UNINSTALLER_PATH];
    CFBundleRef bundle = CFBundleCreate(NULL, (__bridge CFURLRef)url);
    // Credentials belong only to this authorization reference. A short transfer
    // window lets the helper verify its external form without asking again.
    NSDictionary *definition = @{ @"class": @"user", @"group": @"admin",
        @"authenticate-user": @YES, @"allow-root": @NO, @"shared": @NO,
        @"timeout": @300, @"comment": @"Remove MixRelay" };
    status = AuthorizationRightSet(reference, MRRight, (__bridge CFDictionaryRef)definition,
                                   CFSTR("Remove MixRelay"), bundle, CFSTR("Authorization"));
    if (bundle) CFRelease(bundle);
    AuthorizationFree(reference, kAuthorizationFlagDefaults);
    return status;
}

int MRRemovalRightRemove(void) {
    AuthorizationRef reference = NULL;
    OSStatus status = AuthorizationCreate(NULL, NULL, kAuthorizationFlagDefaults, &reference);
    if (status != errAuthorizationSuccess) return status;
    status = AuthorizationRightRemove(reference, MRRight);
    AuthorizationFree(reference, kAuthorizationFlagDefaults);
    // RightGet uses denied for an absent database entry. Do not suppress an
    // actual removal error: a retained policy is incomplete cleanup.
    if (status == errAuthorizationDenied) {
        CFDictionaryRef definition = NULL;
        OSStatus lookup = AuthorizationRightGet(MRRight, &definition);
        if (definition) CFRelease(definition);
        if (lookup == errAuthorizationDenied) return 0;
    }
    return status;
}

int MRRemovalAuthorize(const char *prompt, void **output) {
    *output = NULL;
    MRRemovalAuthorization *authorization = calloc(1, sizeof(*authorization));
    if (!authorization) return errAuthorizationInternal;
    OSStatus status = AuthorizationCreate(NULL, NULL, kAuthorizationFlagDefaults, &authorization->reference);
    if (status == errAuthorizationSuccess) {
        AuthorizationItem item = { MRRight, 0, NULL, 0 };
        AuthorizationRights rights = { 1, &item };
        AuthorizationItem context = { kAuthorizationEnvironmentPrompt, strlen(prompt), (void *)prompt, 0 };
        AuthorizationEnvironment environment = { 1, &context };
        status = AuthorizationCopyRights(authorization->reference, &rights, &environment,
            kAuthorizationFlagInteractionAllowed | kAuthorizationFlagExtendRights, NULL);
    }
    if (status == errAuthorizationSuccess)
        status = AuthorizationMakeExternalForm(authorization->reference, &authorization->external);
    if (status != errAuthorizationSuccess) {
        if (authorization->reference) AuthorizationFree(authorization->reference, kAuthorizationFlagDestroyRights);
        free(authorization);
        return status;
    }
    *output = authorization;
    return 0;
}

void MRRemovalAuthorizationFree(void *opaque) {
    MRRemovalAuthorization *authorization = opaque;
    if (!authorization) return;
    AuthorizationFree(authorization->reference, kAuthorizationFlagDestroyRights);
    memset(&authorization->external, 0, sizeof(authorization->external));
    free(authorization);
}

static NSString *MRRequirement(const char *team, const char *identifier) {
    return [NSString stringWithFormat:@"anchor apple generic and identifier \"%s\" and certificate leaf[subject.OU] = \"%s\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists", identifier, team];
}

// Must only be called after XPC has applied the connection's code-signing requirement.
static bool MRRemovalMessageAuthorized(xpc_object_t message) {
    if (xpc_get_type(message) != XPC_TYPE_DICTIONARY || xpc_dictionary_get_count(message) != 3)
        return false;
    xpc_object_t version = xpc_dictionary_get_value(message, "version");
    xpc_object_t selection = xpc_dictionary_get_value(message, "deleteSettings");
    xpc_object_t blob = xpc_dictionary_get_value(message, "authorization");
    if (!version || xpc_get_type(version) != XPC_TYPE_UINT64 || xpc_uint64_get_value(version) != 1 ||
        !selection || xpc_get_type(selection) != XPC_TYPE_BOOL ||
        !blob || xpc_get_type(blob) != XPC_TYPE_DATA) return false;
    size_t length = 0;
    const void *bytes = xpc_dictionary_get_data(message, "authorization", &length);
    if (!bytes || length != sizeof(AuthorizationExternalForm)) return false;
    AuthorizationExternalForm external;
    memcpy(&external, bytes, sizeof(external));
    AuthorizationRef reference = NULL;
    OSStatus status = AuthorizationCreateFromExternalForm(&external, &reference);
    memset(&external, 0, sizeof(external));
    if (status != errAuthorizationSuccess) return false;
    AuthorizationItem item = { MRRight, 0, NULL, 0 };
    AuthorizationRights rights = { 1, &item };
    // No interaction or extension: an unapproved request must never cause mutation.
    status = AuthorizationCopyRights(reference, &rights, NULL, kAuthorizationFlagDefaults, NULL);
    AuthorizationFree(reference, kAuthorizationFlagDefaults);
    return status == errAuthorizationSuccess;
}

int MRRemovalRequest(void *opaque, const char *team, bool deleteSettings) {
    MRRemovalAuthorization *authorization = opaque;
    if (!authorization) return 1;
    xpc_connection_t connection = xpc_connection_create_mach_service(MRService, NULL, XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!connection) return 1;
    NSString *requirement = MRRequirement(team, "mixrelay-macos-installer");
    if (xpc_connection_set_peer_code_signing_requirement(connection, requirement.UTF8String)) {
        xpc_connection_cancel(connection);
        return 1;
    }
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
    xpc_connection_activate(connection);
    xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_uint64(request, "version", 1);
    xpc_dictionary_set_bool(request, "deleteSettings", deleteSettings);
    xpc_dictionary_set_data(request, "authorization", &authorization->external, sizeof(authorization->external));
    dispatch_semaphore_t finished = dispatch_semaphore_create(0);
    __block atomic_int result;
    atomic_init(&result, 1);
    xpc_connection_send_message_with_reply(connection, request, dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^(xpc_object_t reply) {
        xpc_object_t status = xpc_get_type(reply) == XPC_TYPE_DICTIONARY ? xpc_dictionary_get_value(reply, "status") : NULL;
        if (xpc_connection_get_euid(connection) == 0 && xpc_get_type(reply) == XPC_TYPE_DICTIONARY &&
            xpc_dictionary_get_count(reply) == 1 &&
            status && xpc_get_type(status) == XPC_TYPE_INT64 &&
            xpc_dictionary_get_int64(reply, "status") == 0) {
            atomic_store(&result, 0);
            // A committed success reply precedes the helper's own bootout.
            xpc_object_t acknowledgement = xpc_dictionary_create(NULL, NULL, 0);
            xpc_dictionary_set_bool(acknowledgement, "finish", true);
            xpc_connection_send_message(connection, acknowledgement);
        }
        dispatch_semaphore_signal(finished);
    });
    long timeout = dispatch_semaphore_wait(finished, dispatch_time(DISPATCH_TIME_NOW, MRRequestTimeout * NSEC_PER_SEC));
    if (timeout) { xpc_connection_cancel(connection); return 1; }
    int completed = atomic_load(&result);
    if (completed) xpc_connection_cancel(connection);
    // Successful connections stay alive until acknowledgement is sent. The process
    // owns them until exit; the authorization reference is separately destroyed.
    return completed;
}

#include "MRRemovalSession.h"

typedef int (*MRRemovalCallback)(void *context, bool deleteSettings);
typedef void (*MRFinishCallback)(void *context);
typedef uint32_t (*MRRemovalTransition)(void *context, uint32_t event, uintptr_t peer, int32_t status);

static void MRRemovalApplyAction(uint32_t action, void *context, MRFinishCallback finish, MRFinishCallback idle) {
    if (action == MRRemovalFinish) {
        signal(SIGTERM, SIG_DFL);
        finish(context);
    } else if (action == MRRemovalIdle) {
        idle(context);
    }
}

int MRRemovalServer(const char *team, void *context, MRRemovalCallback remove,
                    MRFinishCallback finish, MRFinishCallback idle, MRRemovalTransition transition) {
    xpc_connection_t listener = xpc_connection_create_mach_service(MRService, NULL, XPC_CONNECTION_MACH_SERVICE_LISTENER);
    if (!listener) return 1;
    NSString *requirement = MRRequirement(team, "io.mixrelay.driver.uninstaller");
    dispatch_queue_t work = dispatch_queue_create("io.mixrelay.driver.removal.operation", DISPATCH_QUEUE_SERIAL);
    signal(SIGTERM, SIG_IGN);
    dispatch_source_t termination = dispatch_source_create(DISPATCH_SOURCE_TYPE_SIGNAL, SIGTERM, 0, work);
    dispatch_source_set_event_handler(termination, ^{
        MRRemovalApplyAction(transition(context, MRRemovalTerminated, 0, 0), context, finish, idle);
    });
    dispatch_resume(termination);
    xpc_connection_set_event_handler(listener, ^(xpc_object_t incoming) {
        if (xpc_get_type(incoming) != XPC_TYPE_CONNECTION) return;
        xpc_connection_t peer = incoming;
        if (xpc_connection_set_peer_code_signing_requirement(peer, requirement.UTF8String)) {
            xpc_connection_cancel(peer);
            return;
        }
        xpc_connection_set_target_queue(peer, work);
        xpc_connection_set_event_handler(peer, ^(xpc_object_t message) {
            uintptr_t identity = (uintptr_t)(__bridge void *)peer;
            if (message == XPC_ERROR_CONNECTION_INVALID) {
                MRRemovalApplyAction(transition(context, MRRemovalDisconnected, identity, 0), context, finish, idle);
                return;
            }
            if (xpc_get_type(message) != XPC_TYPE_DICTIONARY) return;
            xpc_object_t acknowledgement = xpc_dictionary_get_value(message, "finish");
            if (xpc_dictionary_get_count(message) == 1 && acknowledgement &&
                xpc_get_type(acknowledgement) == XPC_TYPE_BOOL && xpc_bool_get_value(acknowledgement)) {
                MRRemovalApplyAction(transition(context, MRRemovalAcknowledged, identity, 0), context, finish, idle);
                return;
            }
            if (transition(context, MRRemovalAdmission, identity, 0) != MRRemovalAllowRemoval) return;
            xpc_object_t reply = xpc_dictionary_create_reply(message);
            if (!reply) return;
            int status = 1;
            if (MRRemovalMessageAuthorized(message))
                status = remove(context, xpc_dictionary_get_bool(message, "deleteSettings"));
            uint32_t action = transition(context, MRRemovalTransaction, identity, status);
            NSLog(@"MixRelay removal: transaction result %d", status);
            xpc_dictionary_set_int64(reply, "status", status);
            xpc_connection_send_message(peer, reply);
            if (action == MRRemovalAwaitAcknowledgement) {
                // Keep the signed sender until its caller's reply deadline has passed.
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (MRRequestTimeout + 30) * NSEC_PER_SEC), work, ^{
                    MRRemovalApplyAction(transition(context, MRRemovalAbandoned, 0, 0), context, finish, idle);
                });
            } else if (action == MRRemovalAwaitIdle) {
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC), work, ^{
                    MRRemovalApplyAction(transition(context, MRRemovalIdleDeadline, 0, 0), context, finish, idle);
                });
            }
        });
        xpc_connection_activate(peer);
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC), work, ^{
        MRRemovalApplyAction(transition(context, MRRemovalIdleDeadline, 0, 0), context, finish, idle);
    });
    xpc_connection_activate(listener);
    dispatch_main();
}
