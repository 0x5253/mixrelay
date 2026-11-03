/* MixRelay-owned LV2 bus-routing fixture; MIT OR Apache-2.0, same as this adapter.
 * Build only in an isolated fixture bundle. Native code provides test PCM and state. */
#include <lv2/core/lv2.h>
#include <lv2/state/state.h>
#include <lv2/urid/urid.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    float *ports[6];
    float bias;
    LV2_URID key;
    LV2_URID type;
} Fixture;

static LV2_Handle instantiate(const LV2_Descriptor *d, double rate, const char *path,
                              const LV2_Feature *const *features) {
    (void)d; (void)rate; (void)path;
    const LV2_URID_Map *map = NULL;
    for (size_t i = 0; features && features[i]; ++i) {
        if (!strcmp(features[i]->URI, LV2_URID__map)) map = features[i]->data;
    }
    if (!map) return NULL;
    Fixture *f = calloc(1, sizeof(*f));
    if (!f) return NULL;
    f->bias = 1.0f;
    f->key = map->map(map->handle, "urn:mixrelay:multibus-fixture:bias");
    f->type = map->map(map->handle, "urn:mixrelay:multibus-fixture:float");
    return f;
}
static void connect_port(LV2_Handle h, uint32_t index, void *data) {
    if (index < 6) ((Fixture *)h)->ports[index] = data;
}
static void activate(LV2_Handle h) { (void)h; }
static void run(LV2_Handle h, uint32_t frames) {
    Fixture *f = h;
    for (uint32_t i = 0; i < frames; ++i) {
        const float left = f->ports[0] ? f->ports[0][i] : 0.0f;
        const float side = f->ports[1] ? f->ports[1][i] : 0.0f;
        const float right = f->ports[2] ? f->ports[2][i] : 0.0f;
        if (f->ports[3]) f->ports[3][i] = left + 10.0f * side + f->bias;
        if (f->ports[4]) f->ports[4][i] = left + right + 30.0f * side + f->bias;
        if (f->ports[5]) f->ports[5][i] = right + 20.0f * side + f->bias;
    }
    f->bias += 1.0f;
}
static void deactivate(LV2_Handle h) { (void)h; }
static void cleanup(LV2_Handle h) { free(h); }
static LV2_State_Status save(LV2_Handle h, LV2_State_Store_Function store,
                             LV2_State_Handle handle, uint32_t flags,
                             const LV2_Feature *const *features) {
    (void)flags; (void)features;
    Fixture *f = h;
    return store(handle, f->key, &f->bias, sizeof(f->bias), f->type,
                 LV2_STATE_IS_POD | LV2_STATE_IS_PORTABLE);
}
static LV2_State_Status restore(LV2_Handle h, LV2_State_Retrieve_Function retrieve,
                                LV2_State_Handle handle, uint32_t flags,
                                const LV2_Feature *const *features) {
    (void)flags; (void)features;
    Fixture *f = h;
    size_t size = 0;
    uint32_t type = 0, stored_flags = 0;
    const void *data = retrieve(handle, f->key, &size, &type, &stored_flags);
    if (!data || size != sizeof(f->bias) || type != f->type) return LV2_STATE_ERR_UNKNOWN;
    memcpy(&f->bias, data, sizeof(f->bias));
    return LV2_STATE_SUCCESS;
}
static const LV2_State_Interface state = {save, restore};
static const void *extension_data(const char *uri) {
    return !strcmp(uri, LV2_STATE__interface) ? &state : NULL;
}
static const LV2_Descriptor descriptor = {
    "urn:mixrelay:multibus-fixture", instantiate, connect_port, activate, run,
    deactivate, cleanup, extension_data
};
LV2_SYMBOL_EXPORT const LV2_Descriptor *lv2_descriptor(uint32_t index) {
    return index == 0 ? &descriptor : NULL;
}
