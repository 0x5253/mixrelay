#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif

/* Owned lab-only source/sink. Generates PCM directly in PipeWire callbacks and
 * measures sink PCM independently of MixRelay's meters or transport rings.
 * Never autoconnects or changes the session's default devices. */
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdbool.h>
#include <fcntl.h>
#include <sched.h>
#include <sys/stat.h>
#include <unistd.h>


/* Fixed-size callback observations, printed only after the main loop stops.
 * pw_buffer.time (PipeWire >= 1.0.5) is the capture buffer's native cycle time,
 * not a guessed timestamp derived from sender start or recorder launch. */
typedef struct {
    uint64_t start_frame, end_frame, zero_frames, buffer_cycle_ns, start_buffer_cycle_ns;
    uint32_t frame_in_buffer, buffer_frames;
} GapObservation;

typedef struct {
    uint64_t frame, previous_cycle_ns, cycle_ns, previous_frames;
    uint64_t previous_entry_ns, entry_ns;
    struct pw_time report;
    int report_result, report_valid;
} CycleObservation;

enum { GAP_OBSERVATION_LIMIT = 8 };
enum {
    SINK_PCM_CAPTURE_RATE = 48000,
    /* Finite lab budget includes device setup/linking plus the unchanged 60 s sender run. */
    SINK_PCM_CAPTURE_SECONDS = 120,
    SINK_PCM_CAPTURE_CHANNELS = 2,
};
#define SINK_PCM_CAPTURE_MAX_FRAMES ((uint64_t)SINK_PCM_CAPTURE_RATE * SINK_PCM_CAPTURE_SECONDS)

typedef struct {
    struct pw_stream *stream;
    uint32_t channels;
    int source;
    uint64_t position, frames, active, invalid, gap, maximum_gap;
    double peaks[64];
    GapObservation gaps[GAP_OBSERVATION_LIMIT];
    GapObservation gap_start;
    uint64_t gap_count, first_buffer_cycle_ns;
    float *capture_pcm;
    uint64_t capture_capacity_frames, capture_frames, capture_observed_frames;
    uint64_t capture_callbacks, capture_invalid_callbacks, capture_zero_frame_callbacks;
    uint64_t capture_cycle_discontinuities, capture_missing_timestamps;
    uint64_t capture_first_cycle_ns, capture_last_cycle_ns;
    uint64_t capture_last_cycle_delta_ns, capture_last_expected_cycle_delta_ns;
    uint64_t capture_previous_cycle_ns, capture_previous_frames;
    CycleObservation capture_cycles[GAP_OBSERVATION_LIMIT];
    uint64_t dequeue_nulls, queue_failures;
    uint64_t source_first_cycle_ns, source_last_cycle_ns, source_missing_timestamps;
    uint64_t source_previous_cycle_ns, source_previous_frames, source_previous_entry_ns;
    uint64_t source_cycle_discontinuities;
    CycleObservation source_cycles[GAP_OBSERVATION_LIMIT];
    uint64_t capture_previous_entry_ns, callback_entry_ns, initial_callback_tid;
    struct pw_time callback_report;
    uint64_t report_failures, report_missing_times, entry_time_unavailable;
    int diagnostic, callback_tid_recorded, callback_report_result, callback_report_valid;
    int source_has_previous_cycle;
    int capture_has_previous_cycle, capture_cap_hit;
} Fixture;
static volatile sig_atomic_t stopping;
static volatile sig_atomic_t resetting;
static volatile sig_atomic_t toggle_source, toggle_sink;
static void stop(int signal) { (void)signal; stopping = 1; }
static void reset(int signal) { (void)signal; resetting = 1; }
static void toggle(int signal) { if (signal == SIGUSR2) toggle_source = 1; else toggle_sink = 1; }

static int reset_sink_counters(struct spa_loop *loop, bool async, uint32_t seq,
                               const void *data, size_t size, void *user_data) {
    (void)loop; (void)async; (void)seq; (void)data; (void)size;
    Fixture *sink = user_data;
    sink->frames = sink->active = sink->invalid = sink->gap = sink->maximum_gap = 0;
    sink->gap_count = sink->first_buffer_cycle_ns = 0;
    memset(sink->peaks, 0, sizeof(sink->peaks));
    memset(sink->gaps, 0, sizeof(sink->gaps));
    return 0;
}

