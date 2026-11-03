#include "MRPipeWire.h"
#include "MRPcmRing.h"
#include <pipewire/pipewire.h>
#include <pipewire/filter.h>
#include <pipewire/data-loop.h>
#include <spa/param/audio/format-utils.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
    MR_NATIVE_CAPTURE_WRITE = 0,
    MR_NATIVE_OUTPUT_READ = 1,
    MR_ENGINE_CAPTURE_READ = 2,
    MR_ENGINE_OUTPUT_WRITE = 3,
};

typedef struct {
    _Atomic uint64_t rejected_calls, rejected_frames;
    _Atomic uint64_t first_position, last_position, first_clock, last_clock;
    /* This group is written by exactly one serialized native or DSP role. */
    bool first_recorded;
} RejectedTransfer;

_Static_assert(sizeof(MRPWRejectedTransfer) == 48, "transfer failure ABI");
_Static_assert(sizeof(MRPWTransferSnapshot) == 256, "transfer snapshot ABI");
_Static_assert(_Alignof(MRPWTransferSnapshot) == 8, "transfer snapshot alignment");
_Static_assert(ATOMIC_LONG_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2,
               "RT diagnostic counters require lock-free 64-bit atomics");

struct MRPWStream {
    struct pw_thread_loop *loop;
    struct pw_stream *stream;
    struct spa_hook listener;
    MRPcmRing ring;
    uint32_t channels, sample_rate, supported_formats;
    bool native_output, manual_output;
    _Atomic int state;
    _Atomic bool format_valid;
    _Atomic int buffer_return_error;
    _Atomic(struct spa_io_position *) position;
    _Atomic uint64_t clock_snapshot;
    _Atomic uint64_t format_snapshot;
    _Atomic uint64_t quantum_snapshot;
    _Atomic uint64_t position_snapshot;
    RejectedTransfer failures[4];
    _Atomic uint64_t engine_read_frames, consumer_discard_calls, consumer_discard_frames;
    _Atomic uint64_t native_restart_calls;
    _Atomic uint64_t native_output_trigger_rejections;
};

static uint32_t pcm_format(enum spa_audio_format format) {
    switch (format) {
        case SPA_AUDIO_FORMAT_F32: return MR_PCM_FORMAT_FLOAT32;
        case SPA_AUDIO_FORMAT_S16: return MR_PCM_FORMAT_SIGNED16;
        case SPA_AUDIO_FORMAT_S24: return MR_PCM_FORMAT_SIGNED24;
        case SPA_AUDIO_FORMAT_S32: return MR_PCM_FORMAT_SIGNED32;
        default: return 0;
    }
}

static uint32_t sample_bytes(uint32_t format) {
    switch (format) {
        case MR_PCM_FORMAT_FLOAT32: case MR_PCM_FORMAT_SIGNED32: return 4;
        case MR_PCM_FORMAT_SIGNED16: return 2;
        case MR_PCM_FORMAT_SIGNED24: return 3;
        default: return 0;
    }
}

static uint32_t stream_encoding(MRPWStream *s) {
    return (atomic_load_explicit(&s->format_snapshot, memory_order_acquire) >> 16) & 0xff;
}

static uint32_t stream_stride(MRPWStream *s) {
    return s->channels * sample_bytes(stream_encoding(s));
}

static void initialize_transfer_observations(MRPWStream *s) {
    atomic_init(&s->buffer_return_error, 0);
    atomic_init(&s->position_snapshot, 0);
    atomic_init(&s->engine_read_frames, 0);
    atomic_init(&s->consumer_discard_calls, 0);
    atomic_init(&s->consumer_discard_frames, 0);
    atomic_init(&s->native_restart_calls, 0);
    atomic_init(&s->native_output_trigger_rejections, 0);
    for (unsigned i = 0; i < 4; ++i) {
        RejectedTransfer *failure = &s->failures[i];
        atomic_init(&failure->rejected_calls, 0);
        atomic_init(&failure->rejected_frames, 0);
        atomic_init(&failure->first_position, 0);
        atomic_init(&failure->last_position, 0);
        atomic_init(&failure->first_clock, 0);
        atomic_init(&failure->last_clock, 0);
        failure->first_recorded = false;
    }
}

