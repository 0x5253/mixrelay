// Installed acceptance for recording-device latency reporting.
// Adds one route SOURCE -> engine -> virtual endpoint to the selected graph, waits for the HAL
// to report a stable input latency on that endpoint, optionally measures the real round trip,
// then restores the exact original graph and checks that the report clears.
//   node scripts/lab/macos-latency-acceptance.mjs SOURCE ENDPOINT_ID EVIDENCE_DIR --verify-installed-latency [--roundtrip] [--settle-seconds=0..120]
// SOURCE is normally another MixRelay virtual endpoint ID (A): the probe plays into A, whose
// playback the engine routes to ENDPOINT_ID (D).
// SOURCE may instead be any device UID whose output returns on its input. The probe never
// plays into D itself: that would bypass the engine and gate the report to zero.
// With a foreign source, --roundtrip first measures its own pass-through before any graph
// write. Its measured-minus-reported latency is kept separately and added only when judging
// the complete path; a foreign device's hidden latency is not attributed to MixRelay.
// --roundtrip records audio, so the terminal needs microphone permission. It uses the probe's
// impulse mode: a route between two devices crosses clocks, so the engine resamples it and
// no bit-exact copy of a test signal can arrive. The round trip passes only when the probe
// measured (one delay aligned most pulses), the recording is complete, neither device's
// timestamps jumped, and the latency the devices report is within ROUNDTRIP tolerance of the
// measured mean:
//   max(32, ceil(rate / 1000))    the engine's path-agreement threshold (32 frames or 1 ms,
//                                 rounded up; runtime_latency.rs `threshold`)
//   + 2 frames                    the probe's pulse alignment window (MRRoundTripProbe.c,
//                                 mr_analyse_impulses: tolerance = 2 frames of host time)
// e.g. 47 frames at 44.1 kHz, 50 at 48 kHz, 98 at 96 kHz.
// The endpoint must be unused by every saved graph, so the test never adds a contributor to a
// live route. Writes name the selected graph (expectedGraphId) and its revision, so another
// client's edit or graph selection makes them fail instead of overwriting that graph.
// SIGINT, SIGTERM or SIGHUP stop the measurement; a confirmed test graph is then restored
// (an unconfirmed write is never guessed at), evidence is written and the signal is re-raised.
import { engineSocket, socketRequest } from '../local-socket.mjs';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { engineOperations } from './engine-operations.mjs';
import { mkdirSync, writeFileSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const endpointPattern = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const ids = { source: 'latency-acceptance-source', destination: 'latency-acceptance-destination' };
// The reporter publishes once a second; the HAL needs three stable windows.
const PUBLICATION_INTERVAL_MS = 1100;
export const INTERRUPTIONS = ['SIGINT', 'SIGTERM', 'SIGHUP'];
/** The engine's path-agreement threshold: max(32 destination frames, 1 ms rounded up to whole frames). */
export const reporterThresholdFrames = rate => Math.max(32, Math.ceil(rate / 1000));
/** MRRoundTripProbe aligns impulse arrivals within 2 frames of host time. */
export const PROBE_UNCERTAINTY_FRAMES = 2;
export const roundtripToleranceFrames = rate => reporterThresholdFrames(rate) + PROBE_UNCERTAINTY_FRAMES;

export function parseArguments(argv) {
  const [sourceUid, endpointId, evidence, approval, ...options] = argv;
  assert.equal(approval, '--verify-installed-latency', 'Pass --verify-installed-latency to change the installed graph');
  let settleSeconds = 0;
  let settleSpecified = false;
  for (const option of options) {
    if (option === '--roundtrip') continue;
    const match = option.match(/^--settle-seconds=(\d+)$/);
    assert.ok(match && !settleSpecified && Number(match[1]) <= 120, `Invalid option: ${option}`);
    settleSeconds = Number(match[1]);
    settleSpecified = true;
  }
  assert.ok(sourceUid, 'Pass a source endpoint ID or device UID');
  // A MixRelay source endpoint by ID; any other value is a looped-back device UID.
  const sourceEndpoint = endpointPattern.test(sourceUid) ? sourceUid : undefined;
  assert.ok(!sourceUid.startsWith('io.mixrelay.'), 'Name a MixRelay source by its endpoint ID');
  assert.ok(evidence && isAbsolute(evidence), 'Evidence directory must be an absolute path');
  assert.match(endpointId ?? '', endpointPattern);
  assert.notEqual(sourceEndpoint, endpointId, 'The source and the measured endpoint must differ');
  return { sourceUid, sourceEndpoint, endpointId, evidence, roundtrip: options.includes('--roundtrip'), settleSeconds };
}

/** Reads `--latency` probe text: the input scope's reported latency, safety and stream frames. */
export function parseReported(text) {
  const input = text.split('\n').find(line => line.startsWith('input '));
  const match = input?.match(/reports latency (\d+) \+ safety (\d+) \+ stream (\d+) frames/);
  assert.ok(match, `Cannot read the reported input latency:\n${text}`);
  return { latency: Number(match[1]), safety: Number(match[2]), stream: Number(match[3]) };
}

/**
 * Why an impulse round trip does not confirm the reported latency (empty when it does).
 * `stableInputLatency` is the HAL report the acceptance observed before the probe ran.
 */
export function roundtripProblems(report, stableInputLatency, sourceResidualFrames = 0) {
  const problems = [];
  if (report?.mode !== 'impulse') problems.push('the probe did not run in impulse mode');
  if (report?.measured !== true) problems.push('the probe did not measure: no single delay aligned most pulses');
  if (report?.complete !== true) problems.push('the recording is incomplete (overflow, dropped chunks or inconsistent buffers)');
  const jumps = report?.timestampDiscontinuities;
  if (jumps?.output !== 0 || jumps?.input !== 0) {
    problems.push(`device timestamps jumped (output ${jumps?.output}, input ${jumps?.input})`);
  }
  const rate = report?.input?.rate;
  if (!(rate > 0) || report?.output?.rate !== rate) problems.push('the devices do not report one common rate');
  if (report?.input?.latency !== stableInputLatency) {
    problems.push(`the probe read input latency ${report?.input?.latency}, not the stable report ${stableInputLatency}`);
  }
  const reported = report?.reportedLatencyFrames;
  const measured = report?.latencyFrames?.mean;
  if (!Number.isFinite(reported)) problems.push('the devices do not report their full latency');
  if (!Number.isFinite(measured)) problems.push('the probe reported no measured latency');
  if (!Number.isFinite(sourceResidualFrames)) problems.push('the source baseline is not finite');
  if (problems.length === 0) {
    const tolerance = roundtripToleranceFrames(rate);
    const difference = Math.abs(reported + sourceResidualFrames - measured);
    if (difference > tolerance) {
      const baseline = sourceResidualFrames === 0 ? '' : ` + source residual ${sourceResidualFrames}`;
      problems.push(`reported ${reported}${baseline} frames differs from the measured mean ${measured} by ${difference.toFixed(2)} frames (tolerance ${tolerance})`);
    }
  }
  return problems;
}

/**
 * Turns SIGINT/SIGTERM/SIGHUP into an abort of the measurement. Engine operations are never
 * aborted: a write in flight runs to its terminal result, so cleanup knows whether it applied.
 */
export function watchInterruptions(signals, warn) {
  const controller = new AbortController();
  const state = {
    received: undefined,
    signal: controller.signal,
    check() {
      if (state.received) throw interruptedError(state.received);
    },
    dispose() {
      for (const name of INTERRUPTIONS) signals.removeListener(name, listener);
    },
  };
  function listener(name) {
    if (state.received) {
      warn(`${name}: still restoring the original graph; each step is bounded. `
        + 'Forcing an exit now (kill -9) leaves the graph unchecked.');
      return;
    }
    state.received = name;
    warn(`${name}: stopping the measurement; the original graph is restored before exit.`);
    controller.abort(interruptedError(name));
  }
  for (const name of INTERRUPTIONS) signals.on(name, listener);
  return state;
}
const interruptedError = signal => Object.assign(new Error(`Interrupted by ${signal}`), { signal });

/**
 * Runs the acceptance. `io` supplies the engine transport (`request`), the probe (`reported`,
 * `roundtrip`), time (`now`, `delay`), signals and logging, so tests run it without an engine.
 */
export async function runAcceptance(options, io) {
  const interruption = watchInterruptions(io.signals, io.warn);
  try {
    return await acceptance(options, io, interruption);
  } finally {
    interruption.dispose();
    // With our listeners gone, the re-raised signal ends the process with its default action.
    if (interruption.received) io.signals.kill(io.signals.pid, interruption.received);
  }
}

async function acceptance({ sourceUid, sourceEndpoint, endpointId, evidence, roundtrip, settleSeconds = 0 }, io, interruption) {
  const { operation, api } = engineOperations(io, 'latency-acceptance');
  const uid = `io.mixrelay.audio.endpoint.${endpointId.replaceAll('-', '')}`;
  const nativeUid = id => `io.mixrelay.audio.endpoint.${id.replaceAll('-', '')}`;
  // Measurement waits stop on an interruption; cleanup waits always finish.
  async function until(what, check, attempts, intervalMs = 100, abort = undefined) {
    for (let attempt = 0; attempt < attempts; attempt++) {
      if (abort) interruption.check();
      const value = await check();
      if (value) return value;
      // An aborted wait reports the interruption, not a generic AbortError.
      await io.delay(intervalMs, abort).catch(error => { interruption.check(); throw error; });
    }
    assert.fail(`Timed out waiting for ${what}`);
  }
  // Applies a graph only while `expectedGraphId` is still selected; returns the terminal reply.
  const applyGraph = (document, expectedGraphId) =>
    operation('PUT', '/api/v1/graph', JSON.stringify({ ...document, expectedGraphId }));
  const ours = key => Object.values(ids).some(id => key === id || key.startsWith(`${id}::`));

  // Preconditions: nothing else routes to this endpoint or reports a latency for it.
  const catalog = await api('graphs');
  const graphId = catalog.activeId;
  for (const { id } of catalog.graphs) {
    interruption.check();
    const document = id === graphId ? catalog.document : await api(`graphs/${encodeURIComponent(id)}`);
    assert.ok(!document.nodes.some(node => node.type === 'virtual_endpoint' &&
      [endpointId, sourceEndpoint].includes(node.config.id)),
    `Graph ${id} already uses the source or measured endpoint; refusing to change a live route`);
  }
  const devices = (await api('status')).audioSession.virtualEndpointSettings.devices;
  const device = devices.find(candidate => candidate.spec.id === endpointId);
  assert.ok(device?.nativeDeviceIds.includes(uid), 'Endpoint is not published by the current engine');
  assert.ok(device.spec.capture, 'The endpoint needs a recording side');
  const sourceDevice = sourceEndpoint && devices.find(candidate => candidate.spec.id === sourceEndpoint);
  assert.ok(!sourceEndpoint || sourceDevice?.spec.render, 'The source endpoint needs a playback side');
  // The device the probe plays into.
  const playback = sourceEndpoint ? nativeUid(sourceEndpoint) : sourceUid;
  const original = await api('graph');
  assert.deepEqual(original, catalog.document, 'Graph changed during preparation');
  assert.ok(!original.nodes.some(node => Object.values(ids).includes(node.id)));
  interruption.check();
  const before = await io.reported(uid);
  assert.equal(before.latency, 0, 'The endpoint already reports a latency; refusing to judge a stale value');
  mkdirSync(evidence, { mode: 0o700 });
  const record = (name, value) =>
    writeFileSync(join(evidence, name), `${JSON.stringify(value, null, 2)}\n`, { flag: 'wx' });
  record('before.json', { graphId, graph: original, reported: before });
  interruption.check();
  let sourceBaseline;
  let sourceResidualFrames = 0;
  if (roundtrip && !sourceEndpoint) {
    const probe = await io.roundtrip(playback, playback, interruption.signal);
    sourceBaseline = JSON.parse(probe.stdout);
    record('source-baseline.json', sourceBaseline);
    assert.equal(probe.status, 0, 'The foreign source baseline probe failed');
    const { input, output, reportedLatencyFrames, latencyFrames, timestampDiscontinuities } = sourceBaseline;
    assert.ok(sourceBaseline.mode === 'impulse' && sourceBaseline.measured === true &&
      sourceBaseline.complete === true, 'The foreign source baseline is not a complete measurement');
    assert.ok(input?.uid === playback && output?.uid === playback && input.rate > 0 &&
      input.rate === output.rate, 'The source baseline must measure this exact duplex device');
    assert.ok(timestampDiscontinuities?.input === 0 && timestampDiscontinuities.output === 0,
      'The source baseline timestamps jumped');
    assert.ok(Number.isFinite(reportedLatencyFrames) && Number.isFinite(latencyFrames?.mean),
      'The source baseline has no measured/reported latency');
    sourceResidualFrames = latencyFrames.mean - reportedLatencyFrames;
    interruption.check();
  }

  const test = structuredClone(original);
  test.revision++;
  test.nodes.push(
    sourceEndpoint
      ? { id: ids.source, label: 'Latency source', type: 'virtual_endpoint',
        config: { ...sourceDevice.spec, reference: { render: true, capture: false } } }
      : { id: ids.source, label: 'Latency source', type: 'audio_device_input',
        config: { selector: { match: 'persistent_uid', uid: sourceUid }, outputChannels: device.spec.capture.channels } },
    { id: ids.destination, label: 'Latency destination', type: 'virtual_endpoint',
      config: { ...device.spec, reference: { render: false, capture: true } } },
  );
  test.edges.push({
    id: 'latency-acceptance-route',
    source: { nodeId: ids.source, portId: sourceEndpoint ? 'from_os' : 'audio' },
    destination: { nodeId: ids.destination, portId: 'to_os' },
    channelMapping: { mode: 'auto' },
  });
  // 'unknown' while a write may still commit; cleanup never guesses in that state. Only a
  // confirmed 'applied' write is ours to restore: a rejected one may have lost to another writer.
  let mutation = 'none';
  let applied;
  const result = { graphId, source: sourceUid, endpoint: uid, sourceBaseline, sourceResidualFrames, settleSeconds };
  const failures = [];
  const abort = interruption.signal;
  try {
    mutation = 'unknown';
    // Not abortable: an interruption waits for this write's terminal result.
    const applying = await applyGraph(test, graphId);
    mutation = applying.status >= 200 && applying.status < 300 ? 'applied' : 'rejected';
    assert.ok(applying.status >= 200 && applying.status < 300, `Test graph rejected: HTTP ${applying.status} ${applying.body}`);
    await until('the test graph', async () => (await api('graph/status')).activeRevision === test.revision, 100, 100, abort);
    // The engine normalizes what it stores (for example default node controls), so the
    // restore check compares against its applied document rather than the submitted one.
    applied = await api('graph');
    assert.equal(applied.revision, test.revision);
    assert.deepEqual(applied.edges, test.edges);
    // An explicit diagnostic wait separates initial FIFO/PI settling from steady
    // operation. Default is zero; no acceptance threshold changes with this wait.
    if (settleSeconds > 0) {
      await io.delay(settleSeconds * 1000, abort).catch(error => { interruption.check(); throw error; });
      interruption.check();
    }
    // Samples about one publication apart, so three equal values span three reporter windows.
    const readings = (result.readings = []);
    result.reported = await until('a stable reported input latency', async () => {
      readings.push(await io.reported(uid, abort));
      const last = readings.slice(-3);
      return last.length === 3 && last[0].latency > 0 && last.every(value => value.latency === last[0].latency)
        ? last[0] : undefined;
    }, 30, PUBLICATION_INTERVAL_MS, abort);
    if (roundtrip) {
      interruption.check();
      const probe = await io.roundtrip(playback, uid, abort);
      try {
        result.roundtrip = JSON.parse(probe.stdout);
      } catch {
        result.roundtrip = { unparsedOutput: probe.stdout };
      }
      interruption.check();
      assert.ok(!sourceBaseline || result.roundtrip.input?.rate === sourceBaseline.input.rate,
        'The source baseline and measured path must use the same rate');
      const problems = roundtripProblems(result.roundtrip, result.reported.latency, sourceResidualFrames);
      if (probe.status !== 0) problems.unshift(`the probe exited with status ${probe.status}`);
      const rate = result.roundtrip.input?.rate;
      result.roundtripToleranceFrames = rate > 0 ? roundtripToleranceFrames(rate) : null;
      assert.deepEqual(problems, [], `Round trip does not confirm the reported latency:\n${problems.join('\n')}`);
    }
    record('measured.json', { graph: applied, ...result });
  } catch (error) {
    failures.push(error);
  }
  // Cleanup runs whether or not the measurement succeeded or was interrupted; its own
  // failure is kept too. Its waits ignore the interruption so a confirmed write is undone.
  try {
    if (mutation === 'unknown') {
      assert.fail(`The test graph operation has no terminal result; check graph ${graphId} (original saved in ${evidence})`);
    }
    if (mutation === 'applied') await restore();
  } catch (error) {
    failures.push(error);
  }
  if (failures.length) {
    // Keep what was observed so a failed run can be diagnosed from its evidence.
    record('failed.json', {
      errors: failures.map(String), interrupted: interruption.received, mutation, graph: applied,
      status: await api('status').catch(String), ...result,
    });
    throw failures[0];
  }
  return {
    endpoint: uid,
    source: sourceUid,
    reportedInputLatencyFrames: result.reported.latency,
    roundtripLatencyFrames: result.roundtrip?.latencyFrames,
    roundtripLatencyMs: result.roundtrip?.latencyMs,
    sourceResidualFrames,
    settleSeconds,
    evidence,
  };

  async function restore() {
    // Our write was confirmed at test.revision and every write advances the revision.
    const current = await api('graph');
    // Every edit advances the revision, so an unchanged revision and document mean no one else edited.
    const untouched = current.revision === test.revision &&
      (applied ? JSON.stringify(current) === JSON.stringify(applied)
        : JSON.stringify(current.edges) === JSON.stringify(test.edges));
    assert.ok(untouched, `Concurrent edit: refusing automatic restore; original saved in ${evidence}`);
    const restored = { ...original, revision: test.revision + 1 };
    const restoring = await applyGraph(restored, graphId);
    assert.ok(restoring.status >= 200 && restoring.status < 300,
      `Restore rejected: HTTP ${restoring.status} ${restoring.body}; original saved in ${evidence}`);
    await until('the restored graph', async () => (await api('graph/status')).activeRevision === restored.revision, 100);
    assert.deepEqual(await api('graph'), restored);
    // An open editor saves positions for the temporary nodes; drop only those.
    const layout = await api(`graphs/${encodeURIComponent(graphId)}/layout`);
    const positions = Object.fromEntries(Object.entries(layout.layout.positions).filter(([key]) => !ours(key)));
    if (Object.keys(positions).length !== Object.keys(layout.layout.positions).length) {
      await api(`graphs/${encodeURIComponent(graphId)}/layout`, {
        expectedLayoutRevision: layout.layoutRevision, layout: { ...layout.layout, positions },
      });
    }
    // With the route gone the endpoint has no measurement, so the HAL must report unknown (0).
    record('restored.json', { graph: restored, reported: await until('the reported latency to clear', async () => {
      const value = await io.reported(uid);
      return value.latency === 0 ? value : undefined;
    }, 10, PUBLICATION_INTERVAL_MS) });
  }
}

function run(file, args, { timeout, signal }) {
  return new Promise(resolvePromise => {
    execFile(file, args, { encoding: 'utf8', timeout, signal, maxBuffer: 16 * 1024 * 1024 }, (error, stdout, stderr) => {
      resolvePromise({ status: error ? (Number.isInteger(error.code) ? error.code : 1) : 0, stdout, stderr, error });
    });
  });
}

/** The installed engine socket and the round-trip probe. */
export function installedIo() {
  const socket = engineSocket();
  const probe = fileURLToPath(new URL('../probe-macos-roundtrip.sh', import.meta.url));
  return {
    request: (path, options) => socketRequest(socket, path, options),
    async reported(uid, signal) {
      const reply = await run('zsh', [probe, '--latency', uid], { timeout: 10000, signal });
      if (signal?.aborted) throw signal.reason;
      assert.equal(reply.status, 0, `Cannot read the reported latency: ${reply.stderr || reply.error?.message}`);
      return parseReported(reply.stdout);
    },
    async roundtrip(playback, uid, signal) {
      const reply = await run('zsh', [probe, '--json', '--impulse', '--seconds', '5', playback, uid], { timeout: 60000, signal });
      if (signal?.aborted) throw signal.reason;
      return reply;
    },
    now: Date.now,
    delay: (ms, signal) => sleep(ms, undefined, { signal }),
    signals: process,
    warn: message => console.error(message),
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.platform, 'darwin');
  const summary = await runAcceptance(parseArguments(process.argv.slice(2)), installedIo());
  console.log(JSON.stringify(summary, null, 2));
}