/* Control side only: wait until reset is serialized with the sink callback.
 * With no stream there is no callback owner; retain the same reset meaning. */
static int reset_sink(Fixture *sink) {
    if (!sink->stream) return reset_sink_counters(NULL, false, 0, NULL, 0, sink);
    struct pw_loop *loop = pw_stream_get_data_loop(sink->stream);
    if (!loop) return -EIO;
    return pw_loop_invoke(loop, reset_sink_counters, 0, NULL, 0, true, sink);
}

static void observe_activity(Fixture *f, int active, GapObservation location) {
    if (!active) {
        if (!f->gap) f->gap_start = location;
        f->gap++;
        return;
    }
    if (f->active && f->gap > f->maximum_gap) f->maximum_gap = f->gap;
    if (f->active && f->gap > 2) {
        location.zero_frames = f->gap;
        location.start_frame = f->gap_start.end_frame;
        location.start_buffer_cycle_ns = f->gap_start.buffer_cycle_ns;
        /* Retain the latest bounded observations, not only startup gaps. */
        f->gaps[f->gap_count % GAP_OBSERVATION_LIMIT] = location;
        f->gap_count++;
    }
    f->active++;
    f->gap = 0;
}

static const GapObservation *gap_observation(const Fixture *f, uint64_t index) {
    uint64_t first = f->gap_count > GAP_OBSERVATION_LIMIT ? f->gap_count - GAP_OBSERVATION_LIMIT : 0;
    return &f->gaps[(first + index) % GAP_OBSERVATION_LIMIT];
}

static uint64_t frames_to_ns(uint64_t frames) {
    return (frames * 1000000000ULL) / SINK_PCM_CAPTURE_RATE;
}

static const CycleObservation *cycle_observation(const Fixture *f, uint64_t index) {
    const uint64_t first = f->capture_cycle_discontinuities > GAP_OBSERVATION_LIMIT
        ? f->capture_cycle_discontinuities - GAP_OBSERVATION_LIMIT : 0;
    return &f->capture_cycles[(first + index) % GAP_OBSERVATION_LIMIT];
}

static const CycleObservation *source_cycle_observation(const Fixture *f, uint64_t index) {
    const uint64_t first = f->source_cycle_discontinuities > GAP_OBSERVATION_LIMIT
        ? f->source_cycle_discontinuities - GAP_OBSERVATION_LIMIT : 0;
    return &f->source_cycles[(first + index) % GAP_OBSERVATION_LIMIT];
}

/* Only this stream's serialized callback writes these preallocated records.
 * Entry time is distinct from the native report update time. Missing reports
 * break the pair rather than inventing a cycle discontinuity. */
static void observe_source_cycle(Fixture *f, uint64_t frames) {
    if (!f->callback_report_valid) {
        f->source_has_previous_cycle = 0;
        return;
    }
    const uint64_t cycle_ns = (uint64_t)f->callback_report.now;
    if (f->source_has_previous_cycle) {
        const uint64_t expected = frames_to_ns(f->source_previous_frames);
        const uint64_t actual = cycle_ns > f->source_previous_cycle_ns
            ? cycle_ns - f->source_previous_cycle_ns : 0;
        const uint64_t difference = actual > expected ? actual - expected : expected - actual;
        if (difference > 1) {
            f->source_cycles[f->source_cycle_discontinuities % GAP_OBSERVATION_LIMIT] = (CycleObservation) {
                .frame = f->position,
                .previous_cycle_ns = f->source_previous_cycle_ns,
                .cycle_ns = cycle_ns,
                .previous_frames = f->source_previous_frames,
                .previous_entry_ns = f->source_previous_entry_ns,
                .entry_ns = f->callback_entry_ns,
                .report = f->callback_report,
                .report_result = f->callback_report_result,
                .report_valid = f->callback_report_valid,
            };
            f->source_cycle_discontinuities++;
        }
    }
    f->source_previous_cycle_ns = cycle_ns;
    f->source_previous_frames = frames;
    f->source_previous_entry_ns = f->callback_entry_ns;
    f->source_has_previous_cycle = 1;
}