static void restart_stream_ring(MRPWStream *s) {
    mr_pcm_ring_restart(&s->ring);
    atomic_fetch_add_explicit(&s->native_restart_calls, 1, memory_order_relaxed);
}

static void record_discarded_transfer(MRPWStream *s, unsigned stage, uint32_t frames) {
    RejectedTransfer *failure = &s->failures[stage];
    uint64_t position = atomic_load_explicit(&s->position_snapshot, memory_order_acquire);
    uint64_t clock = atomic_load_explicit(&s->clock_snapshot, memory_order_acquire);
    if (!failure->first_recorded) {
        failure->first_recorded = true;
        atomic_store_explicit(&failure->first_position, position, memory_order_relaxed);
        atomic_store_explicit(&failure->first_clock, clock, memory_order_relaxed);
    }
    atomic_store_explicit(&failure->last_position, position, memory_order_relaxed);
    atomic_store_explicit(&failure->last_clock, clock, memory_order_relaxed);
    atomic_fetch_add_explicit(&failure->rejected_frames, frames, memory_order_relaxed);
    atomic_fetch_add_explicit(&failure->rejected_calls, 1, memory_order_release);
}

static void record_rejected(MRPWStream *s, unsigned stage, uint32_t requested,
                            uint32_t transferred) {
    if (transferred < requested)
        record_discarded_transfer(s, stage, requested - transferred);
}

static void record_capture_ingress_discard(MRPWStream *s, const struct spa_data *pcm) {
    if (s->native_output) return;
    /* A declared empty capture chunk is legitimate no-data, not lost PCM. */
    if (pcm && pcm->chunk && pcm->chunk->size == 0) return;
    uint32_t frames = 0;
    if (pcm && pcm->chunk && atomic_load_explicit(&s->format_valid, memory_order_acquire)) {
        uint32_t stride = stream_stride(s);
        uint32_t offset = pcm->chunk->offset, size = pcm->chunk->size;
        if (stride && offset <= pcm->maxsize && size <= pcm->maxsize - offset && size % stride == 0 &&
            (pcm->chunk->stride == 0 || pcm->chunk->stride == (int32_t)stride))
            frames = size / stride;
    }
    /* Unknown geometry still records a discarded ingress call, with zero
     * known frames; never invent a PCM quantum from the graph clock. */
    record_discarded_transfer(s, MR_NATIVE_CAPTURE_WRITE, frames);
}

static int set_stream_position(struct spa_loop *loop, bool async, uint32_t seq,
                               const void *data, size_t size, void *user_data) {
    (void)loop; (void)async; (void)seq;
    MRPWStream *s = user_data;
    struct spa_io_position *position = NULL;
    if (size == sizeof(position)) memcpy(&position, data, sizeof(position));
    if (!s->native_output && atomic_load_explicit(&s->position, memory_order_acquire) != position)
        restart_stream_ring(s);
    atomic_store_explicit(&s->position, position, memory_order_release);
    atomic_store_explicit(&s->clock_snapshot, 0, memory_order_release);
    atomic_store_explicit(&s->position_snapshot, 0, memory_order_release);
    atomic_store_explicit(&s->quantum_snapshot, 0, memory_order_release);
    return 0;
}

static void stream_io(void *data, uint32_t id, void *area, uint32_t size) {
    MRPWStream *s = data;
    if (id != SPA_IO_Position) return;
    struct spa_io_position *position = size >= sizeof(*position) ? area : NULL;
    /* Same handoff as PipeWire stream.c impl_set_io: finish earlier callbacks
     * before replacing the borrowed IO pointer. Merely storing an atomic pointer
     * on the main loop would not protect the previous IO area's lifetime. */
    /* PipeWire >= 1.2 assigns data loops per stream. */
    pw_loop_invoke(pw_stream_get_data_loop(s->stream), set_stream_position,
                   0, &position, sizeof(position), true, s);
}

