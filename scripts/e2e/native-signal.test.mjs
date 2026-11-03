import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { EventEmitter } from 'node:events';
import { analyseStereo, analyseRf64, nativeSignal, runNativeAudioCases } from './native-signal.mjs';
import { macosProbeCommand } from './macos-native-signal.mjs';
import { withMacosSchedulerTrace } from './macos-scheduler-trace.mjs';
import { stopSignalProcesses, waitForSignalBoundary } from './signal-process.mjs';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { analyseSystemMix } from './system-capture.mjs';
import { writeProbeEvidence } from './fixtures/probe-pcm-evidence.mjs';

function consentFixture(program, args, directory) {
  if (program !== 'open' || !args.includes('--authorize-input')) return false;
  writeFileSync(join(directory, 'native-probe-authorization.json'), JSON.stringify({
    inputAuthorization: { initial: 3, final: 3, requested: false },
  }));
  return true;
}

test('the LaunchServices measurement retains one original signal and unchanged audio settings', () => {
  const ordinary = macosProbeCommand('/probe.app', '/evidence', 'native-uid');
  assert.equal(ordinary.program, 'open');
  assert.equal(ordinary.args.filter(arg => arg === '--seconds').length, 1);
  assert.equal(ordinary.args[ordinary.args.indexOf('--seconds') + 1], '3');
  assert.equal(ordinary.args.some(arg => /buffer|warmup|retry/.test(arg)), false);
});

test('real consent precedes observer suspension and the single finite signal retains its failure', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-consent-measurement-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const events = [];
  let observing = true;
  await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    snapshot() {}, diagnostics() {},
    measurementBoundary: async measure => {
      events.push('pause acknowledged'); observing = false;
      try { return await measure(); }
      finally { observing = true; events.push('resume acknowledged'); }
    },
    execute(program, args) {
      if (program === 'zsh') return '/probe.app';
      if (program !== 'open') return;
      if (args.includes('--authorize-input')) {
        assert.equal(observing, true, 'Real privacy dialogs must remain observable');
        assert.equal(args.includes('--capture-file'), false);
        assert.equal(args.some(arg => arg.startsWith('io.mixrelay.audio.endpoint.')), false);
        events.push('consent granted');
        writeFileSync(join(directory, 'native-probe-authorization.json'), JSON.stringify({
          inputAuthorization: { initial: 0, final: 3, requested: true },
        }));
      } else {
        assert.equal(observing, false, 'AX scans must stop before finite PCM begins');
        events.push('one finite signal');
        writeProbeEvidence(directory, {
          inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true,
          complete: true, channelOrder: true, missingFrames: 6724,
          repeatedFrames: 0, corruptFrames: 2984, nonfiniteSamples: 0, receivedFrames: 137276,
        });
      }
    },
  }), error => {
    assert.match(error.message, /macOS\/CoreAudio\/hal_pcm/);
    assert.match(error.message, /missingFrames=6724/);
    assert.match(error.message, /corruptFrames=2984/);
    assert.equal(error.measurementFailure.boundary, 'hal_pcm');
    assert.deepEqual(error.measurementFailure.osRoundtrip, {
      missingFrames: 6724, repeatedFrames: 0, corruptFrames: 2984, nonfiniteSamples: 0 });
    assert.equal(error.measurementFailure.violations[0].boundary, 'hal_read');
    return true;
  });
  assert.equal(observing, true);
  assert.deepEqual(events, ['consent granted', 'pause acknowledged', 'one finite signal', 'resume acknowledged']);
});

test('denied consent never suspends permission handling or starts native audio', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-denied-consent-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const events = [];
  await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    snapshot() {}, diagnostics() {},
    measurementBoundary: () => assert.fail('Denied consent cannot enter measurement'),
    execute(program, args) {
      events.push(program);
      if (program === 'zsh') return '/probe.app';
      if (program !== 'open') return;
      assert.ok(args.includes('--authorize-input'));
      writeFileSync(join(directory, 'native-probe-authorization.json'), JSON.stringify({ inputAuthorization: { final: 2 } }));
    },
  }), /macOS probe authorization failed/);
  assert.deepEqual(events, ['zsh', 'open', 'pkill']);
});