/* Called only from the serialized sink callback. This copies complete raw F32
 * stereo frames to memory allocated before pw_init(); it never grows storage. */
static int capture_sink_pcm(Fixture *f, const void *pcm, size_t byte_count, uint64_t cycle_ns) {
    const size_t stride = SINK_PCM_CAPTURE_CHANNELS * sizeof(float);
    if (!f->capture_pcm) return 0;
    if ((byte_count % stride) != 0 || (byte_count != 0 && pcm == NULL)) return EINVAL;

    const uint64_t frames = byte_count / stride;
    if (UINT64_MAX - f->capture_observed_frames < frames) return EOVERFLOW;
    f->capture_callbacks++;
    f->capture_observed_frames += frames;
    if (frames == 0) f->capture_zero_frame_callbacks++;

    if (cycle_ns == 0) {
        f->capture_missing_timestamps++;
        f->capture_has_previous_cycle = 0;
    } else {
        if (f->capture_first_cycle_ns == 0) f->capture_first_cycle_ns = cycle_ns;
        if (f->capture_has_previous_cycle) {
            const uint64_t expected = frames_to_ns(f->capture_previous_frames);
            const uint64_t actual = cycle_ns > f->capture_previous_cycle_ns
                ? cycle_ns - f->capture_previous_cycle_ns : 0;
            const uint64_t difference = actual > expected ? actual - expected : expected - actual;
            f->capture_last_expected_cycle_delta_ns = expected;
            f->capture_last_cycle_delta_ns = actual;
            if (difference > 1) {
                f->capture_cycles[f->capture_cycle_discontinuities % GAP_OBSERVATION_LIMIT] = (CycleObservation) {
                    .frame = f->capture_observed_frames - frames,
                    .previous_cycle_ns = f->capture_previous_cycle_ns,
                    .cycle_ns = cycle_ns,
                    .previous_frames = f->capture_previous_frames,
                    .previous_entry_ns = f->capture_previous_entry_ns,
                    .entry_ns = f->callback_entry_ns,
                    .report = f->callback_report,
                    .report_result = f->callback_report_result,
                    .report_valid = f->callback_report_valid,
                };
                f->capture_cycle_discontinuities++;
            }
        }
        f->capture_last_cycle_ns = cycle_ns;
        f->capture_previous_cycle_ns = cycle_ns;
        f->capture_previous_frames = frames;
        f->capture_previous_entry_ns = f->callback_entry_ns;
        f->capture_has_previous_cycle = 1;
    }

    const uint64_t available = f->capture_frames < f->capture_capacity_frames
        ? f->capture_capacity_frames - f->capture_frames : 0;
    const uint64_t copied = frames < available ? frames : available;
    if (copied != 0) {
        const size_t sample_offset = (size_t)f->capture_frames * SINK_PCM_CAPTURE_CHANNELS;
        const size_t sample_count = (size_t)copied * SINK_PCM_CAPTURE_CHANNELS;
        memcpy(f->capture_pcm + sample_offset, pcm, sample_count * sizeof(float));
        f->capture_frames += copied;
    }
    if (copied != frames) f->capture_cap_hit = 1;
    return 0;
}

static int create_capture_file(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, S_IRUSR | S_IWUSR);
    if (fd < 0) return -1;
    if (fchmod(fd, S_IRUSR | S_IWUSR) < 0) {
        int error = errno;
        if (close(fd) < 0) fprintf(stderr, "close newly created sink PCM file %s: %s\n", path, strerror(errno));
        if (unlink(path) < 0 && errno != ENOENT)
            fprintf(stderr, "remove newly created sink PCM file %s: %s\n", path, strerror(errno));
        errno = error;
        return -1;
    }
    return fd;
}

static int write_all(int fd, const void *buffer, size_t byte_count) {
    const uint8_t *bytes = buffer;
    size_t offset = 0;
    while (offset < byte_count) {
        ssize_t written = write(fd, bytes + offset, byte_count - offset);
        if (written < 0 && errno == EINTR) continue;
        if (written < 0) return -1;
        if (written == 0) { errno = EIO; return -1; }
        offset += (size_t)written;
    }
    return 0;
}

