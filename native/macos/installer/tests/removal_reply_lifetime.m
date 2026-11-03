// Exercise real XPC peer signing across removal of an installed executable.
// Runs only in the current user's isolated launchd job; no Driver/root mutation.
#import <Foundation/Foundation.h>
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>

#ifndef MR_FIXTURE_REQUIREMENT
#define MR_FIXTURE_REQUIREMENT "identifier \"io.mixrelay.removal.fixture\""
#endif
static const char *requirement = MR_FIXTURE_REQUIREMENT;

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc == 7 && strcmp(argv[1], "server") == 0) {
            if (strcmp(argv[5], "handoff") == 0) {
                // Preserve launchd's PID while moving execution off the file
                // this transaction will delete, as the production helper does.
                execl(argv[4], argv[4], "server", argv[2], argv[3], argv[4], "private", argv[6], NULL);
                return 2;
            }
            const char *installed = argv[3];
            const char *running = argv[4];
            const char *client = argv[6];
            bool staged = strcmp(argv[5], "private") == 0;
            xpc_connection_t listener = xpc_connection_create_mach_service(argv[2], NULL, XPC_CONNECTION_MACH_SERVICE_LISTENER);
            if (!listener) return 3;
            xpc_connection_set_event_handler(listener, ^(xpc_object_t incoming) {
                if (xpc_get_type(incoming) != XPC_TYPE_CONNECTION) return;
                xpc_connection_t peer = incoming;
                if (xpc_connection_set_peer_code_signing_requirement(peer, requirement)) exit(4);
                xpc_connection_set_event_handler(peer, ^(xpc_object_t request) {
                    if (xpc_get_type(request) != XPC_TYPE_DICTIONARY) return;
                    if (xpc_dictionary_get_bool(request, "finish")) {
                        if (staged) unlink(running);
                        NSString *marker = [[NSString stringWithUTF8String:running] stringByAppendingString:@".ack"];
                        [@"signed acknowledgement accepted" writeToFile:marker atomically:YES encoding:NSUTF8StringEncoding error:NULL];
                        exit(0);
                    }
                    if (unlink(installed)) exit(5);
                    if (unlink(client)) exit(11);
                    xpc_object_t reply = xpc_dictionary_create_reply(request);
                    if (!reply) exit(6);
                    xpc_dictionary_set_int64(reply, "status", 0);
                    xpc_connection_send_message(peer, reply);
                });
                xpc_connection_activate(peer);
            });
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 15 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ exit(7); });
            xpc_connection_activate(listener);
            dispatch_main();
        }
        if (argc != 6 || strcmp(argv[1], "client")) return 8;
        if (strcmp(argv[5], "handoff") == 0) {
            execl(argv[4], argv[4], "client", argv[2], argv[3], argv[4], "private", NULL);
            return 12;
        }
        xpc_connection_t connection = xpc_connection_create_mach_service(argv[2], NULL, 0);
        if (xpc_connection_set_peer_code_signing_requirement(connection, requirement)) return 9;
        xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
        xpc_connection_activate(connection);
        xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
        xpc_dictionary_set_bool(request, "remove", true);
        dispatch_semaphore_t finished = dispatch_semaphore_create(0);
        __block int result = 1;
        xpc_connection_send_message_with_reply(connection, request, dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^(xpc_object_t reply) {
            if (xpc_get_type(reply) == XPC_TYPE_DICTIONARY && xpc_dictionary_get_int64(reply, "status") == 0) {
                result = 0;
                xpc_object_t acknowledgement = xpc_dictionary_create(NULL, NULL, 0);
                xpc_dictionary_set_bool(acknowledgement, "finish", true);
                xpc_connection_send_message(connection, acknowledgement);
                xpc_connection_send_barrier(connection, ^{ dispatch_semaphore_signal(finished); });
            } else {
                dispatch_semaphore_signal(finished);
            }
        });
        if (dispatch_semaphore_wait(finished, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC))) return 10;
        puts(result == 0 ? "signed success reply accepted" : "deleted sender rejected");
        return result;
    }
}
