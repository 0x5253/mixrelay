#include "MRHALControlProtocol.h"
#include <xpc/xpc.h>

/* XPC requires a Blocks event handler even for a fire-and-forget wake message. */
xpc_connection_t mr_application_engine_wake(xpc_object_t request) {
    xpc_connection_t connection = xpc_connection_create_mach_service(
        MR_ENGINE_SERVICE_NAME, NULL, 0U);
    if (connection == NULL) return NULL;
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
    xpc_connection_activate(connection);
    xpc_connection_send_message(connection, request);
    return connection;
}

/* Blocks-based XPC connection ABI only. Rust owns requests, replies, decoded
 * descriptors, mapping validation and the producer lifetime/state machine. */
xpc_object_t mr_application_source_request(xpc_object_t request) {
    xpc_connection_t connection = xpc_connection_create_mach_service(
        MR_ENGINE_SERVICE_NAME, NULL, 0U);
    if (connection == NULL) return NULL;
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
    xpc_connection_activate(connection);
    xpc_object_t response = xpc_connection_send_message_with_reply_sync(connection, request);
    xpc_connection_cancel(connection);
    xpc_release(connection);
    if (response != NULL && xpc_get_type(response) == XPC_TYPE_ERROR) {
        xpc_release(response);
        return NULL;
    }
    return response;
}