test('OS roundtrip counters remain diagnostic when observed HAL publications and reads satisfy the product contract', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-native-boundary-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  await nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    snapshot() {}, diagnostics() { assert.fail('A healthy product boundary must not trigger failure collection'); },
    execute(program, args) {
      if (program === 'zsh') return '/probe.app';
      if (consentFixture(program, args, directory)) return;
      if (program === 'open') writeProbeEvidence(directory, {
        inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true,
        complete: true, channelOrder: true, missingFrames: 1264, repeatedFrames: 0,
        corruptFrames: 632, nonfiniteSamples: 0, receivedFrames: 142736,
      }, { corruptRead: false });
    },
  });
  const report = JSON.parse(readFileSync(join(directory, 'native-signal.json')));
  assert.equal(report.product.passed, true);
  assert.equal(report.hostRoundtrip.passed, false);
  assert.equal(report.hostRoundtrip.missingFrames, 1264);
  assert.equal(report.hostRoundtrip.corruptFrames, 632);
});

test('a stale loaded HAL cannot qualify a candidate even when its retained PCM is correct', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-native-version-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    snapshot() {}, diagnostics() {},
    execute(program, args) {
      if (program === 'zsh') return '/probe.app';
      if (consentFixture(program, args, directory)) return;
      if (program === 'open') writeProbeEvidence(directory, {
        inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true,
        complete: true, channelOrder: true, missingFrames: 0, repeatedFrames: 0,
        corruptFrames: 0, nonfiniteSamples: 0, receivedFrames: 144000,
      }, { corruptRead: false, traceVersion: 4 });
    },
  }), /Unsupported HAL trace version/);
  assert.equal(JSON.parse(readFileSync(join(directory, 'native-hal-io.json'))).version, 4);
});

test('observation and cleanup failures retain the original PCM boundary and every failure', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-combined-pcm-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    snapshot() {}, diagnostics() {},
    measurementBoundary: async measure => {
      try { return await measure(); }
      catch (error) { throw new AggregateError([error, new Error('observer resume failed')], `${error.message}; observer resume failed`); }
    },
    execute(program, args) {
      if (program === 'zsh') return '/probe.app';
      if (consentFixture(program, args, directory)) return;
      if (program === 'pkill') throw new Error('probe cleanup failed');
      if (program === 'open') writeProbeEvidence(directory, {
        inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true,
        complete: true, channelOrder: true, missingFrames: 84, repeatedFrames: 84,
        corruptFrames: 0, nonfiniteSamples: 0, receivedFrames: 143916,
      });
    },
  }), error => {
    assert.equal(error.errors.length, 2);
    assert.equal(error.errors[0].errors.length, 2);
    assert.match(error.message, /missingFrames=84.*observer resume failed.*probe cleanup failed/);
    assert.equal(error.measurementFailure.osRoundtrip.repeatedFrames, 84);
    assert.equal(error.measurementFailure.boundary, 'hal_pcm');
    return true;
  });
});

test('a scheduler trace cannot turn measured PCM loss into acceptance', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-scheduler-diagnostic-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const before = process.env.GITHUB_ACTIONS;
  process.env.GITHUB_ACTIONS = 'true';
  const measured = { inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true, complete: true,
    channelOrder: true, missingFrames: 128, repeatedFrames: 0, corruptFrames: 128, nonfiniteSamples: 0, receivedFrames: 143872 };
  const calls = [];
  try {
    await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
      schedulerTrace: true, snapshot() {}, diagnostics() {},
      trace: async (_, measure) => { calls.push('trace-start'); try { measure(); } finally { calls.push('trace-stop'); } },
      execute(program, args) {
        calls.push(program);
        if (consentFixture(program, args, directory)) return;
        if (program === 'zsh') return '/probe.app';
        if (program === 'open') writeProbeEvidence(directory, measured);
      },
    }), /missingFrames/);
    assert.deepEqual(calls, ['zsh', 'open', 'trace-start', 'open', 'trace-stop', 'pkill']);
  } finally {
    if (before === undefined) delete process.env.GITHUB_ACTIONS;
    else process.env.GITHUB_ACTIONS = before;
  }
});