static void stream_param(void *data, uint32_t id, const struct spa_pod *param) {
    MRPWStream *s = data;
    if (id != SPA_PARAM_Format) return;
    struct spa_audio_info_raw info = {0};
    bool parsed = param && spa_format_audio_raw_parse(param, &info) >= 0;
    uint32_t encoding = parsed ? pcm_format(info.format) : 0;
    bool valid = encoding && (s->supported_formats & (1u << (encoding - 1))) &&
        info.rate == s->sample_rate && info.channels == s->channels;
    uint64_t next = valid ? ((uint64_t)info.rate << 32) | (encoding << 16) | info.channels : 0;
    if (!valid || atomic_load_explicit(&s->format_snapshot, memory_order_acquire) != next)
        restart_stream_ring(s);
    atomic_store_explicit(&s->format_snapshot, next, memory_order_release);
    atomic_store_explicit(&s->format_valid, valid, memory_order_release);
}

static void stream_state(void *data, enum pw_stream_state old,
                         enum pw_stream_state state, const char *error) {
    (void)error;
    MRPWStream *s = data;
    if (old == PW_STREAM_STATE_STREAMING || state == PW_STREAM_STATE_STREAMING)
        restart_stream_ring(s);
    atomic_store_explicit(&s->state, state, memory_order_release);
}

static void playback_buffer(MRPWStream *s, struct pw_buffer *buffer,
                            struct spa_data *pcm, uint32_t graph_frames) {
    const uint32_t encoding = stream_encoding(s), stride = s->channels * sample_bytes(encoding);
    if (!stride) { pcm->chunk->size = 0; return; }
    uint32_t frames = pcm->maxsize / stride;
    if (buffer->requested && buffer->requested < frames) frames = (uint32_t)buffer->requested;
    atomic_store_explicit(&s->quantum_snapshot, ((uint64_t)graph_frames << 32) | frames,
                          memory_order_release);
    uint32_t transferred = encoding == MR_PCM_FORMAT_FLOAT32
        ? mr_pcm_ring_read(&s->ring, pcm->data, frames)
        : mr_pcm_ring_read_pcm(&s->ring, pcm->data, frames, encoding);
    record_rejected(s, MR_NATIVE_OUTPUT_READ, frames, transferred);
    pcm->chunk->offset = 0;
    pcm->chunk->stride = (int32_t)stride;
    pcm->chunk->size = frames * stride;
    /* Recycled metadata describes this transfer, not a previous buffer. A
     * wholly starved ring emitted silence rather than prepared graph PCM. */
    pcm->chunk->flags = transferred == 0 ? SPA_CHUNK_FLAG_EMPTY : 0;
    /* PipeWire time accounting uses frames, including any emitted silence. */
    buffer->size = frames;
}

static void capture_buffer(MRPWStream *s, struct spa_data *pcm, uint32_t graph_frames) {
    const uint32_t encoding = stream_encoding(s), stride = s->channels * sample_bytes(encoding);
    const uint32_t offset = pcm->chunk->offset;
    const uint32_t size = pcm->chunk->size;
    const bool empty = (pcm->chunk->flags & SPA_CHUNK_FLAG_EMPTY) != 0;
    if (!stride || offset > pcm->maxsize || size > pcm->maxsize - offset ||
        size % stride || (!empty && encoding == MR_PCM_FORMAT_FLOAT32 && offset % sizeof(float)) ||
        (pcm->chunk->stride != 0 && pcm->chunk->stride != (int32_t)stride)) {
        record_capture_ingress_discard(s, pcm);
        return;
    }
    if (pcm->chunk->flags & SPA_CHUNK_FLAG_CORRUPTED) {
        record_capture_ingress_discard(s, pcm);
        restart_stream_ring(s);
        return;
    }
    const uint32_t frames = size / stride;
    atomic_store_explicit(&s->quantum_snapshot, ((uint64_t)graph_frames << 32) | frames,
                          memory_order_release);
    uint32_t transferred;
    if (empty) {
        /* EMPTY payload storage need not contain initialized audio. Preserve
         * the declared timeline with neutral samples, never recycled bytes. */
        transferred = mr_pcm_ring_write_silence(&s->ring, frames);
    } else {
        void *input = (uint8_t *)pcm->data + offset;
        transferred = encoding == MR_PCM_FORMAT_FLOAT32
            ? mr_pcm_ring_write(&s->ring, input, frames)
            : mr_pcm_ring_write_pcm(&s->ring, input, frames, encoding);
    }
    record_rejected(s, MR_NATIVE_CAPTURE_WRITE, frames, transferred);
}

