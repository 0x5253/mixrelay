#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <pipewire/pipewire.h>
static struct pw_loop *reset_test_loop;
static struct pw_loop *reset_test_get_data_loop(struct pw_stream *stream) {
    assert(stream != NULL);
    return reset_test_loop;
}
#define pw_stream_get_data_loop reset_test_get_data_loop
#define main fixture_main
#include "MRDeviceFixture.c"
#undef main
#undef pw_stream_get_data_loop

static int reset_test_invoke(void *object, spa_invoke_func_t func, uint32_t seq,
                             const void *data, size_t size, bool block, void *user_data) {
    int *failure = object;
    assert(func == reset_sink_counters && block && seq == 0 && data == NULL && size == 0);
    if (*failure) return *failure;
    return func(NULL, true, seq, data, size, user_data);
}

static void test_sink_reset(void) {
    Fixture sink = {
        .channels = 2, .frames = 8, .active = 7, .invalid = 6, .gap = 5,
        .maximum_gap = 4, .gap_count = 3, .first_buffer_cycle_ns = 2,
        .peaks = {0.5}, .gaps = {{.end_frame = 8}},
        .gap_start = {.end_frame = 9}, .capture_frames = 10,
        .capture_previous_cycle_ns = 11, .queue_failures = 12,
    };
    Fixture expected = sink;
    expected.frames = expected.active = expected.invalid = expected.gap = expected.maximum_gap = 0;
    expected.gap_count = expected.first_buffer_cycle_ns = 0;
    memset(expected.peaks, 0, sizeof(expected.peaks));
    memset(expected.gaps, 0, sizeof(expected.gaps));
    /* Absent stream resets locally without touching PCM/timeline counters. */
    assert(reset_sink(&sink) == 0 && memcmp(&sink, &expected, sizeof(sink)) == 0);
    sink.frames = 8;
    sink.stream = (struct pw_stream *)&sink; /* Opaque identity, never dereferenced. */
    Fixture original = sink;
    reset_test_loop = NULL;
    assert(reset_sink(&sink) == -EIO && memcmp(&sink, &original, sizeof(sink)) == 0);
    int failure = -EPIPE;
    const struct spa_loop_methods methods = { .version = SPA_VERSION_LOOP_METHODS, .invoke = reset_test_invoke };
    struct spa_loop spa_loop = { .iface = SPA_INTERFACE_INIT(SPA_TYPE_INTERFACE_Loop, SPA_VERSION_LOOP, &methods, &failure) };
    struct pw_loop loop = { .loop = &spa_loop };
    reset_test_loop = &loop;
    assert(reset_sink(&sink) == -EPIPE && memcmp(&sink, &original, sizeof(sink)) == 0);
    failure = 0;
    expected.stream = sink.stream;
    assert(reset_sink(&sink) == 0 && memcmp(&sink, &expected, sizeof(sink)) == 0);
    reset_test_loop = NULL;
}

