/* MixRelay-owned LV2 native-state fixture; MIT OR Apache-2.0, same as this adapter.
 * No installed plugin is modified. Compile inside a temporary fixture bundle. */
#include <lv2/core/lv2.h>
#include <lv2/atom/atom.h>
#include <lv2/state/state.h>
#include <lv2/urid/urid.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    unsigned char bytes[5];
    unsigned char asset[5];
    LV2_URID key, type, asset_key, path_type;
    const float *input, *gain;
    float *output;
} Fixture;

static LV2_Handle instantiate(const LV2_Descriptor *d, double rate, const char *path, const LV2_Feature *const *features) {
    (void)d; (void)rate; (void)path;
    const LV2_URID_Map *map = NULL;
    for (size_t i = 0; features && features[i]; ++i) if (!strcmp(features[i]->URI, LV2_URID__map)) map = features[i]->data;
    if (!map) return NULL;
    Fixture *f = calloc(1, sizeof(*f));
    if (!f) return NULL;
    memcpy(f->bytes, (unsigned char[]){1, 0, 255, 13, 0}, 5);
    memcpy(f->asset, (unsigned char[]){2, 4, 6, 8, 10}, 5);
    f->key = map->map(map->handle, "urn:mixrelay:fixture:opaque");
    f->type = map->map(map->handle, "urn:mixrelay:fixture:binary");
    f->asset_key = map->map(map->handle, "urn:mixrelay:fixture:file");
    f->path_type = map->map(map->handle, LV2_ATOM__Path);
    return f;
}
static void connect_port(LV2_Handle h, uint32_t p, void *v) {
    Fixture *f = h;
    if (p == 0) f->input = v;
    else if (p == 1) f->output = v;
    else if (p == 2) f->gain = v;
}
static void activate(LV2_Handle h) { (void)h; }
static void run(LV2_Handle h, uint32_t frames) {
    Fixture *f = h;
    if (!f->input || !f->output || !f->gain) return;
    for (uint32_t i = 0; i < frames; ++i) f->output[i] = f->input[i] * *f->gain;
}
static void deactivate(LV2_Handle h) { (void)h; }
static void cleanup(LV2_Handle h) { free(h); }

static LV2_State_Status save(LV2_Handle h, LV2_State_Store_Function store, LV2_State_Handle handle, uint32_t flags, const LV2_Feature *const *features) {
    (void)flags;
    Fixture *f = h;
    if (f->bytes[0] == 253) return LV2_STATE_ERR_UNKNOWN;

    const LV2_State_Map_Path *map_path = NULL;
    const LV2_State_Make_Path *make_path = NULL;
    const LV2_State_Free_Path *free_path = NULL;
    for (size_t i = 0; features && features[i]; ++i) {
        if (!strcmp(features[i]->URI, LV2_STATE__mapPath)) map_path = features[i]->data;
        else if (!strcmp(features[i]->URI, LV2_STATE__makePath)) make_path = features[i]->data;
        else if (!strcmp(features[i]->URI, LV2_STATE__freePath)) free_path = features[i]->data;
    }
    if (!map_path || !make_path || !free_path) return LV2_STATE_ERR_NO_FEATURE;

    char *path = make_path->path(make_path->handle, "fixture.bin");
    if (!path) return LV2_STATE_ERR_UNKNOWN;
    FILE *file = fopen(path, "wb");
    if (!file) {
        free_path->free_path(free_path->handle, path);
        return LV2_STATE_ERR_UNKNOWN;
    }
    const size_t written = fwrite(f->asset, 1, sizeof(f->asset), file);
    const int close_status = fclose(file);
    if (written != sizeof(f->asset) || close_status != 0) {
        free_path->free_path(free_path->handle, path);
        return LV2_STATE_ERR_UNKNOWN;
    }

    char *abstract_path = map_path->abstract_path(map_path->handle, path);
    free_path->free_path(free_path->handle, path);
    if (!abstract_path) return LV2_STATE_ERR_UNKNOWN;
    const LV2_State_Status path_status = store(handle, f->asset_key, abstract_path,
        strlen(abstract_path) + 1, f->path_type,
        LV2_STATE_IS_POD | LV2_STATE_IS_PORTABLE);
    free_path->free_path(free_path->handle, abstract_path);
    if (path_status != LV2_STATE_SUCCESS) return path_status;

    return store(handle, f->key, f->bytes, sizeof(f->bytes), f->type,
        LV2_STATE_IS_POD | LV2_STATE_IS_PORTABLE);
}

static LV2_State_Status restore(LV2_Handle h, LV2_State_Retrieve_Function retrieve, LV2_State_Handle handle, uint32_t flags, const LV2_Feature *const *features) {
    (void)flags;
    Fixture *f = h;
    size_t size = 0, path_size = 0;
    uint32_t type = 0, saved_flags = 0, path_type = 0, path_flags = 0;
    const unsigned char *bytes = retrieve(handle, f->key, &size, &type, &saved_flags);
    const char *abstract_path = retrieve(handle, f->asset_key, &path_size, &path_type, &path_flags);
    const uint32_t portable_flags = LV2_STATE_IS_POD | LV2_STATE_IS_PORTABLE;
    const char *path_terminator = NULL;
    if (abstract_path && path_size > 0 && path_size <= 1024) {
        path_terminator = memchr(abstract_path, '\0', path_size);
    }
    if (!bytes || size != sizeof(f->bytes) || type != f->type || bytes[0] == 254 ||
        (saved_flags & portable_flags) != portable_flags ||
        !abstract_path || path_size == 0 || path_size > 1024 ||
        path_type != f->path_type || (path_flags & portable_flags) != portable_flags ||
        !path_terminator || path_terminator != abstract_path + path_size - 1) {
        return LV2_STATE_ERR_UNKNOWN;
    }

    const LV2_State_Map_Path *map_path = NULL;
    const LV2_State_Free_Path *free_path = NULL;
    for (size_t i = 0; features && features[i]; ++i) {
        if (!strcmp(features[i]->URI, LV2_STATE__mapPath)) map_path = features[i]->data;
        else if (!strcmp(features[i]->URI, LV2_STATE__freePath)) free_path = features[i]->data;
    }
    if (!map_path || !free_path) return LV2_STATE_ERR_NO_FEATURE;

    char *path = map_path->absolute_path(map_path->handle, abstract_path);
    if (!path) return LV2_STATE_ERR_UNKNOWN;
    FILE *file = fopen(path, "rb");
    free_path->free_path(free_path->handle, path);
    if (!file) return LV2_STATE_ERR_UNKNOWN;

    unsigned char asset[sizeof(f->asset)];
    const size_t read = fread(asset, 1, sizeof(asset), file);
    const int trailing = fgetc(file);
    const int read_error = ferror(file);
    const int close_status = fclose(file);
    if (read != sizeof(asset) || trailing != EOF || read_error || close_status != 0) {
        return LV2_STATE_ERR_UNKNOWN;
    }

    memcpy(f->bytes, bytes, sizeof(f->bytes));
    memcpy(f->asset, asset, sizeof(f->asset));
    return LV2_STATE_SUCCESS;
}
static const LV2_State_Interface state = {save, restore};
static const void *extension_data(const char *uri) { return !strcmp(uri, LV2_STATE__interface) ? &state : NULL; }
static const LV2_Descriptor descriptor = {"urn:mixrelay:state-fixture", instantiate, connect_port, activate, run, deactivate, cleanup, extension_data};
LV2_SYMBOL_EXPORT const LV2_Descriptor *lv2_descriptor(uint32_t index) { return index == 0 ? &descriptor : NULL; }
