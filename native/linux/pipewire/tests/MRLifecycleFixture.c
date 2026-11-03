/* Owned isolated-daemon regression. Include the production implementation only
 * to drive pw_stream_set_active on this fixture's private stream, not user nodes. */
#include "../MRPipeWire.c"
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stddef.h>
#include <time.h>

typedef struct {
    MRPWStream *source, *observer;
    _Atomic bool generate;
    _Atomic uint64_t received, stale;
    _Atomic uint64_t idle_callbacks;
} Fixture;

static void tick(void *data, const MRPWCycle *cycle) {
    Fixture *f = data;
    if (cycle->frames > 2048) return;
    float samples[2048 * 2];
    bool generate = atomic_load(&f->generate);
    if (generate) {
        for (uint32_t n = 0; n < cycle->frames * 2; n++) samples[n] = 0.125f;
        mr_pw_stream_write(f->source, samples, cycle->frames);
    }
    uint32_t received = mr_pw_stream_read(f->observer, samples, cycle->frames);
    for (uint32_t n = 0; n < received * 2; n++) {
        if (fabsf(samples[n]) > 0.25f) atomic_fetch_add(&f->stale, 1);
        if (fabsf(samples[n]) > 0.05f) atomic_fetch_add(&f->received, 1);
    }
    if (!generate) atomic_fetch_add(&f->idle_callbacks, 1);
    mr_pw_stream_trigger_output(f->source);
}
static void delay(void) {
    const struct timespec pause = {.tv_nsec = 1000000};
    nanosleep(&pause, NULL);
}
typedef struct {
    uint32_t node_id;
    char serial[32];
} FixtureIdentity;
static void registry_global(void *data, uint32_t id, uint32_t permissions,
                            const char *type, uint32_t version, const struct spa_dict *props) {
    (void)permissions; (void)version;
    FixtureIdentity *identity = data;
    if (id != identity->node_id || !spa_streq(type, PW_TYPE_INTERFACE_Node)) return;
    const char *serial = props ? spa_dict_lookup(props, PW_KEY_OBJECT_SERIAL) : NULL;
    assert(serial && strlen(serial) < sizeof(identity->serial));
    strcpy(identity->serial, serial);
}
static const struct pw_registry_events registry_events = {
    PW_VERSION_REGISTRY_EVENTS, .global = registry_global,
};
static void source_serial(MRPWStream *source, char result[32]) {
    /* object.serial belongs to the server registry, not local stream properties.
     * Keep lookup bound to this actual node ID rather than a name/default. */
    pw_thread_loop_lock(source->loop);
    FixtureIdentity identity = { .node_id = pw_stream_get_node_id(source->stream) };
    assert(identity.node_id != PW_ID_ANY);
    struct pw_registry *registry = pw_core_get_registry(pw_stream_get_core(source->stream), PW_VERSION_REGISTRY, 0);
    assert(registry);
    struct spa_hook listener = {0};
    pw_registry_add_listener(registry, &listener, &registry_events, &identity);
    pw_thread_loop_unlock(source->loop);
    for (unsigned attempt = 0; attempt < 3000; attempt++) {
        pw_thread_loop_lock(source->loop);
        bool ready = identity.serial[0] != 0;
        pw_thread_loop_unlock(source->loop);
        if (ready) break;
        delay();
    }
    pw_thread_loop_lock(source->loop);
    assert(identity.serial[0]);
    strcpy(result, identity.serial);
    spa_hook_remove(&listener);
    pw_proxy_destroy((struct pw_proxy *)registry);
    pw_thread_loop_unlock(source->loop);
}
static void active(MRPWStream *s, bool value) {
    pw_thread_loop_lock(s->loop);
    assert(pw_stream_set_active(s->stream, value) == 0);
    /* set_active controls scheduling, not the client stream's link-state.
     * Exercise the actual SPA lifecycle event on this owned node explicitly. */
    struct pw_registry *registry = pw_core_get_registry(pw_stream_get_core(s->stream), PW_VERSION_REGISTRY, 0);
    assert(registry);
    struct pw_node *node = pw_registry_bind(registry, pw_stream_get_node_id(s->stream),
                                           PW_TYPE_INTERFACE_Node, PW_VERSION_NODE, 0);
    assert(node);
    struct spa_command command = SPA_NODE_COMMAND_INIT(value ? SPA_NODE_COMMAND_Start : SPA_NODE_COMMAND_Pause);
    assert(pw_node_send_command(node, &command) >= 0);
    pw_proxy_destroy((struct pw_proxy *)node);
    pw_proxy_destroy((struct pw_proxy *)registry);
    pw_thread_loop_unlock(s->loop);
    int expected = value ? PW_STREAM_STATE_STREAMING : PW_STREAM_STATE_PAUSED;
    for (unsigned n = 0; n < 3000; n++) {
        if (atomic_load(&s->state) == expected) return;
        delay();
    }
    fprintf(stderr, "lifecycle transition active=%d expected_state=%d observed_state=%d negotiated_format=%llu\n",
            value, expected, atomic_load(&s->state), (unsigned long long)mr_pw_stream_format(s));
    assert(!"owned stream did not transition state");
}
static void transfer_counter_contract(void) {
    _Static_assert(offsetof(MRPWTransferSnapshot, failures) == 24, "snapshot stage offset");
    _Static_assert(offsetof(MRPWRejectedTransfer, first_clock) == 32, "failure clock offset");
    _Static_assert(sizeof(MRPWTransferCounters) == 16, "normal report ABI unchanged");
    _Static_assert(sizeof(MRPcmRing) == 120, "shared ring owner ABI unchanged");
    _Static_assert(offsetof(MRPWTransferSnapshot, engine_read_frames) == 216, "delivery offset");
    _Static_assert(offsetof(MRPWTransferSnapshot, native_restart_calls) == 240, "restart offset");
    _Static_assert(offsetof(MRPWTransferSnapshot, native_output_trigger_rejections) == 248, "trigger offset");
    MRPWStream s = {0};
    initialize_transfer_observations(&s);
    atomic_init(&s.clock_snapshot, (UINT64_C(7) << 32) | 48000);
    assert(mr_pcm_ring_init(&s.ring, 2, 512));
    for (unsigned stage = 0; stage < 4; ++stage) {
        atomic_store(&s.position_snapshot, 0);
        record_rejected(&s, stage, 0, 0);
        record_rejected(&s, stage, 128, 128);
        assert(mr_pw_stream_transfer_snapshot(&s).failures[stage].rejected_calls == 0);
        record_rejected(&s, stage, 128, 0);
        atomic_store(&s.position_snapshot, 256);
        record_rejected(&s, stage, 128, 64);
        restart_stream_ring(&s);
        assert(mr_pw_stream_transfer_snapshot(&s).native_restart_calls == stage + 1);
        atomic_store(&s.position_snapshot, 512);
        record_rejected(&s, stage, 128, 0);
        MRPWRejectedTransfer failure = mr_pw_stream_transfer_snapshot(&s).failures[stage];
        assert(failure.rejected_calls == 3 && failure.rejected_frames == 320);
        assert(failure.first_position == 0 && failure.last_position == 512);
        assert(failure.first_clock == ((UINT64_C(7) << 32) | 48000));
        assert(failure.last_clock == failure.first_clock);
        atomic_store(&s.failures[stage].rejected_calls, UINT64_MAX);
        atomic_store(&s.failures[stage].rejected_frames, UINT64_MAX - 63);
        record_rejected(&s, stage, 128, 64);
        failure = mr_pw_stream_transfer_snapshot(&s).failures[stage];
        assert(failure.rejected_calls == 0 && failure.rejected_frames == 0);
        assert(failure.rejected_calls - UINT64_MAX == 1);
        assert(failure.rejected_frames - (UINT64_MAX - 63) == 64);
        assert(failure.first_position == 0);
    }
    mr_pcm_ring_clear(&s.ring);

    MRPWStream ingress = {.channels = 2};
    initialize_transfer_observations(&ingress);
    atomic_init(&ingress.clock_snapshot, (UINT64_C(7) << 32) | 48000);
    atomic_init(&ingress.format_valid, true);
    atomic_init(&ingress.format_snapshot, (uint64_t)48000 << 32 | (MR_PCM_FORMAT_FLOAT32 << 16) | 2);
    assert(mr_pw_stream_format(&ingress) == ((uint64_t)48000 << 32 | 2));
    atomic_store(&ingress.position_snapshot, 640);
    assert(mr_pcm_ring_init(&ingress.ring, 2, 512));
    float pcm[8] = {0};
    struct spa_chunk chunk = {.offset = 2, .size = 16, .stride = 8};
    struct spa_data data = {.data = pcm, .maxsize = sizeof(pcm), .chunk = &chunk};
    capture_buffer(&ingress, &data, 128); /* Misaligned offset discards two known frames. */
    chunk.offset = 0;
    chunk.size = 15;
    capture_buffer(&ingress, &data, 128); /* Partial-frame geometry has unknown frame count. */
    chunk.size = 0;
    capture_buffer(&ingress, &data, 128); /* Valid empty chunk is ordinary no-data. */
    MRPWTransferSnapshot observation = mr_pw_stream_transfer_snapshot(&ingress);
    assert(observation.failures[0].rejected_calls == 2);
    assert(observation.failures[0].rejected_frames == 2);
    assert(observation.read == 0 && observation.written == 0);
    record_capture_ingress_discard(&ingress, NULL); /* Missing required descriptor: unknown. */
    chunk.size = 16;
    data.data = NULL;
    record_capture_ingress_discard(&ingress, &data); /* Known chunk cannot be copied. */
    atomic_store(&ingress.format_valid, false);
    assert(mr_pw_stream_format(&ingress) == 0);
    atomic_store(&ingress.position_snapshot, 1280);
    record_capture_ingress_discard(&ingress, &data); /* Invalid format: frame geometry unknown. */
    observation = mr_pw_stream_transfer_snapshot(&ingress);
    assert(observation.failures[0].rejected_calls == 5);
    assert(observation.failures[0].rejected_frames == 4);
    assert(observation.failures[0].first_position == 640);
    assert(observation.failures[0].last_position == 1280);
    assert(observation.failures[0].first_clock == ((UINT64_C(7) << 32) | 48000));
    assert(mr_pw_stream_capture_failures(&ingress) == 5);
    record_rejected(&ingress, MR_NATIVE_OUTPUT_READ, 128, 0);
    record_rejected(&ingress, MR_ENGINE_CAPTURE_READ, 128, 0);
    record_rejected(&ingress, MR_ENGINE_OUTPUT_WRITE, 128, 0);
    assert(mr_pw_stream_capture_failures(&ingress) == 5);
    assert(mr_pw_stream_capture_failures(&ingress) == 5); /* Observing never consumes. */
    ingress.native_output = true;
    record_capture_ingress_discard(&ingress, NULL);
    assert(mr_pw_stream_transfer_snapshot(&ingress).failures[0].rejected_calls == 5);
    assert(mr_pw_stream_capture_failures(&ingress) == 0);
    mr_pcm_ring_clear(&ingress.ring);
}

