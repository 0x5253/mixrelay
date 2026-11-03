// Analyze PCM captured in the owned sink callback and the release engine's RTP DLL evidence.
// Usage: node analyze-clock-drift.mjs --warmup=20 LAB_DIRECTORY
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const warmupArgument = process.argv.find((argument) => argument.startsWith('--warmup='));
const warmupSeconds = warmupArgument ? Number(warmupArgument.split('=')[1]) : 20;
if (warmupSeconds !== 20) throw new Error('Current release-engine receipt requires 20-second warmup');

const sampleRate = 48_000;
const channels = 2;
const measuredSeconds = 37; // End three seconds before the 60-second sender stops.

export function checkSinkCapture(summary, rawBytes) {
  const capture = summary.sinkPcmCapture;
  if (!capture || capture.sampleRate !== sampleRate || capture.channels !== channels)
    throw new Error('Missing native stereo sink capture receipt');
  for (const key of ['capacityFrames', 'capturedFrames', 'observedFrames', 'callbacks',
    'invalidCallbacks', 'rawBytes']) {
    if (!Number.isSafeInteger(capture[key]) || capture[key] < 0)
      throw new Error(`Invalid sink capture counter: ${key}`);
  }
  if (!capture.callbacks || !capture.capturedFrames || capture.invalidCallbacks ||
      capture.capturedFrames > capture.capacityFrames ||
      capture.observedFrames < capture.capturedFrames ||
      capture.rawBytes !== capture.capturedFrames * Float32Array.BYTES_PER_ELEMENT * channels ||
      capture.rawBytes !== rawBytes ||
      capture.capHit !== (capture.observedFrames > capture.capturedFrames))
    throw new Error('Sink capture bytes or callback receipt are incomplete');
  return capture;
}

function measurePcm(directory) {
  const bytes = readFileSync(join(directory, 'sink-monitor.f32'));
  const capture = checkSinkCapture(
    JSON.parse(readFileSync(join(directory, 'device-fixture.json'), 'utf8')), bytes.length);
  if (bytes.length % (Float32Array.BYTES_PER_ELEMENT * channels))
    throw new Error('Sink-monitor raw F32 is not whole interleaved stereo frames');
  const availableFrames = bytes.length / (Float32Array.BYTES_PER_ELEMENT * channels);
  let firstActive = -1;
  for (let frame = 0; frame < availableFrames; frame += 1) {
    if (Math.abs(bytes.readFloatLE(frame * 8)) > 0.01) { firstActive = frame; break; }
  }
  const start = firstActive < 0 ? availableFrames : firstActive + warmupSeconds * sampleRate;
  const end = Math.min(availableFrames, start + measuredSeconds * sampleRate);
  return {
    file: join(directory, 'sink-monitor.f32'), rawBytes: bytes.length, availableFrames,
    captureMethod: 'owned_sink_callback_memory', capture,
    firstActiveFrame: firstActive, measuredStartFrame: start,
    expectedFrames: measuredSeconds * sampleRate, sampleRate, channels,
    ...measurePcmWindow(bytes, start, end),
  };
}