static int discard_capture_file(const char *path, int *fd, int *owned) {
    int result = 0;
    int error = 0;
    if (*fd >= 0) {
        if (close(*fd) < 0) { result = -1; error = errno; }
        *fd = -1;
    }
    if (*owned) {
        if (unlink(path) < 0 && errno != ENOENT && result == 0) { result = -1; error = errno; }
        *owned = 0;
    }
    if (result < 0) errno = error;
    return result;
}

static int finish_capture_file(const char *path, int *fd, int *owned, const Fixture *sink) {
    if (sink->capture_frames == 0 || sink->capture_frames > SIZE_MAX / (SINK_PCM_CAPTURE_CHANNELS * sizeof(float))) {
        int error = sink->capture_frames == 0 ? ENODATA : EOVERFLOW;
        if (discard_capture_file(path, fd, owned) < 0)
            fprintf(stderr, "remove incomplete sink PCM capture file %s: %s\n", path, strerror(errno));
        errno = error;
        return -1;
    }
    const size_t byte_count = (size_t)sink->capture_frames * SINK_PCM_CAPTURE_CHANNELS * sizeof(float);
    if (write_all(*fd, sink->capture_pcm, byte_count) < 0) {
        int error = errno;
        if (discard_capture_file(path, fd, owned) < 0)
            fprintf(stderr, "remove incomplete sink PCM capture file %s: %s\n", path, strerror(errno));
        errno = error;
        return -1;
    }
    if (close(*fd) < 0) {
        int error = errno;
        *fd = -1;
        if (discard_capture_file(path, fd, owned) < 0)
            fprintf(stderr, "remove sink PCM capture file after close failure %s: %s\n", path, strerror(errno));
        errno = error;
        return -1;
    }
    *fd = -1;
    *owned = 0;
    return 0;
}

static int normal_scheduler_policy(int current_policy) {
    return SCHED_OTHER | (current_policy & SCHED_RESET_ON_FORK);
}

static int restore_normal_scheduler(void) {
    struct sched_param normal = { .sched_priority = 0 };
    int previous_policy = sched_getscheduler(0);
    if (previous_policy < 0) return -1;
    const int reset_on_fork = previous_policy & SCHED_RESET_ON_FORK;
    fprintf(stderr, "pre-demotion scheduler raw_policy=%d reset_on_fork=%d\n",
        previous_policy, reset_on_fork != 0);
    if (sched_setscheduler(0, normal_scheduler_policy(previous_policy), &normal) < 0) return -1;
    int policy = sched_getscheduler(0);
    if (policy < 0) return -1;
    struct sched_param actual;
    if (sched_getparam(0, &actual) < 0) return -1;
    if ((policy & ~SCHED_RESET_ON_FORK) != SCHED_OTHER ||
        (policy & SCHED_RESET_ON_FORK) != reset_on_fork || actual.sched_priority != 0) {
        errno = EPERM;
        return -1;
    }
    fprintf(stderr, "post-loop scheduler raw_policy=%d policy=%d priority=%d reset_on_fork=%d\n",
        policy, policy & ~SCHED_RESET_ON_FORK, actual.sched_priority,
        (policy & SCHED_RESET_ON_FORK) != 0);
    return 0;
}

static void print_cycle_observation(const CycleObservation *cycle) {
    printf("{\"frame\":%lu,\"previousCycleNs\":%lu,\"cycleNs\":%lu,\"previousFrames\":%lu,\"previousEntryNs\":%lu,\"entryNs\":%lu,\"entryPairValid\":%s,\"reportResult\":%d,\"reportValid\":%s,\"report\":{\"now\":%ld,\"ticks\":%lu,\"rateNum\":%u,\"rateDenom\":%u,\"delayTicks\":%ld,\"queuedRawSizeSum\":%lu,\"bufferedFrames\":%lu,\"queuedBuffers\":%u,\"availableBuffers\":%u}}",
        cycle->frame, cycle->previous_cycle_ns, cycle->cycle_ns, cycle->previous_frames,
        cycle->previous_entry_ns, cycle->entry_ns,
        cycle->previous_entry_ns && cycle->entry_ns ? "true" : "false",
        cycle->report_result, cycle->report_valid ? "true" : "false",
        cycle->report.now, cycle->report.ticks, cycle->report.rate.num, cycle->report.rate.denom,
        cycle->report.delay, cycle->report.queued, cycle->report.buffered,
        cycle->report.queued_buffers, cycle->report.avail_buffers);
}

