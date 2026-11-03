/* Clang supplies the nested XPC Block ABI; Rust owns dispatch resources. */
#include <xpc/xpc.h>

extern bool mr_service_accept_peer(xpc_connection_t peer);
extern void mr_service_peer_event(xpc_connection_t peer, xpc_object_t event);

void mr_engine_dispatch_install_discard_handler(xpc_connection_t listener) {
    xpc_connection_set_event_handler(listener, ^(xpc_object_t event) { (void)event; });
}

void mr_engine_dispatch_install_handler(xpc_connection_t listener) {
    xpc_connection_set_event_handler(listener, ^(xpc_object_t peer) {
        if (xpc_get_type(peer) != XPC_TYPE_CONNECTION) return;
        if (!mr_service_accept_peer(peer)) { xpc_connection_cancel(peer); return; }
        xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
            mr_service_peer_event(peer, event);
        });
        xpc_connection_activate(peer);
    });
}