// The production analyzer and regression test measure the same interleaved PCM;
// neither callback counts nor RT scheduling alone can qualify the signal.
export function measurePcmWindow(bytes, start, end) {
  let invalid = 0, stereoMismatches = 0, jumps = 0, crossings = 0;
  let silentFrames = 0, gap = 0, maximumGap = 0, peak = 0, energy = 0, maximumStereoError = 0;
  let previous = 0, measuring = false;
  for (let frame = start; frame < end; frame += 1) {
    const left = bytes.readFloatLE(frame * 8), right = bytes.readFloatLE(frame * 8 + 4);
    if (!Number.isFinite(left) || !Number.isFinite(right)) { invalid += 1; continue; }
    const stereoError = Math.abs(left - right);
    if (stereoError > 0.0001) stereoMismatches += 1;
    maximumStereoError = Math.max(maximumStereoError, stereoError);
    peak = Math.max(peak, Math.abs(left));
    energy += left * left;
    if (measuring && Math.abs(left - previous) > 0.05) jumps += 1;
    if (measuring && previous <= 0 && left > 0) crossings += 1;
    // A sine has valid zero-crossing samples. Only an extended run is a dropout.
    if (Math.abs(left) < 0.00001) { silentFrames += 1; gap += 1; }
    else { maximumGap = Math.max(maximumGap, gap); gap = 0; }
    previous = left;
    measuring = true;
  }
  maximumGap = Math.max(maximumGap, gap);
  const frames = Math.max(0, end - start);
  return {
    frames,
    invalid, stereoMismatches, maximumStereoError, jumps, crossings, silentFrames, maximumGap,
    peak, rms: frames ? Math.sqrt(energy / frames) : 0,
    observedFrequency: frames ? crossings * sampleRate / frames : 0,
  };
}

export function sinkPcmFailures(pcm) {
  const failures = [];
  if (pcm.invalid || pcm.stereoMismatches || pcm.jumps || pcm.maximumGap > 2 || pcm.peak <= 0.2)
    failures.push('sink_pcm_discontinuity_or_corruption');
  const capture = pcm.capture;
  // These bounded observations use offsets in this exact raw callback capture.
  // Startup and sender shutdown events must not contaminate the 37-second window.
  if (!Number.isSafeInteger(capture.missingTimestamps) || capture.missingTimestamps !== 0 ||
      !Number.isSafeInteger(capture.cycleDiscontinuities) || capture.cycleDiscontinuities < 0 ||
      !Number.isSafeInteger(capture.droppedCycleObservations) || capture.droppedCycleObservations !== 0 ||
      !Array.isArray(capture.cycleEvents) || capture.cycleEvents.length !== capture.cycleDiscontinuities ||
      capture.cycleEvents.some(event => !Number.isSafeInteger(event?.frame) || event.frame < 0)) {
    failures.push('sink_native_timeline_incomplete');
  } else if (capture.cycleEvents.some(event =>
      event.frame >= pcm.measuredStartFrame && event.frame < pcm.measuredStartFrame + pcm.frames)) {
    failures.push('sink_native_cycle_discontinuity');
  }
  return failures;
}

function counterDelta(before, after, key) {
  const first = before[key], last = after[key];
  if (!Number.isSafeInteger(first) || !Number.isSafeInteger(last) || last < first)
    throw new Error(`Missing or decreasing engine ${key} counter`);
  return last - first;
}

function slope(samples, key) {
  if (samples.length < 2) return null;
  const origin = samples[0].time;
  const xs = samples.map((row) => row.time - origin);
  const meanX = xs.reduce((a, b) => a + b, 0) / xs.length;
  const meanY = samples.reduce((sum, row) => sum + row[key], 0) / samples.length;
  let covariance = 0;
  let variance = 0;
  for (let index = 0; index < samples.length; index++) {
    covariance += (xs[index] - meanX) * (samples[index][key] - meanY);
    variance += (xs[index] - meanX) ** 2;
  }
  return variance ? covariance / variance : null;
}

export function checkThreadPolicy(threadPolicy, owners) {
  for (const key of ['pipewirePid', 'enginePid']) {
    if (!Number.isSafeInteger(owners[key]) || owners[key] <= 0)
      throw new Error(`Missing owned process identity: ${key}`);
  }
  if (owners.pipewirePid === owners.enginePid) throw new Error('Server and engine must be distinct processes');
  const threads = threadPolicy.trim().split('\n').slice(1).map(line => {
    const [pid, tid, policy, priority, comm] = line.trim().split(/\s+/);
    return { pid: Number(pid), tid: Number(tid), policy, priority: Number(priority), comm };
  });
  const failures = [];
  for (const [key, label] of [['pipewirePid', 'server'], ['enginePid', 'engine']]) {
    const loops = threads.filter(thread => thread.pid === owners[key] && /^data-loop\.\d+$/.test(thread.comm));
    if (!loops.length || loops.some(thread => thread.policy !== 'RR' || thread.priority !== 20))
      failures.push(`owned_${label}_data_loop_not_rr20`);
  }
  return { processOwners: owners,
    rtThreads: threads.filter(thread => ['FF', 'RR'].includes(thread.policy) && thread.priority > 0),
    schedulerFailures: failures };
}