static void print_summary(const Fixture *sink, const Fixture *source) {
    printf("{\"frames\":%lu,\"active\":%lu,\"invalid\":%lu,\"maximumGap\":%lu,\"peaks\":[",
        sink->frames, sink->active, sink->invalid, sink->maximum_gap);
    for (uint32_t c = 0; c < sink->channels; c++) printf("%s%.9f", c ? "," : "", sink->peaks[c]);
    uint64_t dropped = sink->gap_count > GAP_OBSERVATION_LIMIT ? sink->gap_count - GAP_OBSERVATION_LIMIT : 0;
    printf("],\"firstBufferCycleNs\":%lu,\"gapCount\":%lu,\"droppedGapObservations\":%lu,\"gaps\":[",
        sink->first_buffer_cycle_ns, sink->gap_count, dropped);
    for (uint64_t i = 0; i < sink->gap_count && i < GAP_OBSERVATION_LIMIT; i++) {
        const GapObservation *gap = gap_observation(sink, i);
        printf("%s{\"startFrame\":%lu,\"endFrame\":%lu,\"zeroFrames\":%lu,\"startBufferCycleNs\":%lu,\"bufferCycleNs\":%lu,\"frameInBuffer\":%u,\"bufferFrames\":%u}",
            i ? "," : "", gap->start_frame, gap->end_frame, gap->zero_frames,
            gap->start_buffer_cycle_ns, gap->buffer_cycle_ns, gap->frame_in_buffer, gap->buffer_frames);
    }
    printf("],\"sourceTimeline\":{\"producedFrames\":%lu,\"firstCycleNs\":%lu,\"lastCycleNs\":%lu,\"missingTimestamps\":%lu,\"dequeueNulls\":%lu,\"queueFailures\":%lu},\"sinkBufferCounters\":{\"dequeueNulls\":%lu,\"queueFailures\":%lu}",
        source->position, source->source_first_cycle_ns, source->source_last_cycle_ns,
        source->source_missing_timestamps, source->dequeue_nulls, source->queue_failures,
        sink->dequeue_nulls, sink->queue_failures);
    if (sink->capture_pcm) {
        printf(",\"sinkPcmCapture\":{\"sampleRate\":%u,\"channels\":%u,\"capacityFrames\":%lu,\"capturedFrames\":%lu,\"observedFrames\":%lu,\"callbacks\":%lu,\"invalidCallbacks\":%lu,\"zeroFrameCallbacks\":%lu,\"capHit\":%s,\"firstCycleNs\":%lu,\"lastCycleNs\":%lu,\"missingTimestamps\":%lu,\"cycleDiscontinuities\":%lu,\"lastCycleDeltaNs\":%lu,\"lastExpectedCycleDeltaNs\":%lu,\"rawBytes\":%lu,\"droppedCycleObservations\":%lu,\"cycleEvents\":[",
            SINK_PCM_CAPTURE_RATE, SINK_PCM_CAPTURE_CHANNELS, sink->capture_capacity_frames,
            sink->capture_frames, sink->capture_observed_frames, sink->capture_callbacks,
            sink->capture_invalid_callbacks, sink->capture_zero_frame_callbacks,
            sink->capture_cap_hit ? "true" : "false", sink->capture_first_cycle_ns,
            sink->capture_last_cycle_ns, sink->capture_missing_timestamps,
            sink->capture_cycle_discontinuities, sink->capture_last_cycle_delta_ns,
            sink->capture_last_expected_cycle_delta_ns,
            sink->capture_frames * SINK_PCM_CAPTURE_CHANNELS * sizeof(float),
            sink->capture_cycle_discontinuities > GAP_OBSERVATION_LIMIT
                ? sink->capture_cycle_discontinuities - GAP_OBSERVATION_LIMIT : 0);
        for (uint64_t i = 0; i < sink->capture_cycle_discontinuities && i < GAP_OBSERVATION_LIMIT; i++) {
            const CycleObservation *cycle = cycle_observation(sink, i);
            if (i) putchar(',');
            print_cycle_observation(cycle);
        }
        printf("]}");
    }
    if (source->diagnostic) {
        printf(",\"callbackDiagnostics\":{\"scope\":\"instrumented-only; initial TID is not a per-callback TID trace\",\"reportPhase\":\"after dequeue, before queue\",\"queuedUnits\":\"raw pw_buffer.size sum; zero does not prove empty FIFO\",\"source\":{\"initialCallbackTid\":%lu,\"initialCallbackTidValid\":%s,\"reportFailures\":%lu,\"reportMissingTimes\":%lu,\"entryTimeUnavailable\":%lu,\"cycleDiscontinuities\":%lu,\"droppedCycleObservations\":%lu,\"cycleEvents\":[",
            source->initial_callback_tid, source->initial_callback_tid ? "true" : "false",
            source->report_failures, source->report_missing_times, source->entry_time_unavailable,
            source->source_cycle_discontinuities,
            source->source_cycle_discontinuities > GAP_OBSERVATION_LIMIT
                ? source->source_cycle_discontinuities - GAP_OBSERVATION_LIMIT : 0);
        for (uint64_t i = 0; i < source->source_cycle_discontinuities && i < GAP_OBSERVATION_LIMIT; i++) {
            if (i) putchar(',');
            print_cycle_observation(source_cycle_observation(source, i));
        }
        printf("]},\"sink\":{\"initialCallbackTid\":%lu,\"initialCallbackTidValid\":%s,\"reportFailures\":%lu,\"reportMissingTimes\":%lu,\"entryTimeUnavailable\":%lu}}",
            sink->initial_callback_tid, sink->initial_callback_tid ? "true" : "false",
            sink->report_failures, sink->report_missing_times, sink->entry_time_unavailable);
    }
    puts("}");
}

