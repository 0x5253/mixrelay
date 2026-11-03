import { MINIMUM_MACOS } from '../macos-minimum.mjs';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { analyseRf64 } from './native-signal.mjs';
import { stopSignalProcesses } from './signal-process.mjs';
import { controlRecorder } from './recorder-control.mjs';

export function analyseSystemMix(bytes) {
  const analysis = analyseRf64(bytes);
  let offset = 12;
  while (bytes.toString('ascii', offset, offset + 4) !== 'data') {
    const size = bytes.readUInt32LE(offset + 4);
    offset += 8 + size + (size % 2);
  }
  const pcm = bytes.subarray(offset + 8);
  const frames = Math.min(48000, Math.floor(pcm.length / 8));
  assert.ok(frames >= 48000, 'Whole-system capture contains less than a second of PCM');
  const first = Math.floor((pcm.length / 8 - frames) / 2);
  const amplitude = (channel, frequency) => {
    let real = 0, imaginary = 0;
    for (let frame = 0; frame < frames; frame++) {
      const sample = pcm.readFloatLE((first + frame) * 8 + channel * 4);
      const angle = 2 * Math.PI * frequency * frame / 48000;
      real += sample * Math.cos(angle); imaginary += sample * Math.sin(angle);
    }
    return 2 * Math.hypot(real, imaginary) / frames;
  };
  const tones = { left1000: amplitude(0, 1000), right2000: amplitude(1, 2000),
    left2000: amplitude(0, 2000), right1000: amplitude(1, 1000) };
  assert.ok(tones.left1000 > 0.005 && tones.right2000 > 0.005, `Missing independent system source: ${JSON.stringify(tones)}`);
  assert.ok(tones.left2000 < tones.left1000 * 0.05 && tones.right1000 < tones.right2000 * 0.05,
    `Whole-system capture changed channel order: ${JSON.stringify(tones)}`);
  return { ...analysis, tones };
}

function compileTone(output, index) {
  const name = `MixRelayTone${index}`, bundle = join(output, `${name}.app`);
  mkdirSync(join(bundle, 'Contents/MacOS'), { recursive: true });
  const plist = join(bundle, 'Contents/Info.plist');
  copyFileSync('native/macos/application-capture/tests/MRVirtualToneSource-Info.plist', plist);
  for (const [key, value] of [['CFBundleExecutable', name], ['CFBundleIdentifier', `io.mixrelay.qualification.tone${index}`]]) {
    execFileSync('plutil', ['-replace', key, '-string', value, plist]);
  }
  const executable = join(bundle, 'Contents/MacOS', name);
  execFileSync('clang', ['-O2', '-fobjc-arc', '-fblocks', `-mmacosx-version-min=${MINIMUM_MACOS}`,
    `-DMR_TONE_FREQUENCY=${index === 0 ? 1000 : 2000}.0`, `-DMR_TONE_CHANNEL=${index}`,
    'native/macos/application-capture/tests/MRVirtualToneSource.m', '-framework', 'AVFAudio',
    '-framework', 'AudioUnit', '-framework', 'CoreAudio', '-framework', 'Foundation', '-o', executable]);
  execFileSync('codesign', ['--force', '--sign', '-', bundle]);
  return resolve(executable);
}

// macOS bundle identities of MixRelay's own processes, which System Audio excludes.
// scripts/macos-identity.test.mjs checks them against the app and engine bundles.
export const SELF_BUNDLE_IDS = Object.freeze(['io.mixrelay.desktop', 'io.mixrelay.service']);

export function systemCaptureGraph(document, graphId) {
  return { ...document, revision: document.revision + 1, expectedGraphId: graphId,
    nodes: [{ id: 'qa-system', type: 'application_capture', label: null,
      control: { enabled: true, muted: false }, config: {
      selector: { match: 'all_except', bundle_ids: [...SELF_BUNDLE_IDS] },
      stream: { channels: 2, channelLayout: 'stereo' }, sourcePlayback: 'preserve', gainDb: 0,
    } }, { id: 'qa-system-recorder', type: 'recorder', label: null,
      control: { enabled: true, muted: false }, config: { channels: 2 } }],
    edges: [{ id: 'qa-system-record', source: { nodeId: 'qa-system', portId: 'audio' },
      destination: { nodeId: 'qa-system-recorder', portId: 'audio' }, channelMapping: { mode: 'auto' } }] };
}

export async function waitForSystemSignal(api, revision, { now = Date.now, wait = delay, observe = () => {} } = {}) {
  const deadline = now() + 30000;
  let consecutive = 0;
  while (now() < deadline) {
    const snapshot = await api('meters');
    const source = snapshot.nodes.find(node => node.nodeId === 'qa-system' && node.portId === 'audio');
    observe({ at: now(), revision: snapshot.revision, peaks: source?.peaks ?? null });
    const live = snapshot.revision === revision && source?.peaks.length === 2
      && source.peaks.every(peak => Number.isFinite(peak) && peak > 0.04 && peak < 0.06);
    consecutive = live ? consecutive + 1 : 0;
    if (consecutive >= 3) return;
    await wait(100);
  }
  assert.fail('Both whole-system source channels did not become live after consent');
}

