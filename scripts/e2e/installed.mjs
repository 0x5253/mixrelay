import { runQualificationMode, waitForObservation } from '../core/installed-qualification.mjs';
import assert from 'node:assert/strict';
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { engineOperations } from '../lab/engine-operations.mjs';
import { invoke } from '../runtime-control.mjs';
import { driverSocket, engineSocket, socketRequest } from '../local-socket.mjs';
import { artifactBytes, validateCandidate } from '../qualification.mjs';
import { digest, loadContract } from '../support-contract.mjs';
import { requiredScenarios } from '../core/support-targets.mjs';
import { validateBaseline } from './baseline.mjs';
import { collectLinux, collectLinuxUiProblems } from './collect-linux.mjs';
import { collectMacos } from './collect-macos.mjs';
import { observeEnvironment } from './environment.mjs';
import { validateEnvironment } from '../core/environment-policy.mjs';
import { startInstalledUi } from './installed-ui.mjs';
import { finishAfterCleanup } from '../core/qualification-cleanup.mjs';
import { analyseRecording, nativeSignal, runNativeAudioCases } from './native-signal.mjs';
import { driverOperations } from './driver-operations.mjs';
import { installAndVerify, launchInstalled, requireDisposableHost } from './package.mjs';
import { removeInstalled } from './removal.mjs';
import { wholeSystemCapture } from './system-capture.mjs';
import { controlRecorder } from './recorder-control.mjs';
import { startLinuxControlTrace } from './linux-control-trace.mjs';
import { recordSourceComparison } from './source-comparison-recording.mjs';
import { readProbeRecordingEvidence } from './macos-pcm-evidence.mjs';
import {
  isolateFreshState,
  prepareUpgradeDriver,
  preservedState,
  restoreUpgradeDriver,
  saveWorkspace,
  upgradeGraph,
  verifyAuthoredGraph,
  verifyPreservedState,
} from './upgrade-state.mjs';

async function eventually(read, predicate, description, {
  timeoutMs = process.env.MIXRELAY_E2E_VM_ACCELERATOR === 'tcg' ? 120000 : 30000,
  now = Date.now, wait = delay,
} = {}) {
  return waitForObservation(read, predicate, description, { timeoutMs, now, wait });
}

export function installedReadyStatus(api, { os, accelerator = process.env.MIXRELAY_E2E_VM_ACCELERATOR,
  now = Date.now, wait = delay } = {}) {
  // Cold HAL discovery and emulated guest startup both exceeded 30s in hosted
  // installs. These paths still need an actual ready observation within 120s.
  return eventually(() => api('status'), value => value.driverState === 'ready',
    'Installed engine/Driver not ready', { timeoutMs: os === 'macos' || accelerator === 'tcg' ? 120000 : 30000, now, wait });
}

export function signalGraph(original, graphId, spec) {
  const template = JSON.parse(
    readFileSync(
      new URL('../../crates/audio-graph/src/fixtures/qualification-stereo.json', import.meta.url),
    ),
  );
  return {
    ...original,
    revision: original.revision + 1,
    expectedGraphId: graphId,
    nodes: template.nodes.map((node) =>
      node.type === 'virtual_endpoint'
        ? { ...node, config: { ...spec, reference: node.config.reference } }
        : node,
    ),
    edges: template.edges,
  };
}

// The desktop has already collected the operation receipt. A pending graph
// publication still needs its audio-thread acknowledgement before recording.
export async function applyDesktopGraph(document, { invoke, active, api }) {
  const response = await invoke('control_apply_graph', { document });
  assert.ok([200, 202].includes(response.status), JSON.stringify(response));
  assert.equal(response.body.protocolVersion, 1);
  assert.equal(response.body.revision, document.revision);
  assert.ok(['active', 'pending'].includes(response.body.state), JSON.stringify(response));
  await active(document.revision);
  const observed = await api('graph');
  assert.equal(observed.revision, document.revision);
  assert.deepEqual(observed.nodes, document.nodes);
  assert.deepEqual(observed.edges, document.edges);
  return observed;
}

// Native preparation owns first-use consent. Qualification submits one change and
// collects its receipt; permission errors must fail instead of being replayed here.
export async function applyNativeGraph(document, { operation }) {
  const reply = await operation('PUT', '/api/v1/graph', JSON.stringify(document));
  assert.ok(reply.status >= 200 && reply.status < 300, `Native graph: HTTP ${reply.status} ${reply.body}`);
  assert.equal(reply.json.revision, document.revision);
  assert.ok(['active', 'pending'].includes(reply.json.state), JSON.stringify(reply));
}

// This is the same suite for every contract target. Platform code owns only installation
// and native signal generation. API evidence never substitutes for installed UI evidence.
export async function qualifyMode(options) {
  return runQualificationMode({ ...options, loaded: options.loaded ?? loadContract() }, qualificationPorts());
}