static void process(void *data) {
    Fixture *f = data;
    if (f->diagnostic) {
        f->callback_entry_ns = pw_stream_get_nsec(f->stream);
        if (!f->callback_entry_ns) f->entry_time_unavailable++;
        if (!f->callback_tid_recorded) {
            const pid_t tid = gettid();
            f->initial_callback_tid = tid > 0 ? (uint64_t)tid : 0;
            f->callback_tid_recorded = 1;
        }
    }
    struct pw_buffer *b = pw_stream_dequeue_buffer(f->stream);
    if (!b) { f->dequeue_nulls++; return; }
    struct spa_buffer *buffer = b->buffer;
    if (!buffer || buffer->n_datas != 1) {
        if (!f->source && f->capture_pcm) f->capture_invalid_callbacks++;
        goto done;
    }
    struct spa_data *d = &buffer->datas[0];
    uint32_t stride = f->channels * sizeof(float);
    if (!d->data || !d->chunk) {
        if (!f->source && f->capture_pcm) f->capture_invalid_callbacks++;
        goto done;
    }
    if (f->source) {
        /* Output pw_buffer.time is not a capture timestamp. Query the native
         * stream report without I/O, allocation or locks in this data loop. */
        struct pw_time time = {0};
        const int report_result = pw_stream_get_time_n(f->stream, &time, sizeof(time));
        if (report_result == 0 && time.now > 0) {
            if (!f->source_first_cycle_ns) f->source_first_cycle_ns = (uint64_t)time.now;
            f->source_last_cycle_ns = (uint64_t)time.now;
        } else {
            f->source_missing_timestamps++;
        }
        uint32_t frames = d->maxsize / stride;
        if (b->requested && b->requested < frames) frames = b->requested;
        if (f->diagnostic) {
            f->callback_report = time;
            f->callback_report_result = report_result;
            f->callback_report_valid = report_result == 0 && time.now > 0;
            if (report_result != 0) f->report_failures++;
            else if (time.now <= 0) f->report_missing_times++;
            observe_source_cycle(f, frames);
        }
        float *samples = d->data;
        for (uint32_t n = 0; n < frames; n++, f->position++)
            for (uint32_t c = 0; c < f->channels; c++)
                samples[n * f->channels + c] = (c + 1) / 1024.0 * sin(2.0 * M_PI * (f->position % 48) / 48.0);
        d->chunk->offset = 0; d->chunk->stride = stride; d->chunk->size = frames * stride;
    } else {
        if (f->diagnostic) {
            f->callback_report = (struct pw_time){0};
            f->callback_report_result = pw_stream_get_time_n(f->stream,
                &f->callback_report, sizeof(f->callback_report));
            f->callback_report_valid = f->callback_report_result == 0 && f->callback_report.now > 0;
            if (f->callback_report_result != 0) f->report_failures++;
            else if (f->callback_report.now <= 0) f->report_missing_times++;
        }
        if (d->chunk->offset > d->maxsize || d->chunk->size > d->maxsize - d->chunk->offset || d->chunk->size % stride) {
            f->invalid++;
            if (f->capture_pcm) f->capture_invalid_callbacks++;
            goto done;
        }
        float *samples = (float *)((uint8_t *)d->data + d->chunk->offset);
        uint32_t frames = d->chunk->size / stride;
        if (f->capture_pcm && capture_sink_pcm(f, samples, d->chunk->size, b->time) != 0) {
            f->capture_invalid_callbacks++;
            f->invalid++;
            goto done;
        }
        uint64_t first_frame = f->frames;
        if (!f->frames) f->first_buffer_cycle_ns = b->time;
        f->frames += frames;
        for (uint32_t n = 0; n < frames; n++) {
            int active = 0;
            for (uint32_t c = 0; c < f->channels; c++) {
                double value = fabs(samples[n * f->channels + c]);
                if (!isfinite(value)) { f->invalid++; continue; }
                if (value > f->peaks[c]) f->peaks[c] = value;
                active |= value > 0.0001;
            }
            observe_activity(f, active, (GapObservation) {
                .end_frame = first_frame + n,
                .buffer_cycle_ns = b->time,
                .frame_in_buffer = n,
                .buffer_frames = frames,
            });
        }
    }
done:
    if (pw_stream_queue_buffer(f->stream, b) < 0) f->queue_failures++;
}
static const struct pw_stream_events events = { PW_VERSION_STREAM_EVENTS, .process = process };