export async function waitForSystemConsent(readGrants, { now = Date.now, wait = delay } = {}) {
  const deadline = now() + 30000;
  while (now() < deadline) {
    const grant = (await readGrants()).find(item => /^(Allow|OK)(\/|$)/.test(item.action)
      && item.subject?.some(text => /mixrelay/i.test(text) && /system audio/i.test(text))
      && Number.isFinite(Date.parse(item.dismissedAt)));
    if (grant) return grant;
    await wait(100);
  }
  assert.fail('First-use system-audio approval did not reach confirmed dialog dismissal');
}

async function toneStatus(child) {
  const response = once(child.stdout, 'data', { signal: AbortSignal.timeout(5000) });
  child.stdin.write('status\n');
  const [bytes] = await response;
  return JSON.parse(bytes.toString());
}

export async function wholeSystemCapture({ api, operation, apply, graphId, document, device, output, firstUse }) {
  assert.equal(typeof firstUse, 'boolean');
  const uid = device.nativeDeviceIds.find(id => id.startsWith('io.mixrelay.audio.endpoint.'));
  assert.ok(uid);
  const children = [];
  let applied, recordingStarted = false;
  const path = join(resolve(output), 'whole-system.wav');
  const recorder = `graphs/${encodeURIComponent(graphId)}/recorders/qa-system-recorder`;
  const recorderAdmission = [];
  const controlTake = request => controlRecorder(recorder, request, {
    api, operation, graphId, observe: value => recorderAdmission.push(value),
  });
  try {
    applied = await apply(systemCaptureGraph(document, graphId));
    const grantsFile = join(output, 'audio-permission-actions.json');
    const consent = firstUse ? await waitForSystemConsent(() => existsSync(grantsFile)
      ? JSON.parse(readFileSync(grantsFile, 'utf8')) : []) : null;
    writeFileSync(join(output, 'whole-system-consent.json'), JSON.stringify({ firstUse, consent }, null, 2) + '\n', { flag: 'wx' });
    for (let index = 0; index < 2; index++) {
      const executable = compileTone(output, index);
      const stderr = openSync(join(output, `tone-${index}.stderr.log`), 'wx');
      let child;
      try { child = spawn(executable, ['--mixrelay-virtual-only', uid], { stdio: ['pipe', 'pipe', stderr] }); }
      finally { closeSync(stderr); }
      children.push(child);
      child.stdin.on('error', () => {});
      const [ready] = await once(child.stdout, 'data', { signal: AbortSignal.timeout(15000) });
      assert.equal(ready.toString().trim(), `ready ${uid}`);
    }
    const readiness = [];
    const sources = { before: await Promise.all(children.map(toneStatus)) };
    try {
      await waitForSystemSignal(api, applied.revision, { observe: value => readiness.push(value) });
    } finally {
      writeFileSync(join(output, 'whole-system-ready.json'), JSON.stringify(readiness, null, 2) + '\n', { flag: 'wx' });
      sources.after = await Promise.all(children.map(toneStatus));
      writeFileSync(join(output, 'whole-system-sources.json'), JSON.stringify(sources, null, 2) + '\n', { flag: 'wx' });
    }
    let take = await api(recorder);
    await controlTake({ expectedRevision: applied.revision, expectedRecording: { state: take.state, path: take.path },
      request: { action: 'record', path, sampleRate: 48000, encoding: 'float32' } });
    recordingStarted = true;
    await delay(3000);
    take = await api(recorder);
    const stopped = await controlTake({ expectedRevision: applied.revision,
      expectedRecording: { state: take.state, path: take.path }, request: { action: 'stop' } });
    recordingStarted = false;
    assert.equal(stopped.state, 'stopped'); assert.equal(stopped.error, null);
    const analysis = analyseSystemMix(readFileSync(path));
    writeFileSync(join(output, 'whole-system-analysis.json'), JSON.stringify({ access, analysis,
      sources: ['io.mixrelay.qualification.tone0', 'io.mixrelay.qualification.tone1'] }, null, 2) + '\n', { flag: 'wx' });
  } finally {
    try {
      if (applied) {
        if (recordingStarted) {
          const take = await api(recorder);
          await controlTake({ expectedRevision: applied.revision,
            expectedRecording: { state: take.state, path: take.path }, request: { action: 'stop' } });
        }
        const current = await api('graph');
        assert.deepEqual(current, applied, 'External graph change during whole-system acceptance');
        await apply({ ...document, expectedGraphId: graphId, revision: current.revision + 1 });
      }
    } finally {
      try {
        writeFileSync(join(output, 'whole-system-recorder-admission.json'), JSON.stringify(recorderAdmission, null, 2) + '\n', { flag: 'wx' });
      } finally { await stopSignalProcesses(children); }
    }
  }
}
