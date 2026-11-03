#ifndef MR_ENGINE_CONTROL_H
#define MR_ENGINE_CONTROL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <xpc/xpc.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MR_ENGINE_SOURCE_ID_MAX_BYTES = 128,
};

typedef struct MREngineSourceRequest {
    char source_id[MR_ENGINE_SOURCE_ID_MAX_BYTES];
    uint32_t capacity_frames;
    uint32_t channels;
    uint32_t transport_version;
} MREngineSourceRequest;

typedef struct MREngineSourceAttachment {
    int descriptor;
    size_t mapped_size;
    uint64_t producer_lease;
    uint64_t source_epoch;
} MREngineSourceAttachment;

/* Only the current MR_APPLICATION_CLOCK_VERSION is supported. */
xpc_object_t mr_engine_control_create_clocked_source_request(
    const char *source_id, uint32_t capacity_frames, uint32_t channels);
bool mr_engine_control_parse_source_request(
    xpc_object_t message,
    MREngineSourceRequest *request);

/* The creator borrows its FD; XPC retains a duplicate. Lease metadata is atomic
 * with the descriptor. Parsing leaves output unchanged on every error. */
xpc_object_t mr_engine_control_create_source_attachment_reply(
    xpc_object_t request, int descriptor, size_t mapped_size,
    uint64_t producer_lease, uint64_t source_epoch);
bool mr_engine_control_parse_source_attachment(
    xpc_object_t message, MREngineSourceAttachment *attachment);
void mr_engine_control_source_attachment_close(MREngineSourceAttachment *attachment);

#ifdef __cplusplus
}
#endif

#endif