static int open_fixture(Fixture *f, struct pw_main_loop *loop, const char *name) {
    struct pw_properties *props = pw_properties_new(PW_KEY_NODE_NAME, name,
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, f->source ? "Playback" : "Capture",
        PW_KEY_MEDIA_CLASS, f->source ? "Audio/Source" : "Audio/Sink",
        /* Keep automatic default selection on these owned simulated devices
         * when the graph creates a lower-priority virtual endpoint. */
        PW_KEY_PRIORITY_SESSION, "100000",
        PW_KEY_NODE_VIRTUAL, "true", "node.pause-on-idle", "false", "node.always-process", "true", NULL);
    pw_properties_setf(props, "audio.channels", "%u", f->channels);
    f->stream = pw_stream_new_simple(pw_main_loop_get_loop(loop), name, props, &events, f);
    if (!f->stream) return -1;
    uint8_t storage[2048];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw format = { .format = SPA_AUDIO_FORMAT_F32, .rate = 48000, .channels = f->channels };
    if (f->channels == 2) { format.position[0] = SPA_AUDIO_CHANNEL_FL; format.position[1] = SPA_AUDIO_CHANNEL_FR; }
    else for (uint32_t c = 0; c < f->channels; c++) format.position[c] = SPA_AUDIO_CHANNEL_AUX0 + c;
    const struct spa_pod *param = spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &format);
    return pw_stream_connect(f->stream, f->source ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT, PW_ID_ANY,
        PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS, &param, 1);
}
int main(int argc, char **argv) {
    if (argc != 3 && argc != 4) return 2;
    Fixture source = { .source = 1, .channels = (uint32_t)atoi(argv[1]) };
    Fixture sink = { .channels = (uint32_t)atoi(argv[2]) };
    if (!source.channels || source.channels > 64 || !sink.channels || sink.channels > 64) return 2;
    const char *capture_path = argc == 4 ? argv[3] : NULL;
    source.diagnostic = sink.diagnostic = capture_path != NULL;
    int capture_fd = -1;
    int capture_file_owned = 0;
    if (capture_path) {
        if (!capture_path[0] || sink.channels != SINK_PCM_CAPTURE_CHANNELS ||
            SINK_PCM_CAPTURE_MAX_FRAMES > SIZE_MAX / (SINK_PCM_CAPTURE_CHANNELS * sizeof(float))) return 2;
        sink.capture_capacity_frames = SINK_PCM_CAPTURE_MAX_FRAMES;
        sink.capture_pcm = malloc((size_t)sink.capture_capacity_frames * SINK_PCM_CAPTURE_CHANNELS * sizeof(float));
        if (!sink.capture_pcm) { perror("sink PCM capture allocation"); return 1; }
        volatile float *capture_pre_touch = sink.capture_pcm;
        const uint64_t capture_samples = sink.capture_capacity_frames * SINK_PCM_CAPTURE_CHANNELS;
        for (uint64_t sample = 0; sample < capture_samples; sample++) capture_pre_touch[sample] = 0.0f;
        capture_fd = create_capture_file(capture_path);
        if (capture_fd < 0) {
            int error = errno;
            free(sink.capture_pcm);
            errno = error;
            perror("create sink PCM capture file");
            return 1;
        }
        capture_file_owned = 1;
    }
    pw_init(NULL, NULL);
    struct pw_main_loop *loop = pw_main_loop_new(NULL);
    int result = 1;
    if (!loop) goto cleanup;
    if (open_fixture(&source, loop, "io.mixrelay.lab-device-source") < 0 ||
        open_fixture(&sink, loop, "io.mixrelay.lab-device-sink") < 0) goto cleanup;
    signal(SIGTERM, stop); signal(SIGINT, stop); signal(SIGUSR1, reset);
    signal(SIGUSR2, toggle); signal(SIGHUP, toggle);
    /* RT callbacks own observations; control resets are synchronized on their loop.
     * Reporting and file writes remain after both streams have been destroyed. */
    while (!stopping) {
        int iteration = pw_loop_iterate(pw_main_loop_get_loop(loop), 50);
        if (iteration < 0 && iteration != -EINTR) { fprintf(stderr, "main loop: %d\n", iteration); goto cleanup; }
        if (resetting) {
            int reset_result = reset_sink(&sink);
            if (reset_result < 0) {
                fprintf(stderr, "sink reset: %d\n", reset_result);
                goto cleanup;
            }
            resetting = 0;
        }
        /* Independent removal/arrival with stable names and fresh native serials. */
        if (toggle_source) {
            if (source.stream) { pw_stream_destroy(source.stream); source.stream = NULL; }
            else if (open_fixture(&source, loop, "io.mixrelay.lab-device-source") < 0) goto cleanup;
            toggle_source = 0;
        }
        if (toggle_sink) {
            if (sink.stream) { pw_stream_destroy(sink.stream); sink.stream = NULL; }
            else if (open_fixture(&sink, loop, "io.mixrelay.lab-device-sink") < 0) goto cleanup;
            toggle_sink = 0;
        }
    }
    result = 0;
cleanup:
    if (source.stream) pw_stream_destroy(source.stream);
    if (sink.stream) pw_stream_destroy(sink.stream);
    if (loop) pw_main_loop_destroy(loop);
    if (capture_path && restore_normal_scheduler() < 0) {
        int error = errno;
        fprintf(stderr, "restore fixture main-thread scheduler: %s\n", strerror(error));
        result = 1;
    }
    pw_deinit();
    if (capture_path) {
        if (result == 0) {
            if (finish_capture_file(capture_path, &capture_fd, &capture_file_owned, &sink) < 0) {
                int error = errno;
                fprintf(stderr, "write sink PCM capture file: %s\n", strerror(error));
                result = 1;
            }
        } else if (discard_capture_file(capture_path, &capture_fd, &capture_file_owned) < 0) {
            fprintf(stderr, "remove incomplete sink PCM capture file: %s\n", strerror(errno));
        }
        if (result == 0) print_summary(&sink, &source);
    } else if (result == 0) {
        print_summary(&sink, &source);
    }
    free(sink.capture_pcm);
    return result;
}