static void stream_process_buffer(MRPWStream *s) {
    if (atomic_load_explicit(&s->buffer_return_error, memory_order_acquire) < 0) return;
    /* Only the synchronized data loop dereferences borrowed IO memory. Rust
     * receives an atomic value copy, never a native pointer. */
    struct spa_io_position *position = atomic_load_explicit(&s->position, memory_order_acquire);
    uint64_t snapshot = 0;
    if (position && position->clock.rate.num == 1)
        snapshot = ((uint64_t)position->clock.id << 32) | position->clock.rate.denom;
    /* Native inputs cannot carry PCM across clock-domain replacement. A
     * quantum-only change or scheduling delay keeps the retained backlog. */
    if (!s->native_output &&
        atomic_load_explicit(&s->clock_snapshot, memory_order_acquire) != snapshot)
        restart_stream_ring(s);
    atomic_store_explicit(&s->position_snapshot,
                          snapshot ? position->clock.position : 0, memory_order_release);
    atomic_store_explicit(&s->clock_snapshot, snapshot, memory_order_release);
    struct pw_buffer *b = pw_stream_dequeue_buffer(s->stream);
    if (!b) return;
    /* A recycled output must not retain the previous buffer's frame count. */
    if (s->native_output) b->size = 0;
    struct spa_buffer *buffer = b->buffer;
    /* Every dequeued buffer, including unusable buffers, is returned exactly once. */
    if (!buffer || buffer->n_datas != 1) {
        record_capture_ingress_discard(s, NULL);
        goto done;
    }
    struct spa_data *d = &buffer->datas[0];
    if (!d->chunk || (!d->data &&
        (s->native_output || !(d->chunk->flags & SPA_CHUNK_FLAG_EMPTY)))) {
        record_capture_ingress_discard(s, d);
        goto done;
    }
    if (!atomic_load_explicit(&s->format_valid, memory_order_acquire)) {
        if (s->native_output) d->chunk->size = 0;
        else record_capture_ingress_discard(s, d);
        goto done;
    }
    uint32_t graph_frames = position && position->clock.duration <= UINT32_MAX
        ? (uint32_t)position->clock.duration : 0;
    if (s->native_output) playback_buffer(s, b, d, graph_frames);
    else capture_buffer(s, d, graph_frames);
done:
    const int returned = pw_stream_queue_buffer(s->stream, b);
    if (returned < 0) {
        /* The data-loop callback is the sole writer. This acquisition stays
         * faulted until control retires it; a replacement owns further PCM. */
        atomic_store_explicit(&s->buffer_return_error, returned, memory_order_release);
    }
}

#ifdef MIXRELAY_LAB_PROCESS_OBSERVER
/* Available only when the private lab preloads its bounded observer. */
extern void mr_pw_observe_process(struct pw_stream *stream, uint32_t phase)
    __attribute__((weak));
#endif

static void stream_process(void *data) {
    MRPWStream *s = data;
#ifdef MIXRELAY_LAB_PROCESS_OBSERVER
    if (mr_pw_observe_process) mr_pw_observe_process(s->stream, 1);
#endif
    stream_process_buffer(s);
#ifdef MIXRELAY_LAB_PROCESS_OBSERVER
    if (mr_pw_observe_process) mr_pw_observe_process(s->stream, 2);
#endif
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = stream_state, .process = stream_process,
    .io_changed = stream_io, .param_changed = stream_param,
};