static void native_format_advertisement(void) {
    for (unsigned virtual_endpoint = 0; virtual_endpoint < 2; virtual_endpoint++) {
        MRPWStream s = {.channels = SPA_AUDIO_MAX_CHANNELS, .sample_rate = 96000};
        uint8_t storage[2048];
        struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        const struct spa_pod *params[4];
        unsigned count = build_stream_formats(&s, &builder, params, virtual_endpoint, 0);
        assert(count == (virtual_endpoint ? 4 : 1));
        assert(mr_pw_stream_supported_formats(&s) == (virtual_endpoint ? 15 : 1));
        for (unsigned i = 0; i < count; i++) {
            struct spa_audio_info_raw info = {0};
            assert(spa_format_audio_raw_parse(params[i], &info) >= 0);
            assert(pcm_format(info.format) == i + 1);
            assert(info.rate == 96000 && info.channels == SPA_AUDIO_MAX_CHANNELS);
        }
    }
}

static void native_format_pcm(uint32_t encoding, enum spa_audio_format format) {
    MRPWStream s = {.channels = 2, .sample_rate = 48000, .supported_formats = 15};
    initialize_transfer_observations(&s);
    assert(mr_pcm_ring_init(&s.ring, 2, 512));
    uint8_t storage[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = {.format = format, .rate = 48000, .channels = 2};
    const struct spa_pod *param = spa_format_audio_raw_build(&builder, SPA_PARAM_Format, &info);
    stream_param(&s, SPA_PARAM_Format, param);
    assert(mr_pw_stream_native_format(&s) == ((UINT64_C(48000) << 32) | (encoding << 16) | 2));
    assert(mr_pw_stream_format(&s) == ((UINT64_C(48000) << 32) | 2));
    uint32_t bytes = sample_bytes(encoding), stride = 2 * bytes;
    const uint8_t expected[][4] = {
        {0, 0, 0, 0x3e}, {0, 0x10}, {0, 0, 0x10}, {0, 0, 0, 0x10},
    };
    /* Integer input deliberately starts at an unaligned byte address. S24 must
     * remain packed, not sign-extended into a four-byte SPA S24_32 container. */
    _Alignas(float) uint8_t input[256 * 2 * 4 + 1];
    uint32_t offset = encoding == MR_PCM_FORMAT_FLOAT32 ? 0 : 1;
    for (unsigned n = 0; n < 256 * 2; n++) memcpy(input + offset + n * bytes, expected[encoding - 1], bytes);
    struct spa_chunk chunk = {.offset = offset, .size = 128 * stride, .stride = stride};
    struct spa_data pcm = {.data = input, .maxsize = sizeof(input), .chunk = &chunk};
    capture_buffer(&s, &pcm, 128);
    chunk.offset += 128 * stride;
    capture_buffer(&s, &pcm, 128);
    float samples[256 * 2];
    assert(mr_pw_stream_read(&s, samples, 128) == 128);
    assert(mr_pw_stream_read(&s, samples + 128 * 2, 128) == 128);
    for (unsigned n = 0; n < 256 * 2; n++) assert(samples[n] == 0.125f);
    assert(mr_pw_stream_write(&s, samples, 128) == 128);
    assert(mr_pw_stream_write(&s, samples + 128 * 2, 128) == 128);
    _Alignas(float) uint8_t output[128 * 2 * 4 + 1];
    memset(output, 0xa5, sizeof(output));
    pcm.data = output; pcm.maxsize = 128 * stride + 1;
    struct pw_buffer buffer = {.requested = 128};
    playback_buffer(&s, &buffer, &pcm, 128);
    assert(chunk.offset == 0 && chunk.stride == (int32_t)stride && chunk.size == 128 * stride);
    for (unsigned n = 0; n < 128 * 2; n++) assert(memcmp(output + n * bytes, expected[encoding - 1], bytes) == 0);
    assert(output[128 * stride] == 0xa5);
    assert(mr_pw_stream_transfer_snapshot(&s).failures[0].rejected_calls == 0);
    assert(mr_pw_stream_transfer_snapshot(&s).failures[1].rejected_calls == 0);
    stream_param(&s, SPA_PARAM_Format, NULL);
    assert(mr_pw_stream_native_format(&s) == 0);
    mr_pcm_ring_clear(&s.ring);
}

static void native_format_contract(void) {
    native_format_advertisement();
    native_format_pcm(MR_PCM_FORMAT_FLOAT32, SPA_AUDIO_FORMAT_F32);
    native_format_pcm(MR_PCM_FORMAT_SIGNED16, SPA_AUDIO_FORMAT_S16);
    native_format_pcm(MR_PCM_FORMAT_SIGNED24, SPA_AUDIO_FORMAT_S24);
    native_format_pcm(MR_PCM_FORMAT_SIGNED32, SPA_AUDIO_FORMAT_S32);
    assert(pcm_format(SPA_AUDIO_FORMAT_S24_32) == 0);
}

static void capture_chunk_flags_contract(void) {
    for (uint32_t encoding = 1; encoding <= 4; encoding++) {
        MRPWStream s = {.channels = 2, .sample_rate = 48000};
        initialize_transfer_observations(&s);
        atomic_store(&s.format_snapshot, (UINT64_C(48000) << 32) | (encoding << 16) | 2);
        atomic_store(&s.format_valid, true);
        assert(mr_pcm_ring_init(&s.ring, 2, 512));
        const uint32_t stride = 2 * sample_bytes(encoding);
        _Alignas(float) float stale[256 * 2];
        for (unsigned n = 0; n < 256 * 2; n++) stale[n] = 0.875f;
        struct spa_chunk chunk = {.size = 128 * stride, .stride = stride,
                                  .flags = SPA_CHUNK_FLAG_EMPTY};
        struct spa_data pcm = {.data = stale, .maxsize = sizeof(stale), .chunk = &chunk};
        capture_buffer(&s, &pcm, 128);
        /* An EMPTY chunk carries neutral audio even without payload storage. */
        pcm.data = NULL;
        capture_buffer(&s, &pcm, 128);
        float output[128 * 2];
        for (unsigned block = 0; block < 2; block++) {
            assert(mr_pw_stream_read(&s, output, 128) == 128);
            for (unsigned n = 0; n < 128 * 2; n++) assert(output[n] == 0.0f);
        }
        assert(mr_pw_stream_transfer_snapshot(&s).failures[0].rejected_calls == 0);
        assert(mr_pcm_ring_write(&s.ring, stale, 128) == 128);
        pcm.data = stale;
        chunk.flags = SPA_CHUNK_FLAG_EMPTY | SPA_CHUNK_FLAG_CORRUPTED;
        capture_buffer(&s, &pcm, 128);
        /* Rejected PCM invalidates old queued samples before fresh publication. */
        mr_pcm_ring_write_silence(&s.ring, 0);
        assert(mr_pw_stream_read(&s, output, 128) == 0);
        for (unsigned n = 0; n < 128 * 2; n++) assert(output[n] == 0.0f);
        assert(mr_pw_stream_transfer_snapshot(&s).failures[0].rejected_frames == 128);
        mr_pcm_ring_clear(&s.ring);
    }
}

static void transfer_ring_results(void) {
    MRPWStream s = {0};
    initialize_transfer_observations(&s);
    atomic_init(&s.clock_snapshot, 0);
    assert(mr_pcm_ring_init(&s.ring, 2, 512));
    float samples[512 * 2] = {0};
    assert(mr_pw_stream_read(&s, samples, 128) == 0);
    assert(mr_pw_stream_write(&s, samples, 256) == 256);
    assert(mr_pw_stream_write(&s, samples, 256) == 256);
    assert(mr_pw_stream_write(&s, samples, 128) == 0);
    assert(mr_pw_stream_read(&s, samples, 128) == 128);
    assert(mr_pw_stream_read(&s, samples, 384) == 384);
    uint32_t transferred = mr_pcm_ring_read(&s.ring, samples, 128);
    assert(transferred == 0);
    record_rejected(&s, 1, 128, transferred);
    assert(mr_pcm_ring_write(&s.ring, samples, 256) == 256);
    assert(mr_pcm_ring_write(&s.ring, samples, 256) == 256);
    transferred = mr_pcm_ring_write(&s.ring, samples, 128);
    assert(transferred == 0);
    record_rejected(&s, 0, 128, transferred);
    MRPWTransferSnapshot snapshot = mr_pw_stream_transfer_snapshot(&s);
    assert(snapshot.read == 512 && snapshot.written == 1024 && snapshot.queued == 512);
    assert(snapshot.engine_read_frames == 512);
    assert(snapshot.consumer_discard_calls == 0 && snapshot.consumer_discard_frames == 0);
    assert(snapshot.native_restart_calls == 0);
    for (unsigned stage = 0; stage < 4; ++stage) {
        assert(snapshot.failures[stage].rejected_calls == 1);
        assert(snapshot.failures[stage].rejected_frames == 128);
        assert(snapshot.failures[stage].first_clock == 0); /* unavailable, not clock ID zero proof */
    }
    mr_pw_stream_discard_input(&s);
    mr_pw_stream_discard_input(&s); /* Empty discards still count an invocation. */
    restart_stream_ring(&s);
    snapshot = mr_pw_stream_transfer_snapshot(&s);
    assert(snapshot.read == 1024 && snapshot.written == 1024 && snapshot.queued == 0);
    assert(snapshot.engine_read_frames == 512);
    assert(snapshot.consumer_discard_calls == 2 && snapshot.consumer_discard_frames == 512);
    assert(snapshot.native_restart_calls == 1);
    for (unsigned stage = 0; stage < 4; ++stage)
        assert(snapshot.failures[stage].rejected_calls == 1);
    atomic_store(&s.engine_read_frames, UINT64_MAX);
    atomic_store(&s.consumer_discard_calls, UINT64_MAX);
    atomic_store(&s.consumer_discard_frames, UINT64_MAX - 511);
    atomic_store(&s.native_restart_calls, UINT64_MAX);
    assert(mr_pw_stream_write(&s, samples, 256) == 256);
    assert(mr_pw_stream_write(&s, samples, 256) == 256);
    assert(mr_pw_stream_read(&s, samples, 128) == 128);
    mr_pw_stream_discard_input(&s);
    restart_stream_ring(&s);
    snapshot = mr_pw_stream_transfer_snapshot(&s);
    assert(snapshot.engine_read_frames == 127);
    assert(snapshot.consumer_discard_calls == 0);
    assert(snapshot.consumer_discard_frames == UINT64_MAX - 127);
    assert(snapshot.native_restart_calls == 0);
    mr_pcm_ring_clear(&s.ring);
    MRPWStream fresh = {0};
    initialize_transfer_observations(&fresh);
    atomic_init(&fresh.clock_snapshot, 0);
    snapshot = mr_pw_stream_transfer_snapshot(&fresh);
    assert(snapshot.engine_read_frames == 0 && snapshot.consumer_discard_calls == 0);
    assert(snapshot.consumer_discard_frames == 0 && snapshot.native_restart_calls == 0);
    assert(mr_pw_stream_trigger_output(&fresh) == -EINVAL);
    assert(mr_pw_stream_transfer_snapshot(&fresh).native_output_trigger_rejections == 1);
    atomic_store(&fresh.native_output_trigger_rejections, UINT64_MAX);
    assert(mr_pw_stream_trigger_output(&fresh) == -EINVAL);
    assert(mr_pw_stream_transfer_snapshot(&fresh).native_output_trigger_rejections == 0);
    for (unsigned stage = 0; stage < 4; ++stage)
        assert(mr_pw_stream_transfer_snapshot(&fresh).failures[stage].rejected_calls == 0);
}

static void select_native_format(MRPWStream *s, enum spa_audio_format format) {
    uint8_t storage[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = {.format = format, .rate = 48000, .channels = 2};
    info.position[0] = SPA_AUDIO_CHANNEL_FL; info.position[1] = SPA_AUDIO_CHANNEL_FR;
    const struct spa_pod *param = spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info);
    assert(param);
    /* Change only this private acquisition's client EnumFormat before activation.
     * The actual Format callback must acknowledge the choice; never inject it. */
    pw_thread_loop_lock(s->loop);
    assert(pw_stream_update_params(s->stream, &param, 1) >= 0);
    s->supported_formats = 1u << (pcm_format(format) - 1);
    pw_thread_loop_unlock(s->loop);
}

static void native_lifecycle(enum spa_audio_format format) {
    Fixture f = {0};
    atomic_init(&f.generate, false);
    atomic_init(&f.received, 0);
    atomic_init(&f.stale, 0);
    atomic_init(&f.idle_callbacks, 0);
    MRPWConnection *source_connection = mr_pw_connection_open("fixture-source");
    MRPWConnection *observer_connection = mr_pw_connection_open("fixture-observer");
    assert(source_connection && observer_connection);
    MRPWConnectionHandles source_handles = mr_pw_connection_handles(source_connection);
    MRPWConnectionHandles observer_handles = mr_pw_connection_handles(observer_connection);
    f.source = mr_pw_stream_open(&source_handles, "io.mixrelay.fixture.lifecycle", "Owned lifecycle fixture",
                                NULL, 1, 2, 48000, 128, true, 32768);
    assert(f.source);
    assert(mr_pw_stream_supported_formats(f.source) == 15);
    select_native_format(f.source, format);
    assert(mr_pw_connection_start(source_connection) == 0);
    /* Explicit target streams intentionally never reconnect/fall back. Wait for
     * this source to register before asking policy to resolve its exact name. */
    uint64_t expected_format = (UINT64_C(48000) << 32) | (pcm_format(format) << 16) | 2;
    for (unsigned n = 0; n < 3000 && (!mr_pw_stream_ready(f.source) || mr_pw_stream_native_format(f.source) != expected_format); n++) delay();
    assert(mr_pw_stream_native_format(f.source) == expected_format);
    char serial[32];
    source_serial(f.source, serial);
    f.observer = mr_pw_stream_open(&observer_handles, "io.mixrelay.fixture.lifecycle.observer", "Owned observer",
                                  serial, 3, 2, 48000, 128, false, 32768);
    assert(f.observer);
    assert(mr_pw_stream_supported_formats(f.observer) == 1);
    assert(mr_pw_connection_start(observer_connection) == 0);
    for (unsigned n = 0; n < 3000 && mr_pw_stream_node_id(f.observer) == PW_ID_ANY; n++) delay();
    assert(mr_pw_connection_prepare_capture(observer_connection, mr_pw_stream_node_id(f.observer), 48000) == 0);
    assert(atomic_load(&f.observer->state) != PW_STREAM_STATE_STREAMING);
    assert(mr_pw_stream_activate_capture(f.observer) == 0);
    /* Manager Format and explicit monitor PortConfig now precede Start. A
     * duplicate Start must not need to repair an INACTIVE shared activation. */
    active(f.observer, true);
    MRPWDriver *driver = mr_pw_driver_open(tick, &f);
    assert(driver);
    /* Permit idle only after the source/observer/driver have negotiated. A
     * non-idling product node need not enter PAUSED when scheduling is disabled;
     * this fixture explicitly exercises native idle/resume instead. */
    const struct spa_dict_item idle_items[] = {
        SPA_DICT_ITEM_INIT("node.pause-on-idle", "true"),
        SPA_DICT_ITEM_INIT("node.always-process", "false"),
    };
    const struct spa_dict idle_properties = SPA_DICT_INIT_ARRAY(idle_items);
    pw_thread_loop_lock(f.source->loop);
    assert(pw_stream_update_properties(f.source->stream, &idle_properties) >= 0);
    pw_thread_loop_unlock(f.source->loop);
    puts("lifecycle: initial source start"); fflush(stdout);
    active(f.source, true);
    for (unsigned cycle = 0; cycle < 5; cycle++) {
        atomic_store(&f.generate, false);
        uint64_t idle = atomic_load(&f.idle_callbacks);
        for (unsigned n = 0; n < 3000 && atomic_load(&f.idle_callbacks) == idle; n++) delay();
        assert(atomic_load(&f.idle_callbacks) != idle);
        printf("lifecycle: pause cycle=%u\n", cycle); fflush(stdout);
        active(f.source, false);
        /* There is no producer while generate=false; seed the stale queue that
         * the real PAUSED→STREAMING callback must invalidate on both endpoints. */
        float stale[8192 * 2];
        for (unsigned n = 0; n < 8192 * 2; n++) stale[n] = 1.0f;
        assert(mr_pw_stream_write(f.source, stale, 8192) == 8192);
        printf("lifecycle: resume cycle=%u\n", cycle); fflush(stdout);
        active(f.source, true);
        atomic_store(&f.generate, true);
        uint64_t before = atomic_load(&f.received);
        for (unsigned n = 0; n < 400; n++) delay();
        assert(atomic_load(&f.received) > before);
        assert(atomic_load(&f.stale) == 0);
    }
    mr_pw_driver_close(driver);
    mr_pw_stream_close(f.observer);
    mr_pw_stream_close(f.source);
    mr_pw_connection_close(observer_connection);
    mr_pw_connection_close(source_connection);
    printf("PASS: native_encoding=%u; five real pause/resume transitions; fresh_samples=%llu; stale_samples=%llu\n",
           pcm_format(format), (unsigned long long)atomic_load(&f.received), (unsigned long long)atomic_load(&f.stale));
}

int main(int argc, char **argv) {
    transfer_counter_contract();
    transfer_ring_results();
    native_format_contract();
    capture_chunk_flags_contract();
    if (argc == 2 && strcmp(argv[1], "--transfer-counters-only") == 0) {
        puts("PASS: transfer ABI, ring lifetime, native format negotiation and PCM byte geometry");
        return 0;
    }
    assert(argc == 1);
    native_lifecycle(SPA_AUDIO_FORMAT_F32);
    native_lifecycle(SPA_AUDIO_FORMAT_S16);
    native_lifecycle(SPA_AUDIO_FORMAT_S24);
    native_lifecycle(SPA_AUDIO_FORMAT_S32);
}