test('a native property failure retains its procedure and OSStatus and is never replayed', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-property-failure-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const processFailed of [false, true]) {
    const original = new Error('probe exited with status 1');
    const calls = [];
    await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
      snapshot() {}, diagnostics() {},
      execute(program, args) {
        calls.push(program);
        if (consentFixture(program, args, directory)) return;
        if (program === 'zsh') return '/probe.app';
        if (program === 'open') {
          writeFileSync(join(directory, 'native-probe.stdout.json'), JSON.stringify({
            error: { stage: 'hal_trace_enable', osStatus: 2003332927, errno: 0 },
          }));
          if (processFailed) throw original;
        }
      },
    }), error => {
      assert.match(error.message, /stage=hal_trace_enable osStatus=2003332927 errno=0/);
      assert.equal(error.cause, processFailed ? original : undefined);
      return true;
    });
    assert.deepEqual(calls, ['zsh', 'open', 'open', 'pkill']);
  }
});

test('system-wide tracing is ready before the original measurement and stops after success or failure', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-system-trace-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const before = process.env.GITHUB_ACTIONS;
  process.env.GITHUB_ACTIONS = 'true';
  t.after(() => { if (before === undefined) delete process.env.GITHUB_ACTIONS; else process.env.GITHUB_ACTIONS = before; });
  for (const failed of [false, true]) {
    const phases = [];
    let notified = false;
    const child = new EventEmitter();
    child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
    child.kill = signal => { phases.push(signal); child.emit('close', 0, null); };
    const work = withMacosSchedulerTrace(directory, () => {
      phases.push('measure');
      if (failed) throw new Error('original measurement failed');
      return 42;
    }, {
      start(program, args) {
        assert.equal(program, 'xcrun'); assert.ok(args.includes('--all-processes'));
        assert.equal(args.includes('--launch'), false);
        assert.equal(args[args.indexOf('--notify-tracing-started') + 1], 'trace.fixture');
        queueMicrotask(() => { phases.push('ready'); notified = true; });
        return child;
      },
      notify: () => ({ name: 'trace.fixture', registered: () => true,
        started: () => notified, log: () => 'registered\nstarted\n',
        async stop() { phases.push('observer-stop'); } }),
      wait: async read => { await Promise.resolve(); assert.equal(read(), true); },
    });
    if (failed) await assert.rejects(work, /original measurement failed/);
    else assert.equal(await work, 42);
    assert.deepEqual(phases, ['ready', 'measure', 'SIGINT', 'observer-stop']);
    // The diagnostic output name is reused only after this invocation has finished.
    rmSync(join(directory, 'scheduler.log'));
    rmSync(join(directory, 'scheduler-notification.log'));
  }
});

test('trace stdout cannot start PCM before the Darwin notification, and both processes are reclaimed', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-trace-not-ready-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const before = process.env.GITHUB_ACTIONS;
  process.env.GITHUB_ACTIONS = 'true';
  t.after(() => { if (before === undefined) delete process.env.GITHUB_ACTIONS; else process.env.GITHUB_ACTIONS = before; });
  const phases = [];
  const child = new EventEmitter();
  child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
  child.kill = signal => { phases.push(signal); child.emit('close', 0, null); };
  await assert.rejects(withMacosSchedulerTrace(directory, () => phases.push('measure'), {
    start: () => {
      queueMicrotask(() => child.stdout.emit('data', Buffer.from('Ctrl-C to stop the recording\n')));
      return child;
    },
    notify: () => ({ name: 'trace.fixture', registered: () => true, started: () => false,
      log: () => 'registered\n', async stop() { phases.push('observer-stop'); } }),
    wait: async (read, phase) => {
      await Promise.resolve();
      if (phase === 'System Trace readiness') {
        assert.equal(read(), false);
        throw new Error('No Darwin start notification');
      }
      assert.equal(read(), true);
    },
  }), /No Darwin start notification/);
  assert.deepEqual(phases, ['SIGINT', 'observer-stop']);
  assert.match(readFileSync(join(directory, 'scheduler.log'), 'utf8'), /Ctrl-C/);
});