static uint32_t build_stream_formats(MRPWStream *s, struct spa_pod_builder *builder,
                                    const struct spa_pod **params, bool virtual_endpoint,
                                    uint32_t selected_format) {
    const enum spa_audio_format formats[] = {
        SPA_AUDIO_FORMAT_F32, SPA_AUDIO_FORMAT_S16, SPA_AUDIO_FORMAT_S24, SPA_AUDIO_FORMAT_S32,
    };
    struct spa_audio_info_raw info = { .rate = s->sample_rate, .channels = s->channels };
    if (s->channels == 1) info.position[0] = SPA_AUDIO_CHANNEL_MONO;
    else if (s->channels == 2) {
        info.position[0] = SPA_AUDIO_CHANNEL_FL; info.position[1] = SPA_AUDIO_CHANNEL_FR;
    } else for (uint32_t c = 0; c < s->channels; c++) info.position[c] = SPA_AUDIO_CHANNEL_AUX0 + c;
    const uint32_t count = virtual_endpoint ? 4 : 1;
    uint32_t written = 0;
    for (uint32_t i = 0; i < count; i++) {
        if (selected_format && pcm_format(formats[i]) != selected_format) continue;
        info.format = formats[i];
        params[written] = spa_format_audio_raw_build(builder, SPA_PARAM_EnumFormat, &info);
        if (!params[written]) return 0;
        s->supported_formats |= 1u << (pcm_format(info.format) - 1);
        written++;
    }
    return written;
}

MRPWStream *mr_pw_stream_open(const MRPWConnectionHandles *connection, const char *name, const char *description,
                             const char *target, uint32_t mode, uint32_t channels,
                             uint32_t sample_rate, uint32_t block_frames, bool manual_output,
                             uint32_t ring_frames) {
    return mr_pw_stream_open_pcm(connection, name, description, target, mode, channels,
                                 sample_rate, block_frames, manual_output, 0, ring_frames);
}