static void test_capture_copy_and_timing(void) {
    float storage[6] = {0};
    const float first[] = { 1.0f, 2.0f, 3.0f, 4.0f };
    const float second[] = { 5.0f, 6.0f, 7.0f, 8.0f };
    Fixture fixture = {
        .channels = SINK_PCM_CAPTURE_CHANNELS,
        .capture_pcm = storage,
        .capture_capacity_frames = 3,
        .callback_entry_ns = 1010000,
    };

    assert(capture_sink_pcm(&fixture, first, sizeof(first), 1000000) == 0);
    fixture.callback_entry_ns = 1051667;
    assert(capture_sink_pcm(&fixture, second, sizeof(second), 1041667) == 0);
    assert(fixture.capture_cycle_discontinuities == 0); /* 1 ns rounding is valid. */
    fixture.callback_entry_ns = 2010000;
    fixture.callback_report_valid = 1;
    fixture.callback_report = (struct pw_time) {
        .now = 1999999, .ticks = 96, .rate = {1, 48000}, .delay = -128,
        .queued = 0, .buffered = 3, .queued_buffers = 1, .avail_buffers = 2,
    };
    assert(capture_sink_pcm(&fixture, second, sizeof(second), 2000000) == 0);
    assert(fixture.capture_cycle_discontinuities == 1);
    const CycleObservation *cycle = cycle_observation(&fixture, 0);
    assert(cycle->frame == 4); /* Observed absolute index, not capped copied index 3. */
    assert(cycle->previous_cycle_ns == 1041667 && cycle->cycle_ns == 2000000);
    assert(cycle->previous_frames == 2);
    assert(cycle->previous_entry_ns == 1051667 && cycle->entry_ns == 2010000);
    assert(cycle->report_valid && cycle->report.now == 1999999);
    assert(cycle->report.delay == -128 && cycle->report.queued == 0);
    assert(cycle->report.buffered == 3 && cycle->report.queued_buffers == 1);
    assert(cycle->report.avail_buffers == 2);
    assert(fixture.capture_callbacks == 3 && fixture.capture_observed_frames == 6);
    assert(fixture.capture_frames == 3 && fixture.capture_cap_hit == 1);
    assert(storage[0] == 1.0f && storage[1] == 2.0f);
    assert(storage[2] == 3.0f && storage[3] == 4.0f);
    assert(storage[4] == 5.0f && storage[5] == 6.0f);

    assert(capture_sink_pcm(&fixture, NULL, 8, 0) == EINVAL);
    assert(capture_sink_pcm(&fixture, first, 7, 0) == EINVAL);
    assert(fixture.capture_callbacks == 3 && fixture.capture_frames == 3);
    assert(capture_sink_pcm(&fixture, NULL, 0, 0) == 0);
    assert(fixture.capture_callbacks == 4 && fixture.capture_zero_frame_callbacks == 1);
    assert(fixture.capture_missing_timestamps == 1);
    assert(fixture.capture_cycle_discontinuities == 1);
    /* A missing timestamp breaks the pair; the next valid report cannot invent
     * a discontinuity against the stale previous report. */
    assert(capture_sink_pcm(&fixture, first, sizeof(first), 9000000) == 0);
    assert(fixture.capture_cycle_discontinuities == 1);
}

static void test_source_cycle_observations(void) {
    Fixture source = {
        .source = 1, .diagnostic = 1, .callback_report_valid = 1,
        .callback_entry_ns = 1010000,
        .callback_report = { .now = 1000000, .rate = {1, 48000} },
    };
    observe_source_cycle(&source, 128);
    source.position = 128;
    source.callback_entry_ns = 3676667;
    source.callback_report.now = 3666667;
    observe_source_cycle(&source, 128);
    assert(source.source_cycle_discontinuities == 0); /* Same 1 ns rounding allowance. */
    for (uint64_t event = 1; event <= 10; event++) {
        source.position += 128;
        source.callback_entry_ns += 5333333;
        source.callback_report.now += 5333333;
        source.callback_report.ticks += 256;
        source.callback_report.delay = -128;
        source.callback_report.queued_buffers = 1;
        observe_source_cycle(&source, 128);
    }
    assert(source.source_cycle_discontinuities == 10);
    for (uint64_t i = 0; i < GAP_OBSERVATION_LIMIT; i++) {
        const CycleObservation *cycle = source_cycle_observation(&source, i);
        assert(cycle->frame == (i + 4) * 128);
        assert(cycle->previous_frames == 128);
        assert(cycle->cycle_ns - cycle->previous_cycle_ns == 5333333);
        assert(cycle->entry_ns - cycle->previous_entry_ns == 5333333);
        assert(cycle->report_valid && cycle->report.now == (int64_t)cycle->cycle_ns);
        assert(cycle->report.ticks == (i + 3) * 256);
        assert(cycle->report.delay == -128 && cycle->report.queued_buffers == 1);
    }
    const Fixture snapshot = source;
    const Fixture sink = {0};
    print_summary(&sink, &source);
    assert(memcmp(&source, &snapshot, sizeof(source)) == 0);

    source.callback_report_result = -EIO;
    source.callback_report_valid = 0;
    observe_source_cycle(&source, 128);
    assert(!source.source_has_previous_cycle && source.source_cycle_discontinuities == 10);
    source.callback_report_valid = 1;
    source.callback_report_result = 0;
    source.callback_report.now = 100000000;
    observe_source_cycle(&source, 128);
    assert(source.source_cycle_discontinuities == 10); /* No invented pair across missing report. */
    source.callback_report.now = 0;
    source.callback_report_valid = 0;
    observe_source_cycle(&source, 128);
    assert(!source.source_has_previous_cycle && source.source_cycle_discontinuities == 10);
    source.callback_report.now = 100000000;
    source.callback_report_valid = 1;
    observe_source_cycle(&source, 128);
    observe_source_cycle(&source, 128);
    assert(source.source_cycle_discontinuities == 11); /* Duplicate native cycle remains recorded. */
    source.callback_report.now = 99000000;
    source.callback_entry_ns = 0;
    observe_source_cycle(&source, 128);
    const CycleObservation *backward = source_cycle_observation(&source, 7);
    assert(source.source_cycle_discontinuities == 12);
    assert(backward->previous_cycle_ns == 100000000 && backward->cycle_ns == 99000000);
    assert(backward->entry_ns == 0); /* Missing entry remains unavailable, not synthetic. */
}