function recording() {
  const header = Buffer.alloc(80);
  header.write('RF64'); header.writeUInt32LE(0xffffffff, 4); header.write('WAVE', 8);
  header.write('ds64', 12); header.writeUInt32LE(28, 16);
  header.writeBigUInt64LE(BigInt(80 + 48000 * 8 - 8), 20);
  header.writeBigUInt64LE(48000n * 8n, 28); header.writeBigUInt64LE(48000n, 36);
  header.write('fmt ', 48); header.writeUInt32LE(16, 52); header.writeUInt16LE(3, 56);
  header.writeUInt16LE(2, 58); header.writeUInt32LE(48000, 60); header.writeUInt32LE(384000, 64);
  header.writeUInt16LE(8, 68); header.writeUInt16LE(32, 70); header.write('data', 72); header.writeUInt32LE(0xffffffff, 76);
  const pcm = Buffer.alloc(48000 * 8);
  for (let frame = 0; frame < 48000; frame++) {
    pcm.writeFloatLE(Math.sin(frame * Math.PI / 24) * 0.125, frame * 8);
    pcm.writeFloatLE(Math.sin(frame * Math.PI / 12) * 0.25, frame * 8 + 4);
  }
  return Buffer.concat([header, pcm]);
}
test('finalized RF64 evidence contains finite native stereo PCM and consistent 64-bit sizes', () => {
  assert.equal(analyseRf64(recording()).frames, 48000);
  const unfinalized = recording(); unfinalized.writeBigUInt64LE(0n, 28);
  assert.throws(() => analyseRf64(unfinalized), /Unfinalized/);
  const wrongRate = recording(); wrongRate.writeUInt32LE(44100, 60);
  assert.throws(() => analyseRf64(wrongRate));
  assert.throws(() => analyseStereo(Buffer.alloc(48000 * 8)), /native stereo/);
  const nonfinite = recording().subarray(80); nonfinite.writeFloatLE(NaN, 0);
  assert.throws(() => analyseStereo(nonfinite), /Non-finite/);
  assert.throws(() => analyseRf64(recording().subarray(0, -4)));
});

test('system capture requires both independent frequencies in the correct stereo lanes', () => {
  assert.ok(analyseSystemMix(recording()).tones.left1000 > 0.12);
  const missing = recording();
  for (let offset = 84, frame = 0; offset < missing.length; offset += 8, frame++) missing.writeFloatLE(Math.sin(2 * Math.PI * 3000 * frame / 48000) * 0.1, offset);
  assert.throws(() => analyseSystemMix(missing), /Missing independent/);
  const swapped = recording();
  for (let offset = 80; offset < swapped.length; offset += 8) {
    const left = swapped.readFloatLE(offset), right = swapped.readFloatLE(offset + 4);
    swapped.writeFloatLE(right, offset); swapped.writeFloatLE(left, offset + 4);
  }
  assert.throws(() => analyseSystemMix(swapped), /Missing independent/);
});
test('the recorder uses WAVE_FORMAT_EXTENSIBLE for stereo Float32, including its exact float GUID', () => {
  const original = recording();
  const format = Buffer.alloc(48);
  original.copy(format, 0, 48, 72);
  format.writeUInt32LE(40, 4); format.writeUInt16LE(0xfffe, 8);
  format.writeUInt16LE(22, 24); format.writeUInt16LE(32, 26); format.writeUInt32LE(3, 28);
  Buffer.from('0300000000001000800000aa00389b71', 'hex').copy(format, 32);
  const extended = Buffer.concat([original.subarray(0, 48), format, original.subarray(72)]);
  extended.writeBigUInt64LE(BigInt(extended.length - 8), 20);
  assert.equal(analyseRf64(extended).frames, 48000);
  extended.writeUInt32LE(1, 80);
  assert.throws(() => analyseRf64(extended), /subformat/);
});

