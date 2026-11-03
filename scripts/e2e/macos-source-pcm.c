#include "macos-source-pcm.h"
#include <math.h>
#include <stddef.h>
#include <string.h>

static uint32_t frames(const AudioBufferList *list) {
    if (list == NULL || list->mNumberBuffers != 1U) return 0U;
    const AudioBuffer *buffer = &list->mBuffers[0];
    if (buffer->mNumberChannels != 2U || buffer->mData == NULL ||
        buffer->mDataByteSize % (2U * sizeof(float)) != 0U) return 0U;
    return buffer->mDataByteSize / (2U * (uint32_t)sizeof(float));
}

static bool sample_time(const AudioTimeStamp *time, uint64_t *sample) {
    if (time == NULL || (time->mFlags & 3U) != 3U || !isfinite(time->mSampleTime) ||
        time->mSampleTime < 0.0 || time->mSampleTime >= 281474976710656.0 ||
        floor(time->mSampleTime) != time->mSampleTime) return false;
    *sample = (uint64_t)time->mSampleTime;
    return true;
}

static void chunk(MRSourcePcm *pcm, bool input, uint64_t first, uint64_t sample,
                  const AudioTimeStamp *time, uint32_t count, uint64_t entered, uint64_t exited) {
    uint32_t *used = input ? &pcm->input_count : &pcm->output_count;
    MRSourceChunk *rows = input ? pcm->input : pcm->output;
    if (*used == pcm->chunk_capacity) { pcm->dropped_chunks++; return; }
    rows[(*used)++] = (MRSourceChunk){.first = first, .sample = sample, .host = time->mHostTime,
        .entered = entered, .exited = exited, .frames = count, .flags = time->mFlags};
}

OSStatus mr_source_io(AudioDeviceID device, const AudioTimeStamp *now,
                     const AudioBufferList *input, const AudioTimeStamp *input_time,
                     AudioBufferList *output, const AudioTimeStamp *output_time, void *context) {
    (void)device; (void)now;
    MRSourcePcm *pcm = context;
    const uint64_t entered = pcm->clock();
    const uint32_t incoming = frames(input), outgoing = frames(output);
    uint64_t in_sample = 0U, out_sample = 0U;
    const bool in_valid = sample_time(input_time, &in_sample);
    const bool out_valid = sample_time(output_time, &out_sample);
    if (outgoing > 0U) memset(output->mBuffers[0].mData, 0, output->mBuffers[0].mDataByteSize);
    if (incoming != pcm->quantum || outgoing != pcm->quantum) pcm->invalid_buffers++;
    if (!in_valid || !out_valid) pcm->invalid_timestamps++;
    const uint64_t recorded_start = pcm->recorded_frames;
    bool captured = false, rendered = false;
    uint64_t source_start = 0U;
    // Capture owns the finite horizon; a late control-side stop cannot extend it.
    if (in_valid && incoming > 0U && recorded_start < pcm->source_frames) {
        if (recorded_start <= pcm->capacity && incoming <= pcm->capacity - recorded_start) {
            memcpy(pcm->recorded + recorded_start * 2U, input->mBuffers[0].mData,
                   (size_t)incoming * 2U * sizeof(float));
            pcm->recorded_frames += incoming;
            captured = true;
        } else pcm->overflow++;
    }
    if (out_valid && outgoing > 0U) {
        if (!pcm->has_origin) { pcm->origin = out_sample; pcm->has_origin = true; }
        if (out_sample >= pcm->origin) {
            source_start = out_sample - pcm->origin;
            if (source_start < pcm->source_frames) {
                const uint64_t remaining = pcm->source_frames - source_start;
                const uint64_t count = remaining < outgoing ? remaining : outgoing;
                memcpy(output->mBuffers[0].mData, pcm->source + source_start * 2U,
                       (size_t)count * 2U * sizeof(float));
            }
            rendered = source_start < pcm->source_frames;
        } else pcm->invalid_timestamps++;
    }
    const uint64_t exited = pcm->clock();
    if (captured) chunk(pcm, true, recorded_start, in_sample, input_time, incoming, entered, exited);
    if (rendered) chunk(pcm, false, source_start, out_sample, output_time, outgoing, entered, exited);
    return noErr;
}