static void test_cycle_observation_latest_eight(void) {
    float storage[2] = {0};
    const float samples[] = { 0.25f, -0.25f };
    Fixture fixture = {
        .channels = SINK_PCM_CAPTURE_CHANNELS,
        .capture_pcm = storage,
        .capture_capacity_frames = 1,
    };
    assert(capture_sink_pcm(&fixture, samples, sizeof(samples), 1000000) == 0);
    for (uint64_t event = 1; event <= 10; event++) {
        assert(capture_sink_pcm(&fixture, samples, sizeof(samples),
            1000000 + event * 1000000) == 0);
    }
    assert(fixture.capture_cycle_discontinuities == 10);
    assert(fixture.capture_frames == 1 && fixture.capture_observed_frames == 11);
    assert(fixture.capture_cap_hit == 1);
    for (uint64_t i = 0; i < GAP_OBSERVATION_LIMIT; i++) {
        const CycleObservation *cycle = cycle_observation(&fixture, i);
        const uint64_t event = i + 3;
        assert(cycle->frame == event);
        assert(cycle->previous_cycle_ns == event * 1000000);
        assert(cycle->cycle_ns == (event + 1) * 1000000);
        assert(cycle->previous_frames == 1);
    }
    /* Reporting only reads the retained ring after callback activity ends. */
    const Fixture snapshot = fixture;
    for (uint64_t i = 0; i < GAP_OBSERVATION_LIMIT; i++) {
        assert(memcmp(cycle_observation(&fixture, i), cycle_observation(&snapshot, i),
            sizeof(CycleObservation)) == 0);
    }
    assert(memcmp(&fixture, &snapshot, sizeof(fixture)) == 0);
    const Fixture source = { .source = 1, .position = 11,
        .source_first_cycle_ns = 1000000, .source_last_cycle_ns = 11000000 };
    print_summary(&fixture, &source);
}