test('native probe cleanup reaps both owners when the first ignores termination', async t => {
  const children = [true, false].map(ignore => spawn(process.execPath, ['-e', `
    ${ignore ? "process.on('SIGTERM',()=>{});" : ''}
    setInterval(()=>{},1000); process.stdout.write('ready\\n');
  `], { stdio: ['ignore', 'pipe', 'ignore'] }));
  t.after(() => { for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
  await Promise.all(children.map(child => once(child.stdout, 'data')));
  await assert.rejects(stopSignalProcesses(children, 100), /SIGKILL/);
  assert.equal(children[0].signalCode, 'SIGKILL');
  assert.ok(children.every(child => child.exitCode !== null || child.signalCode !== null), 'Both native probe owners must be reaped');
});

test('slow stream startup and drain are observed before analysing the same finite signal', async () => {
  let now = 0, frames = 0, played = false;
  await waitForSignalBoundary(() => played, 'playback drain', {
    timeoutMs: 1000, now: () => now,
    wait: async milliseconds => {
      now += milliseconds;
      if (now >= 400) { frames = 48000; played = true; }
    },
  });
  assert.equal(frames, 48000);
  assert.equal(now, 400, 'Do not return on a fixed startup or capture sleep');
});

test('a stalled stream fails within its bound and a process failure is never retried', async () => {
  let now = 0;
  await assert.rejects(waitForSignalBoundary(() => false, 'capture readiness', {
    timeoutMs: 100, now: () => now, wait: async milliseconds => { now += milliseconds; },
  }), /capture readiness.*deadline/);
  assert.equal(now, 100);
  let reads = 0;
  await assert.rejects(waitForSignalBoundary(() => { reads++; throw new Error('player exited 1'); }, 'playback drain'), /player exited 1/);
  assert.equal(reads, 1);
});

test('failed Mac PCM keeps the original verdict and collects host load only after the single signal ends', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-native-failure-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const measured = { inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true, complete: true,
    channelOrder: true, missingFrames: 128, repeatedFrames: 0, corruptFrames: 0, nonfiniteSamples: 0, receivedFrames: 48000 };
  const phases = [];
  await assert.rejects(nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
    execute(program, args) {
      phases.push(program);
      if (consentFixture(program, args, directory)) return;
      if (program === 'zsh') return '/probe.app';
      if (program === 'open') {
        assert.equal(args[args.indexOf('--capture-file') + 1], join(directory, 'native-signal.f32'));
        writeProbeEvidence(directory, measured);
      }
    },
    snapshot() { phases.push('snapshot'); },
    diagnostics(output) {
      assert.deepEqual(phases, ['zsh', 'open', 'open', 'pkill', 'snapshot']);
      const original = JSON.parse(readFileSync(join(output, 'native-probe.stdout.json')));
      assert.deepEqual(Object.fromEntries(Object.keys(measured).map(key => [key, original[key]])), measured);
      phases.push('diagnostics');
      throw new Error('diagnostic collection unavailable');
    },
  }), /missingFrames/);
  assert.deepEqual(phases, ['zsh', 'open', 'open', 'pkill', 'snapshot', 'diagnostics']);
});

test('installed audio stops recording before post-signal observers, retaining the original PCM failure', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mr-audio-order-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const measured = { inputAuthorization: { initial: 3, final: 3, requested: false }, activityProtection: true, complete: true,
    channelOrder: true, missingFrames: 636, repeatedFrames: 0, corruptFrames: 636, nonfiniteSamples: 0, receivedFrames: 143364 };
  let recording = true;
  const phases = [], verdicts = [];
  await runNativeAudioCases({ directory,
    async measure(observers) {
      try {
        await nativeSignal({ nativeDeviceIds: ['io.mixrelay.audio.endpoint.fixture'] }, directory, 'macos', {
          ...observers,
          execute(program, args) {
            if (consentFixture(program, args, directory)) return;
            if (program === 'zsh') return '/probe.app';
            if (program === 'open') {
              phases.push('finite signal');
              writeProbeEvidence(directory, measured);
            }
          },
        });
      } catch (error) { verdicts.push(error.message); }
    },
    async finalize() { phases.push('recorder stopped'); recording = false; },
    snapshot() { phases.push('process snapshot'); assert.equal(recording, false); },
    diagnostics() { phases.push('thread samples'); assert.equal(recording, false); },
  });
  assert.deepEqual(phases, ['finite signal', 'recorder stopped', 'process snapshot', 'thread samples']);
  assert.equal(verdicts.length, 1);
  assert.match(verdicts[0], /missingFrames/);
});

test('unexpected measurement failure still finalizes recording before collecting requested diagnostics', async () => {
  const phases = [];
  await assert.rejects(runNativeAudioCases({ directory: '/unused',
    async measure(observers) { observers.snapshot(); throw new Error('measurement failed'); },
    async finalize() { phases.push('recorder stopped'); },
    snapshot() { phases.push('snapshot'); },
    diagnostics() { assert.fail('Successful or unrequested diagnostics must not run'); },
  }), /measurement failed/);
  assert.deepEqual(phases, ['recorder stopped', 'snapshot']);
});
