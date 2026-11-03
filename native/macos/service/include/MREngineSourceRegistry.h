#ifndef MR_ENGINE_SOURCE_REGISTRY_H
#define MR_ENGINE_SOURCE_REGISTRY_H

#include "MREngineControl.h"
#include "MRSharedAudioBuffer.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MR_ENGINE_MAX_APPLICATION_SOURCES = 64,
};

typedef struct MREngineSourceRegistry MREngineSourceRegistry;

typedef struct MRApplicationSourceClock {
    uint64_t epoch;
    uint64_t native_frames;
    uint32_t native_rate;
    uint32_t graph_rate;
    uint32_t running;
    uint32_t synchronized;
} MRApplicationSourceClock;

bool mr_engine_source_registry_clock(
    MREngineSourceRegistry *registry, uint64_t source_token, MRApplicationSourceClock *snapshot);
/* Single engine/control owner calls when choosing the native clock master. */
bool mr_engine_source_registry_set_clock_master(
    MREngineSourceRegistry *registry, uint64_t source_token, bool master);

MREngineSourceRegistry *mr_engine_source_registry_create(void);
void mr_engine_source_registry_destroy(MREngineSourceRegistry *registry);

/* A successful attachment transfers an owned descriptor to the caller. */
bool mr_engine_source_registry_attach_clocked(
    MREngineSourceRegistry *registry, const MREngineSourceRequest *request,
    int *descriptor, size_t *mapped_size, uint64_t *producer_lease, uint64_t *epoch);

uint64_t mr_engine_source_id_token(const char *source_id);
uint32_t mr_engine_source_registry_read_clocked(
    MREngineSourceRegistry *registry, uint64_t source_token, float *interleaved_samples,
    uint32_t frame_count, uint32_t channels, uint32_t sample_rate);
uint64_t mr_engine_source_registry_take_source_faults(
    const MREngineSourceRegistry *registry, uint64_t source_token);

#ifdef __cplusplus
}
#endif

#endif