MRPWStream *mr_pw_stream_open_pcm(const MRPWConnectionHandles *connection, const char *name, const char *description,
                                 const char *target, uint32_t mode, uint32_t channels,
                                 uint32_t sample_rate, uint32_t block_frames, bool manual_output,
                                 uint32_t sample_format, uint32_t ring_frames) {
    if (!connection || !connection->core || !connection->thread_loop) return NULL;
    if (!name || !description || mode > MR_PW_GRAPH_CONTRIBUTION ||
        (mode >= MR_PW_APPLICATION_CAPTURE && (!target || !target[0]))) return NULL;
    if (!channels || channels > SPA_AUDIO_MAX_CHANNELS || sample_format > 4) return NULL;
    if (sample_rate < 8000 || sample_rate > 384000 || !block_frames || block_frames > 8192) return NULL;
    MRPWStream *s = calloc(1, sizeof(*s));
    if (!s) return NULL;
    atomic_init(&s->state, PW_STREAM_STATE_UNCONNECTED);
    atomic_init(&s->format_valid, false);
    atomic_init(&s->position, NULL);
    atomic_init(&s->clock_snapshot, 0);
    atomic_init(&s->format_snapshot, 0);
    atomic_init(&s->quantum_snapshot, 0);
    initialize_transfer_observations(s);
    s->native_output = mode == MR_PW_VIRTUAL_CAPTURE || mode == MR_PW_DEVICE_OUTPUT ||
                       mode == MR_PW_GRAPH_CONTRIBUTION;
    s->manual_output = s->native_output && manual_output;
    s->channels = channels; s->sample_rate = sample_rate;
    s->loop = connection->thread_loop;
    if (!mr_pcm_ring_init(&s->ring, channels, ring_frames)) goto fail;
    struct pw_properties *p = pw_properties_new(
        PW_KEY_NODE_NAME, name, PW_KEY_NODE_DESCRIPTION, description,
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, s->native_output ? "Playback" : "Capture",
        PW_KEY_MEDIA_ROLE, "DSP", PW_KEY_NODE_VIRTUAL, "true",
        PW_KEY_APP_NAME, "MixRelay", PW_KEY_APP_ID,
        mode < MR_PW_APPLICATION_CAPTURE ? "io.mixrelay.driver" : "io.mixrelay.engine",
        PW_KEY_NODE_GROUP, "mixrelay.graph", PW_KEY_NODE_RATE, "1/48000",
        PW_KEY_NODE_LATENCY, "128/48000",
        "node.pause-on-idle", "false", "node.always-process", "true", NULL);
    if (!p) goto fail;
    if (!s->native_output) {
        /* These DSP monitor ports belong to this capture adapter, not its
         * upstream source. A downstream link orders the inline FIFO ingress
         * callback before the graph consumer; RAW PCM still uses this one SRC. */
        pw_properties_set(p, PW_KEY_NODE_ASYNC, "false");
    } else if (mode == MR_PW_GRAPH_CONTRIBUTION) {
        /* Only graph contributions may depend on this sink's audio return path capture.
         * PipeWire's async link IO breaks that causal cycle by one quantum;
         * ordinary playback and the Driver's basic audio return path stay synchronous. */
        pw_properties_set(p, PW_KEY_NODE_ASYNC, "true");
    }
    pw_properties_setf(p, PW_KEY_APP_PROCESS_ID, "%ld", (long)getpid());
    /* Preferences belong to this application's adapter, never server metadata. */
    pw_properties_setf(p, PW_KEY_NODE_RATE, "1/%u", sample_rate);
    pw_properties_setf(p, PW_KEY_NODE_LATENCY, "%u/%u", block_frames, sample_rate);
    pw_properties_setf(p, PW_KEY_AUDIO_CHANNELS, "%u", channels);
    if (mode < MR_PW_APPLICATION_CAPTURE)
        pw_properties_set(p, PW_KEY_MEDIA_CLASS, mode == MR_PW_VIRTUAL_RENDER ? "Audio/Sink" : "Audio/Source");
    else {
        pw_properties_set(p, PW_KEY_TARGET_OBJECT, target);
        pw_properties_set(p, "stream.dont-remix", "true");
        pw_properties_set(p, "node.dont-reconnect", "true");
        /* An unavailable explicit target must never leak audio to the default. */
        pw_properties_set(p, "node.dont-fallback", "true");
        pw_properties_set(p, "node.passive", "true");
    }
    s->stream = pw_stream_new(connection->core, name, p);
    if (!s->stream) goto fail;
    pw_stream_add_listener(s->stream, &s->listener, &stream_events, s);
    uint8_t storage[2048];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *params[4];
    uint32_t param_count = build_stream_formats(s, &builder, params, mode < MR_PW_APPLICATION_CAPTURE, sample_format);
    if (!param_count) goto fail;
    enum pw_stream_flags flags = PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS;
    /* PCM depends on the DSP publisher, not only on the common driver. Each
     * complete DSP cycle releases this output after its final slice is queued. */
    if (s->manual_output) flags |= PW_STREAM_FLAG_TRIGGER;
    /* Manager Format/PortConfig must finish before the adapter starts. The
     * Rust control owner prepares DSP monitor PortConfig before activation. */
    if (!s->native_output) flags |= PW_STREAM_FLAG_INACTIVE;
    if (mode >= MR_PW_APPLICATION_CAPTURE) flags |= PW_STREAM_FLAG_AUTOCONNECT;
    if (pw_stream_connect(s->stream, s->native_output ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
                          PW_ID_ANY, flags, params, param_count) < 0) goto fail;
    return s;
fail:
    mr_pw_stream_close(s);
    return NULL;
}