function readThreadPolicy(directory) {
  return checkThreadPolicy(readFileSync(join(directory, 'thread-policy.txt'), 'utf8'),
    JSON.parse(readFileSync(join(directory, 'process-owners.json'), 'utf8')));
}

function summarize(directory) {
  const sender = JSON.parse(readFileSync(join(directory, 'sender.json'), 'utf8'));
  const pcm = measurePcm(directory);
  const warmupStatus = JSON.parse(readFileSync(join(directory, 'engine-status-warmup.json'), 'utf8'));
  const endStatus = JSON.parse(readFileSync(join(directory, 'engine-status-end.json'), 'utf8'));
  const afterSenderStatus = JSON.parse(readFileSync(join(directory, 'engine-status-after-sender.json'), 'utf8'));
  const phaseTimes = Object.fromEntries(readFileSync(join(directory, 'phase-times.txt'), 'utf8')
    .trim().split('\n').map(line => {
      const [key, value] = line.split('=');
      return [key, Number(value)];
    }));
  const config = readFileSync(join(directory, 'pipewire.conf'), 'utf8');
  const scheduler = readThreadPolicy(directory);
  const log = readFileSync(join(directory, 'pipewire.log'), 'utf8').split('\n');
  const start = sender.firstSendNs / 1e9;
  const end = phaseTimes.end_before_ns / 1e9;
  const rows = [];
  const faults = [];
  for (const line of log) {
    const logTime = Number(line.match(/^\[(?:[EWIDT]|\*T\*)\]\[(\d{5}\.\d{6})\]\s+mod\.rtp-source\s+\|/)?.[1]);
    const time = logTime + Math.round((start - logTime) / 100_000) * 100_000;
    if (!Number.isFinite(time) || time < start + warmupSeconds || time > end - 0.02) continue;
    const dll = line.match(/avail:(\d+) target:(\d+) error:([\d.-]+) corr:([\d.-]+)/);
    if (dll) rows.push({ time, avail: Number(dll[1]), target: Number(dll[2]), correction: Number(dll[4]) });
    if (/underrun|overrun|unexpected seq|sync to timestamp/.test(line)) faults.push(line);
  }
  const tail = rows.filter((row) => row.time >= end - 30);
  const average = (key) => tail.reduce((sum, row) => sum + row[key], 0) / tail.length;
  const engineCounterDelta = {
    underruns: counterDelta(warmupStatus, endStatus, 'underruns'),
    overruns: counterDelta(warmupStatus, endStatus, 'overruns'),
    discontinuities: counterDelta(warmupStatus.audioSession ?? {}, endStatus.audioSession ?? {}, 'discontinuities'),
  };
  const afterSenderCounterDelta = {
    underruns: counterDelta(endStatus, afterSenderStatus, 'underruns'),
    overruns: counterDelta(endStatus, afterSenderStatus, 'overruns'),
    discontinuities: counterDelta(endStatus.audioSession ?? {}, afterSenderStatus.audioSession ?? {}, 'discontinuities'),
  };
  const expectedFrequency = 1000 * (1 + sender.observedPpm / 1e6);
  const occupancySlope = slope(tail, 'avail');
  const failures = [...scheduler.schedulerFailures];
  if (!/name = libpipewire-module-rt\b/.test(config)) failures.push('rt_module_not_requested');
  const warmupOffsetSeconds = (phaseTimes.warmup_before_ns - sender.firstSendNs) / 1e9;
  const endOffsetSeconds = (phaseTimes.end_after_ns - sender.firstSendNs) / 1e9;
  if (!Number.isFinite(warmupOffsetSeconds) || Math.abs(warmupOffsetSeconds - 20) > 1 ||
      !Number.isFinite(endOffsetSeconds) || Math.abs(endOffsetSeconds - 57) > 1)
    failures.push('engine_status_window_not_aligned_with_sender');
  if (Math.abs(sender.observedPpm - 2000) > 50) failures.push('sender_clock_not_near_requested_2000ppm');
  if (sender.maxLatenessUs > 250_000) failures.push('sender_lateness_exceeded_rtp_target_latency');
  if (pcm.frames !== pcm.expectedFrames) failures.push('insufficient_post_warmup_sink_pcm');
  failures.push(...sinkPcmFailures(pcm));
  if (Math.abs(pcm.observedFrequency - expectedFrequency) > 0.2) failures.push('output_tone_not_clock_corrected');
  if (Object.values(engineCounterDelta).some(value => value !== 0)) failures.push('engine_process_counter_increased');
  if (!tail.length || occupancySlope === null) failures.push('missing_rtp_dll_fifo_evidence');
  if (faults.length) failures.push('rtp_fault_logged_after_warmup');
  // 9.6 frames/s is 10% of the uncorrected +2000-ppm drift at 48 kHz.
  // This diagnostic bound does not itself qualify long-run clock stability.
  if (occupancySlope !== null && Math.abs(occupancySlope) > 9.6) failures.push('rtp_fifo_tail_not_settled');
  return {
    directory,
    compensationEnabled: !/resample.disable\s*=\s*true/.test(config),
    rtModuleRequested: /name = libpipewire-module-rt\b/.test(config),
    ...scheduler,
    sender,
    pcm,
    engineCounterDelta,
    afterSenderCounterDelta,
    phaseTimes: { ...phaseTimes, warmupOffsetSeconds, endOffsetSeconds },
    expectedClockCorrectedFrequency: expectedFrequency,
    frequencyErrorHz: pcm.observedFrequency - expectedFrequency,
    warmupSeconds,
    analyzedSeconds: end - start - warmupSeconds,
    dllSamples: rows.length,
    tailSamples: tail.length,
    tailOccupancyMin: tail.length ? Math.min(...tail.map((row) => row.avail)) : null,
    tailOccupancyMax: tail.length ? Math.max(...tail.map((row) => row.avail)) : null,
    tailOccupancyMean: tail.length ? average('avail') : null,
    tailOccupancySlopeFramesPerSecond: occupancySlope,
    tailCorrectionMean: tail.length ? average('correction') : null,
    tailDllCorrectionPpm: tail.length ? (average('correction') - 1) * 1e6 : null,
    tailRequestedResampleRatePpm: tail.length ? (1 / average('correction') - 1) * 1e6 : null,
    postWarmupFaultCount: faults.length,
    postWarmupFaults: faults,
    diagnosticFailures: failures,
    samplePerfectQualified: false,
    limitations: ['Tone frequency, adjacent jumps and silence runs do not prove every PCM sample is unique or correctly ordered.',
      'This 37-second measured window is not a long-duration release stability qualification.'],
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const rtOnly = process.argv.includes('--check-rt');
  const directories = process.argv.slice(2)
    .filter(argument => !argument.startsWith('--warmup=') && argument !== '--check-rt');
  if (directories.length !== 1) throw new Error('Expected one owned current-engine evidence directory');
  const result = rtOnly ? readThreadPolicy(directories[0]) : summarize(directories[0]);
  console.log(JSON.stringify(result, null, 2));
  if ((rtOnly ? result.schedulerFailures : result.diagnosticFailures).length) process.exitCode = 1;
}
