#ifndef MR_HAL_CONTROL_PROTOCOL_H
#define MR_HAL_CONTROL_PROTOCOL_H

#include "MREndpointTransport.h"

#include <stdbool.h>
#include <stddef.h>
#include <xpc/xpc.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Engine application-source transport and wake service only.
 * HAL inventory/control connects to Driver; client PCM grants connect to Engine. */
#define MR_ENGINE_SERVICE_NAME "io.mixrelay.audio.engine"

enum {
    MR_HAL_CONTROL_PROTOCOL_VERSION = 1,
    MR_HAL_ENDPOINT_DISPLAY_NAME_MAX_BYTES = 128,
    /* Framing limit, not a silent truncation or a registry product policy. */
    MR_HAL_CONTROL_INVENTORY_MAX_ENTRIES = 64,
    MR_HAL_TRANSPORT_COMPATIBILITY_MAX_BUNDLES = 32,
};

typedef struct MRHALControlInventoryEntry {
    uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES];
    char display_name[MR_HAL_ENDPOINT_DISPLAY_NAME_MAX_BYTES];
    uint32_t sample_rate;
    int descriptor;
    size_t mapped_size;
    uint64_t generation;
} MRHALControlInventoryEntry;

typedef struct MRHALControlInventorySnapshot {
    uint64_t generation;
    size_t count;
    MRHALControlInventoryEntry entries[MR_HAL_CONTROL_INVENTORY_MAX_ENTRIES];
    /* Required v1 wire field; an empty list grants no bundle. Local ABI rebuilt together. */
    struct { size_t count; char bundles[MR_HAL_TRANSPORT_COMPATIBILITY_MAX_BUNDLES][MR_HAL_ENDPOINT_DISPLAY_NAME_MAX_BYTES]; } transport_compatibility;
} MRHALControlInventorySnapshot;

xpc_object_t mr_hal_control_create_inventory_request(void);
bool mr_hal_control_parse_inventory_request(xpc_object_t message);
/* Creators borrow caller descriptors. The XPC message retains its own copies. */
xpc_object_t mr_hal_control_create_inventory_snapshot(
    const MRHALControlInventoryEntry *entries, size_t count, uint64_t generation);
xpc_object_t mr_hal_control_create_inventory_snapshot_reply(
    xpc_object_t request, const MRHALControlInventoryEntry *entries,
    size_t count, uint64_t generation);
/* Output is unchanged on error, including when a later entry is malformed.
 * Success transfers duplicated descriptors to an empty/previously closed output.
 * Parsing validates framing/backing sizes; transport layout and product rate
 * support must all validate before a registry applies authoritative removals. */
bool mr_hal_control_parse_inventory_snapshot(
    xpc_object_t message, MRHALControlInventorySnapshot *snapshot);
void mr_hal_control_inventory_snapshot_close(MRHALControlInventorySnapshot *snapshot);

/* Typed removal intent. The registry, not the codec, checks current generation. */
xpc_object_t mr_hal_control_create_endpoint_remove(
    const uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES], uint64_t generation);
bool mr_hal_control_parse_endpoint_remove(
    xpc_object_t message, uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES],
    uint64_t *generation);

/* Replacement intent only. The Driver service compares expected_generation so repeated
 * requests cannot replace a newly published mapping a second time. */
xpc_object_t mr_hal_control_create_transport_replacement_request(
    const uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES], uint64_t expected_generation);
/* Outputs remain unchanged on malformed input. */
bool mr_hal_control_parse_transport_replacement_request(
    xpc_object_t message, uint8_t endpoint_id[MR_ENDPOINT_ID_BYTES],
    uint64_t *expected_generation);

#ifdef __cplusplus
}
#endif

#endif