void mr_pw_stream_close(MRPWStream *s) {
    if (!s) return;
    if (s->loop) {
        /* Disconnect synchronizes with the data loop. The control-loop lock is
         * never acquired by our RT callbacks; it serializes control callbacks. */
        pw_thread_loop_lock(s->loop);
        if (s->stream) { pw_stream_disconnect(s->stream); pw_stream_destroy(s->stream); }
        pw_thread_loop_unlock(s->loop);
    }
    mr_pcm_ring_clear(&s->ring);
    free(s);
}
int mr_pw_stream_set_description(MRPWStream *s, const char *description) {
    if (!s || !description) return -EINVAL;
    /* The ABI glue accesses the opaque stream/loop and SPA dictionary macro.
     * Identity, transaction ordering, rollback and lifetime stay in Rust. */
    const struct spa_dict_item item = SPA_DICT_ITEM_INIT(PW_KEY_NODE_DESCRIPTION, description);
    const struct spa_dict properties = SPA_DICT_INIT(&item, 1);
    pw_thread_loop_lock(s->loop);
    int result = pw_stream_update_properties(s->stream, &properties);
    pw_thread_loop_unlock(s->loop);
    return result;
}
uint32_t mr_pw_stream_node_id(MRPWStream *s) {
    if (!s) return SPA_ID_INVALID;
    pw_thread_loop_lock(s->loop);
    uint32_t id = pw_stream_get_node_id(s->stream);
    pw_thread_loop_unlock(s->loop);
    return id;
}
int mr_pw_stream_activate_capture(MRPWStream *s) {
    if (!s || s->native_output) return -EINVAL;
    pw_thread_loop_lock(s->loop);
    int result = pw_stream_set_active(s->stream, true);
    pw_thread_loop_unlock(s->loop);
    return result;
}
int mr_pw_stream_return_error(MRPWStream *s) {
    return atomic_load_explicit(&s->buffer_return_error, memory_order_acquire);
}
int mr_pw_stream_ready(MRPWStream *s) {
    if (mr_pw_stream_return_error(s) < 0) return 0;
    int state = atomic_load_explicit(&s->state, memory_order_acquire);
    return state == PW_STREAM_STATE_PAUSED || state == PW_STREAM_STATE_STREAMING;
}
int mr_pw_stream_failed(MRPWStream *s) {
    if (mr_pw_stream_return_error(s) < 0) return 1;
    int state = atomic_load_explicit(&s->state, memory_order_acquire);
    return state == PW_STREAM_STATE_UNCONNECTED || state == PW_STREAM_STATE_ERROR;
}
int mr_pw_stream_connected(MRPWStream *s) {
    return mr_pw_stream_return_error(s) == 0 &&
        atomic_load_explicit(&s->state, memory_order_acquire) == PW_STREAM_STATE_STREAMING;
}
int mr_pw_stream_input_activity(MRPWStream *s) {
    if (mr_pw_stream_return_error(s) < 0) return 0;
    int state = atomic_load_explicit(&s->state, memory_order_acquire);
    return state == PW_STREAM_STATE_STREAMING ? 2 : state == PW_STREAM_STATE_PAUSED ? 1 : 0;
}
uint64_t mr_pw_stream_clock(MRPWStream *s) {
    if (mr_pw_stream_return_error(s) < 0 ||
        !atomic_load_explicit(&s->format_valid, memory_order_acquire) ||
        atomic_load_explicit(&s->state, memory_order_acquire) != PW_STREAM_STATE_STREAMING) return 0;
    return atomic_load_explicit(&s->clock_snapshot, memory_order_acquire);
}
uint64_t mr_pw_stream_format(MRPWStream *s) {
    uint64_t format = mr_pw_stream_native_format(s);
    return (format & UINT64_C(0xffffffff00000000)) | (format & 0xffff);
}
uint64_t mr_pw_stream_native_format(MRPWStream *s) {
    /* A single atomic tuple also publishes zero on invalid/unavailable Format. */
    if (mr_pw_stream_return_error(s) < 0 ||
        !atomic_load_explicit(&s->format_valid, memory_order_acquire)) return 0;
    return atomic_load_explicit(&s->format_snapshot, memory_order_acquire);
}
uint32_t mr_pw_stream_supported_formats(MRPWStream *s) {
    return s->supported_formats;
}
uint64_t mr_pw_stream_quantum(MRPWStream *s) {
    if (mr_pw_stream_return_error(s) < 0 ||
        !atomic_load_explicit(&s->format_valid, memory_order_acquire) ||
        atomic_load_explicit(&s->state, memory_order_acquire) != PW_STREAM_STATE_STREAMING) return 0;
    return atomic_load_explicit(&s->quantum_snapshot, memory_order_acquire);
}
void mr_pw_stream_discard_input(MRPWStream *s) {
    /* Engine is the sole consumer only for input-side streams. */
    if (!s->native_output) {
        uint64_t before = mr_pcm_ring_stats(&s->ring).read;
        mr_pcm_ring_discard(&s->ring);
        uint64_t after = mr_pcm_ring_stats(&s->ring).read;
        atomic_fetch_add_explicit(&s->consumer_discard_frames, after - before, memory_order_relaxed);
        atomic_fetch_add_explicit(&s->consumer_discard_calls, 1, memory_order_relaxed);
    }
}
MRPWTransferCounters mr_pw_stream_counters(MRPWStream *s) {
    MRPcmRingStats stats = mr_pcm_ring_stats(&s->ring);
    return (MRPWTransferCounters) {
        .overflows = stats.overflows,
        .starvations = stats.starvations,
    };
}
uint64_t mr_pw_stream_capture_failures(MRPWStream *s) {
    if (s->native_output) return 0;
    return atomic_load_explicit(&s->failures[MR_NATIVE_CAPTURE_WRITE].rejected_calls,
                                memory_order_acquire) +
        atomic_load_explicit(&s->native_restart_calls, memory_order_acquire) +
        (mr_pw_stream_return_error(s) < 0);
}
uint32_t mr_pw_stream_read(MRPWStream *s, float *samples, uint32_t frames) {
    if (mr_pw_stream_return_error(s) < 0) {
        if (samples && frames)
            memset(samples, 0, (size_t)frames * s->channels * sizeof(float));
        return 0;
    }
    uint32_t transferred = mr_pcm_ring_read(&s->ring, samples, frames);
    if (transferred)
        atomic_fetch_add_explicit(&s->engine_read_frames, transferred, memory_order_relaxed);
    record_rejected(s, MR_ENGINE_CAPTURE_READ, frames, transferred);
    return transferred;
}
uint32_t mr_pw_stream_write(MRPWStream *s, const float *samples, uint32_t frames) {
    if (mr_pw_stream_return_error(s) < 0) return 0;
    uint32_t transferred = mr_pcm_ring_write(&s->ring, samples, frames);
    record_rejected(s, MR_ENGINE_OUTPUT_WRITE, frames, transferred);
    return transferred;
}
int mr_pw_stream_trigger_output(MRPWStream *s) {
    const int error = mr_pw_stream_return_error(s);
    if (error < 0) return error;
    int result;
    if (!s->manual_output) result = -EINVAL;
    else if (atomic_load_explicit(&s->state, memory_order_acquire) != PW_STREAM_STATE_STREAMING)
        return 0;
    else result = pw_stream_trigger_process(s->stream);
    if (result < 0)
        atomic_fetch_add_explicit(&s->native_output_trigger_rejections, 1, memory_order_relaxed);
    return result;
}