function qualificationPorts() {
  return {
    storage: { mkdir: mkdirSync, write: writeFileSync, read: readFileSync, exists: existsSync },
    context: { variables: process.env, timestamp: () => new Date().toISOString(), now: Date.now, wait: delay,
      log: console.log, observeEnvironment },
    artifacts: { validateCandidate, artifactBytes, validateBaseline },
    runtime: {
      engine: () => engineOperations({ request: (path, options) => socketRequest(engineSocket(), path, options), now: Date.now, delay }, 'qualification'),
      driver: (observe) => driverOperations({ request: (path, options) => socketRequest(driverSocket(), path, options), observe }),
      invoke, controlRecorder, applyNativeGraph, applyDesktopGraph, signalGraph, installedReadyStatus,
    },
    installation: { requireDisposableHost, installAndVerify, launchInstalled, removeInstalled },
    ui: { startInstalledUi, finishAfterCleanup, startLinuxControlTrace },
    audio: { nativeSignal, runNativeAudioCases, analyseRecording, readProbeRecordingEvidence, recordSourceComparison, wholeSystemCapture },
    upgrade: { isolateFreshState, prepareUpgradeDriver, upgradeGraph, saveWorkspace, verifyAuthoredGraph,
      preservedState, verifyPreservedState, restoreUpgradeDriver },
    diagnostics: { collectLinux, collectLinuxUiProblems, collectMacos },
  };
}

function stateEvidence(directory, mode, flatten) {
  return ['upgrade-state-before.json', 'upgrade-state-after.json', 'upgrade-session.mix']
    .filter((name) => existsSync(join(directory, name)))
    .flatMap((name) =>
      flatten(directory, mode, [
        { file: name, sha256: digest(readFileSync(join(directory, name))) },
      ]),
    );
}

export async function qualifyInstalled(options, runMode = qualifyMode) {
  const { output, targetId } = options;
  mkdirSync(output, { mode: 0o700 });
  const freshOutput = join(output, 'fresh');
  const fresh = await runMode({ ...options, output: freshOutput, baselineDirectory: undefined });
  const baselineDirectory = options.baselineDirectory || join(options.directory, 'baseline');
  const upgradeOutput = join(output, 'upgrade');
  const upgrade = await runMode({ ...options, output: upgradeOutput, baselineDirectory });
  const flatten = (directory, prefix, items) =>
    items.map((item) => {
      // Gate evidence uses flat filenames; encode the complete relative path to retain its identity.
      const file = `${prefix}-${encodeURIComponent(item.file)}`;
      copyFileSync(join(directory, item.file), join(output, file));
      return { file, sha256: item.sha256 };
    });
  const report = {
    ...fresh,
    endedAt: upgrade.endedAt,
    baseline: upgrade.baseline,
    cleanupError:
      [fresh.cleanupError, upgrade.cleanupError].filter(Boolean).join('\n') || undefined,
    status: fresh.status === 'passed' && upgrade.status === 'passed' ? 'passed' : 'failed',
    cleanup: fresh.cleanup === 'passed' && upgrade.cleanup === 'passed' ? 'passed' : 'failed',
    scenarios: fresh.scenarios.map((item) => ({
      ...item,
      evidence: flatten(freshOutput, 'fresh', item.evidence),
    })),
  };
  const upgradeEvidence = upgrade.scenarios.flatMap((item) =>
    flatten(upgradeOutput, 'upgrade', item.evidence),
  );
  for (const [mode, directory, result] of [
    ['fresh', freshOutput, fresh],
    ['upgrade', upgradeOutput, upgrade],
  ]) {
    const file = `${mode}-report.json`;
    writeFileSync(join(output, file), `${JSON.stringify(result, null, 2)}\n`, { flag: 'wx' });
    upgradeEvidence.push({ file, sha256: digest(readFileSync(join(output, file))) });
    upgradeEvidence.push(...stateEvidence(directory, mode, flatten));
  }
  report.scenarios.push({
    id: 'UPGRADE-01',
    scope: 'artifact',
    status: upgrade.status,
    error:
      upgrade.scenarios.find((item) => item.status === 'failed')?.error ??
      upgrade.error ??
      upgrade.cleanupError,
    evidence: upgradeEvidence,
  });
  writeFileSync(
    join(output, `${targetId}.qualification.json`),
    `${JSON.stringify(report, null, 2)}\n`,
    { flag: 'wx' },
  );
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const { values } = parseArgs({
      options: {
        target: { type: 'string' },
        candidate: { type: 'string' },
        baseline: { type: 'string' },
        directory: { type: 'string' },
        output: { type: 'string' },
        'disposable-environment': { type: 'boolean' },
        help: { type: 'boolean' },
      },
    });
    if (values.help)
      console.log(
        'pnpm test:e2e:installed --target ID --candidate FILE --directory FINAL_ARTIFACTS --output NEW_EVIDENCE_DIR --disposable-environment\nRuns fresh install/removal, then baseline-to-candidate upgrade/removal in the same disposable session. Baseline defaults to FINAL_ARTIFACTS/baseline; override with --baseline DIRECTORY. Never run on a personal workstation. Missing scenarios block release.',
      );
    else {
      for (const key of ['target', 'candidate', 'directory', 'output'])
        assert.ok(values[key], `--${key} is required`);
      const report = await qualifyInstalled({
        targetId: values.target,
        candidate: JSON.parse(readFileSync(values.candidate)),
        directory: resolve(values.directory),
        output: resolve(values.output),
        baselineDirectory: values.baseline && resolve(values.baseline),
        disposable: values['disposable-environment'],
      });
      console.log(`${report.targetId}: ${report.status}${report.error ? `: ${report.error}` : ''}`);
      process.exitCode = report.status === 'passed' ? 0 : 1;
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
