/* XPC exposes only a Blocks event registration API. The block captures no
 * owning object: Rust retains its control owner for the HAL process lifetime. */
#include <stdint.h>
#include <xpc/xpc.h>

void mr_hal_control_install_handler(xpc_connection_t connection, const void *owner,
    uint64_t generation, void (*event)(const void *, uint64_t, xpc_object_t)) {
    xpc_connection_set_event_handler(connection, ^(xpc_object_t message) {
        event(owner, generation, message);
    });
}