static uint64_t fixed_clock(void) { return 123U; }

static int finite_capture_self_test(void) {
    float source[2048] = {0}, recorded[3072] = {0}, received[1024] = {0}, played[1024];
    MRSourceChunk ins[16], outs[16];
    MRSourcePcm pcm = {.source = source, .source_frames = 768U, .recorded = recorded,
        .capacity = 1536U, .input = ins, .output = outs, .chunk_capacity = 16U,
        .quantum = 512U, .clock = fixed_clock};
    AudioBufferList input = {.mNumberBuffers = 1U, .mBuffers = {{2U, sizeof(received), received}}};
    AudioBufferList output = {.mNumberBuffers = 1U, .mBuffers = {{2U, sizeof(played), played}}};
    AudioTimeStamp time = {.mSampleTime = 10000.0, .mHostTime = 1U, .mFlags = 3U};
    for (size_t i = 2048U; i < 3072U; i++) recorded[i] = 9.0F;
    // The whole last 512-frame callback is retained, then control-side stop can be late.
    for (unsigned phase = 0U; phase < 8U; phase++) {
        mr_source_io(0U, NULL, &input, &time, &output, &time, &pcm);
        time.mSampleTime += 512.0;
    }
    if (pcm.recorded_frames != 1024U || pcm.input_count != 2U || pcm.output_count != 2U ||
        pcm.overflow != 0U || pcm.dropped_chunks != 0U) return 1;
    for (size_t i = 2048U; i < 3072U; i++) if (recorded[i] != 9.0F) return 1;
    for (size_t i = 0U; i < 1024U; i++) if (played[i] != 0.0F) return 1;
    // An undersized capture still fails before the finite horizon is complete.
    pcm.recorded_frames = 0U; pcm.capacity = 256U;
    mr_source_io(0U, NULL, &input, &time, &output, &time, &pcm);
    return pcm.overflow == 1U && pcm.recorded_frames == 0U ? 0 : 1;
}

int mr_source_pcm_self_test(void) {
    float source[2048] = {0}, recorded[2048] = {0}, received[1024] = {0}, played[1024];
    for (size_t i = 1024U; i < 2048U; i++) source[i] = (float)(i % 7U + 1U) / 32.0F;
    MRSourceChunk ins[4], outs[4];
    MRSourcePcm pcm = {.source = source, .source_frames = 1024U, .recorded = recorded,
        .capacity = 1024U, .input = ins, .output = outs, .chunk_capacity = 4U,
        .quantum = 512U, .clock = fixed_clock};
    AudioBufferList input = {.mNumberBuffers = 1U, .mBuffers = {{2U, sizeof(received), received}}};
    AudioBufferList output = {.mNumberBuffers = 1U, .mBuffers = {{2U, sizeof(played), played}}};
    AudioTimeStamp time = {.mSampleTime = 10000.0, .mHostTime = 1U, .mFlags = 3U};
    for (unsigned phase = 0U; phase < 2U; phase++) {
        for (size_t i = 0U; i < 1024U; i++) played[i] = 9.0F;
        memcpy(received, source + phase * 1024U, sizeof(received));
        mr_source_io(0U, NULL, &input, &time, &output, &time, &pcm);
        if (memcmp(played, received, sizeof(received)) != 0) return 1;
        time.mSampleTime += 512.0;
    }
    if (memcmp(recorded, source, sizeof(source)) != 0 || pcm.recorded_frames != 1024U ||
        pcm.input_count != 2U || pcm.output_count != 2U || pcm.invalid_buffers != 0U ||
        pcm.invalid_timestamps != 0U || pcm.output[1].first != 512U) return 1;
    time.mFlags = 0U;
    mr_source_io(0U, NULL, &input, &time, &output, &time, &pcm);
    if (pcm.invalid_timestamps != 1U || pcm.recorded_frames != 1024U) return 1;
    for (size_t i = 0U; i < 1024U; i++) if (played[i] != 0.0F) return 1;
    return finite_capture_self_test();
}