static void test_capture_file_create_write_and_failure(void) {
    char directory[] = "/tmp/mr-device-fixture-test-XXXXXX";
    assert(mkdtemp(directory) != NULL);
    char path[512];
    assert(snprintf(path, sizeof(path), "%s/capture.f32", directory) > 0);
    float samples[] = { 0.25f, -0.25f, 0.5f, -0.5f };
    Fixture fixture = {
        .channels = SINK_PCM_CAPTURE_CHANNELS,
        .capture_pcm = samples,
        .capture_capacity_frames = 2,
        .capture_frames = 2,
    };

    int fd = create_capture_file(path);
    assert(fd >= 0);
    int owned = 1;
    struct stat status;
    assert(fstat(fd, &status) == 0 && (status.st_mode & 0777) == 0600);
    errno = 0;
    assert(create_capture_file(path) < 0 && errno == EEXIST);
    assert(finish_capture_file(path, &fd, &owned, &fixture) == 0);
    assert(fd == -1 && owned == 0);
    assert(stat(path, &status) == 0 && status.st_size == (off_t)sizeof(samples));
    assert((status.st_mode & 0777) == 0600);
    fd = open(path, O_RDONLY | O_CLOEXEC);
    assert(fd >= 0);
    float actual[4] = {0};
    assert(read(fd, actual, sizeof(actual)) == (ssize_t)sizeof(actual));
    assert(memcmp(actual, samples, sizeof(samples)) == 0);
    assert(close(fd) == 0 && unlink(path) == 0);

    assert(snprintf(path, sizeof(path), "%s/failed.f32", directory) > 0);
    fd = create_capture_file(path);
    assert(fd >= 0 && close(fd) == 0);
    fd = -1;
    owned = 1;
    errno = 0;
    assert(finish_capture_file(path, &fd, &owned, &fixture) < 0 && errno == EBADF);
    assert(fd == -1 && owned == 0 && access(path, F_OK) < 0 && errno == ENOENT);

    assert(snprintf(path, sizeof(path), "%s/empty.f32", directory) > 0);
    fd = create_capture_file(path);
    assert(fd >= 0 && owned == 0);
    owned = 1;
    Fixture empty = { .capture_pcm = samples };
    errno = 0;
    assert(finish_capture_file(path, &fd, &owned, &empty) < 0 && errno == ENODATA);
    assert(fd == -1 && owned == 0 && access(path, F_OK) < 0 && errno == ENOENT);
    assert(rmdir(directory) == 0);
    assert(write_all(-1, samples, sizeof(samples)) < 0 && errno == EBADF);
}

static void test_normal_scheduler_preserves_reset_on_fork(void) {
    assert(normal_scheduler_policy(SCHED_RR | SCHED_RESET_ON_FORK) ==
        (SCHED_OTHER | SCHED_RESET_ON_FORK));
    assert(normal_scheduler_policy(SCHED_RR) == SCHED_OTHER);
    assert(normal_scheduler_policy(SCHED_OTHER | SCHED_RESET_ON_FORK) ==
        (SCHED_OTHER | SCHED_RESET_ON_FORK));
}

int main(void) {
    Fixture fixture = { .channels = 2 };
    GapObservation location = {
        .end_frame = 512, .buffer_cycle_ns = 123456789,
        .frame_in_buffer = 0, .buffer_frames = 128,
    };
    for (int i = 0; i < 256; i++) observe_activity(&fixture, 0, location);
    observe_activity(&fixture, 1, location);
    assert(fixture.gap_count == 0 && fixture.maximum_gap == 0);
    observe_activity(&fixture, 0, location);
    observe_activity(&fixture, 0, location);
    observe_activity(&fixture, 1, location);
    assert(fixture.gap_count == 0 && fixture.maximum_gap == 2);
    for (int i = 0; i < 128; i++) observe_activity(&fixture, 0, location);
    location.end_frame = 640;
    location.buffer_cycle_ns = 126123456;
    observe_activity(&fixture, 1, location);
    assert(fixture.gap_count == 1 && fixture.maximum_gap == 128);
    assert(fixture.gaps[0].zero_frames == 128 && fixture.gaps[0].start_frame == 512);
    assert(fixture.gaps[0].end_frame == 640);
    assert(fixture.gaps[0].start_buffer_cycle_ns == 123456789);
    assert(fixture.gaps[0].buffer_cycle_ns == 126123456);
    assert(fixture.gaps[0].frame_in_buffer == 0 && fixture.gaps[0].buffer_frames == 128);
    for (int event = 1; event < 10; event++) {
        location.end_frame += 256;
        for (int i = 0; i < 128; i++) observe_activity(&fixture, 0, location);
        observe_activity(&fixture, 1, location);
    }
    assert(fixture.gap_count == 10);
    for (uint64_t i = 0; i < GAP_OBSERVATION_LIMIT; i++) {
        const GapObservation *gap = gap_observation(&fixture, i);
        assert(gap->end_frame == 1152 + i * 256);
        assert(gap->zero_frames == 128);
    }
    test_capture_copy_and_timing();
    test_cycle_observation_latest_eight();
    test_source_cycle_observations();
    test_capture_file_create_write_and_failure();
    test_normal_scheduler_preserves_reset_on_fork();
    test_sink_reset();
    assert(restore_normal_scheduler() == 0);
    puts("gap observer and bounded sink PCM capture tests passed");
    return 0;
}
