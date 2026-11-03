/* Clang supplies the nested XPC Block ABI; Rust owns dispatch resources. */
#include <xpc/xpc.h>

extern bool mr_driver_service_accept_peer(void *context, xpc_connection_t peer);
extern void mr_driver_service_peer_event(void *context, xpc_connection_t peer, xpc_object_t event);

void mr_driver_dispatch_install_discard_handler(xpc_connection_t listener) {
    xpc_connection_set_event_handler(listener, ^(xpc_object_t event) { (void)event; });
}

void mr_driver_dispatch_install_handler(xpc_connection_t listener, dispatch_queue_t queue, void *context) {
    xpc_connection_set_event_handler(listener, ^(xpc_object_t peer) {
        if (xpc_get_type(peer) != XPC_TYPE_CONNECTION) return;
        if (!mr_driver_service_accept_peer(context, peer)) { xpc_connection_cancel(peer); return; }
        xpc_connection_set_target_queue(peer, queue);
        xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
            mr_driver_service_peer_event(context, peer, event);
        });
        xpc_connection_activate(peer);
    });
}
