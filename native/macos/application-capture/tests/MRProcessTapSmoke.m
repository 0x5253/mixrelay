#import "MRProcessTap.h"

#import <Foundation/Foundation.h>
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static bool MeasureCapture(MRProcessTapSession *session, const char *phase,
    double *reference_rms) {
    const MRProcessTapStats before = mr_process_tap_stats(session);
    float samples[4096] = {0};
    uint64_t frames = 0;
    double energy = 0;
    float peak = 0;
    bool finite = true;
    for (size_t iteration = 0; iteration < 200; ++iteration) {
        usleep(10000);
        const size_t captured = mr_process_tap_read(session, samples, 2048);
        frames += captured;
        for (size_t sample = 0; sample < captured * 2; ++sample) {
            const float value = samples[sample];
            finite &= isfinite(value);
            const float magnitude = fabsf(value);
            peak = magnitude > peak ? magnitude : peak;
            energy += (double)value * (double)value;
        }
    }
    const MRProcessTapStats after = mr_process_tap_stats(session);
    const double rms = frames == 0 ? 0 : sqrt(energy / ((double)frames * 2));
    const bool valid = finite && after.running && after.channels == 2 &&
        isfinite(after.sample_rate) && after.sample_rate > 0 &&
        (double)frames > after.sample_rate && after.callback_count > before.callback_count &&
        after.dropped_frames == before.dropped_frames && peak > 0.01F && rms > 0.01 &&
        (*reference_rms == 0 || fabs(rms - *reference_rms) <= *reference_rms * 0.05);
    printf("phase=%s running=%d callbacks=%" PRIu64 " read=%" PRIu64
        " peak=%.6f rms=%.6f channels=%u sample_rate=%.0f valid=%d\n",
        phase, after.running, after.callback_count, frames, peak, rms,
        after.channels, after.sample_rate, valid);
    if (valid && *reference_rms == 0) *reference_rms = rms;
    return valid;
}

int main(int argument_count, const char *arguments[]) {
    @autoreleasepool {
        if (argument_count != 2 && argument_count != 3) {
            fprintf(stderr, "usage: MRProcessTapSmoke <owned-fixture-bundle-id> [exact-owned-endpoint-uid]\n");
            return EXIT_FAILURE;
        }
        MRProcessTapConfig config = {
            .bundle_id = arguments[1],
            .mute_while_captured = false,
            .ring_capacity_frames = 16384,
            .target_device_uid = NULL,
        };
        MRProcessTapError error = {0};
        MRProcessTapSession *session = mr_process_tap_create(&config, &error);
        if (session == NULL) {
            fprintf(stderr, "create failed: code=%d os_status=%d\n", error.code, error.os_status);
            return EXIT_FAILURE;
        }
        error = mr_process_tap_start(session);
        if (error.code != MR_PROCESS_TAP_OK) {
            fprintf(stderr, "start failed: code=%d os_status=%d\n", error.code, error.os_status);
            mr_process_tap_destroy(session);
            return EXIT_FAILURE;
        }
        double reference_rms = 0;
        bool valid = MeasureCapture(session, "preserved", &reference_rms);
        if (valid && argument_count == 3) {
            config.mute_while_captured = true;
            config.target_device_uid = arguments[2];
            MRProcessTapSession *scoped = mr_process_tap_create(&config, &error);
            if (scoped == NULL) {
                fprintf(stderr, "scoped create failed: code=%d os_status=%d\n", error.code, error.os_status);
                valid = false;
            } else {
                // Stopped preparation must not affect canonical capture.
                valid = !mr_process_tap_stats(scoped).running &&
                    MeasureCapture(session, "scoped-prepared", &reference_rms);
                if (valid) {
                    error = mr_process_tap_start(scoped);
                    valid = error.code == MR_PROCESS_TAP_OK;
                    if (!valid) fprintf(stderr, "scoped start failed: code=%d os_status=%d\n", error.code, error.os_status);
                }
                if (valid) {
                    valid = MeasureCapture(session, "scoped-muted-pre-mute-pcm", &reference_rms);
                    const MRProcessTapStats scoped_stats = mr_process_tap_stats(scoped);
                    valid &= scoped_stats.running && scoped_stats.callback_count > 0;
                    if (!scoped_stats.running || scoped_stats.callback_count == 0)
                        fprintf(stderr, "scoped tap did not remain active with native callbacks\n");
                }
                if (mr_process_tap_stats(scoped).running) {
                    error = mr_process_tap_stop(scoped);
                    valid &= error.code == MR_PROCESS_TAP_OK;
                    if (error.code != MR_PROCESS_TAP_OK)
                        fprintf(stderr, "scoped stop failed: code=%d os_status=%d\n", error.code, error.os_status);
                    valid &= !mr_process_tap_stats(scoped).running;
                }
                mr_process_tap_destroy(scoped);
                if (valid) valid = MeasureCapture(session, "scoped-released", &reference_rms);
            }
        }
        mr_process_tap_destroy(session);
        return valid ? EXIT_SUCCESS : EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