MRPWTransferSnapshot mr_pw_stream_transfer_snapshot(MRPWStream *s) {
    MRPcmRingStats stats = mr_pcm_ring_stats(&s->ring);
    MRPWTransferSnapshot result = {.read = stats.read, .written = stats.written,
                                 .queued = stats.queued,
        .engine_read_frames = atomic_load_explicit(&s->engine_read_frames, memory_order_relaxed),
        .consumer_discard_calls = atomic_load_explicit(&s->consumer_discard_calls, memory_order_relaxed),
        .consumer_discard_frames = atomic_load_explicit(&s->consumer_discard_frames, memory_order_relaxed),
        .native_restart_calls = atomic_load_explicit(&s->native_restart_calls, memory_order_relaxed),
        .native_output_trigger_rejections = atomic_load_explicit(&s->native_output_trigger_rejections,
                                                               memory_order_relaxed)};
    for (unsigned i = 0; i < 4; ++i) {
        RejectedTransfer *failure = &s->failures[i];
        result.failures[i] = (MRPWRejectedTransfer) {
            .rejected_calls = atomic_load_explicit(&failure->rejected_calls, memory_order_acquire),
            .rejected_frames = atomic_load_explicit(&failure->rejected_frames, memory_order_relaxed),
            .first_position = atomic_load_explicit(&failure->first_position, memory_order_relaxed),
            .last_position = atomic_load_explicit(&failure->last_position, memory_order_relaxed),
            .first_clock = atomic_load_explicit(&failure->first_clock, memory_order_relaxed),
            .last_clock = atomic_load_explicit(&failure->last_clock, memory_order_relaxed),
        };
    }
    return result;
}
