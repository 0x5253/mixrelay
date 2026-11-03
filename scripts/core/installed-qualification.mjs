import { restoreQualificationGraph } from './qualification-cleanup.mjs';
import { validateEnvironment } from './environment-policy.mjs';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { requiredScenarios } from './support-targets.mjs';

const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

export async function waitForObservation(read, predicate, description, { timeoutMs, now, wait }) {
  const deadline = now() + timeoutMs;
  let last;
  while (now() < deadline) {
    try { last = await read(); if (predicate(last)) return last; }
    catch (error) { last = error.message; }
    await wait(200);
  }
  throw new Error(`${description}: ${JSON.stringify(last)}`);
}

export async function runQualificationMode({
  targetId,
  candidate,
  directory,
  output,
  disposable,
  baselineDirectory,
  loaded,
}, ports) {
  const { mkdir: mkdirSync, write: writeFileSync, read: readFileSync, exists: existsSync } = ports.storage;
  const { variables, timestamp, now, wait, log, observeEnvironment } = ports.context;
  const { validateCandidate, artifactBytes, validateBaseline } = ports.artifacts;
  const { invoke, controlRecorder, applyNativeGraph, applyDesktopGraph, signalGraph, installedReadyStatus } = ports.runtime;
  const { requireDisposableHost, installAndVerify, launchInstalled, removeInstalled } = ports.installation;
  const { startInstalledUi, finishAfterCleanup, startLinuxControlTrace } = ports.ui;
  const { nativeSignal, runNativeAudioCases, analyseRecording, readProbeRecordingEvidence, recordSourceComparison, wholeSystemCapture } = ports.audio;
  const { isolateFreshState, prepareUpgradeDriver, upgradeGraph, saveWorkspace, verifyAuthoredGraph,
    preservedState, verifyPreservedState, restoreUpgradeDriver } = ports.upgrade;
  const { collectLinux, collectLinuxUiProblems, collectMacos } = ports.diagnostics;
  const eventually = (read, predicate, description) => waitForObservation(read, predicate, description, {
    timeoutMs: variables.MIXRELAY_E2E_VM_ACCELERATOR === 'tcg' ? 120000 : 30000, now, wait,
  });
  const { contract, sha256 } = loaded;
  const target = contract.qualificationTargets.find((item) => item.id === targetId);
  assert.ok(target, 'Unknown qualification target');
  validateCandidate(candidate, directory, loaded);
  const artifact = candidate.artifacts.find((item) => item.id === target.artifact);
  const definition = contract.artifacts.find((item) => item.id === target.artifact);
  mkdirSync(output, { mode: 0o700 });
  const report = {
    schemaVersion: 1,
    targetId,
    sourceRevision: candidate.sourceRevision,
    contractSha256: sha256,
    candidateSha256: digest(JSON.stringify(candidate)),
    artifactSha256: artifact.sha256,
    provenance: {
      repository: variables.GITHUB_REPOSITORY,
      runId: variables.GITHUB_RUN_ID,
      runAttempt: variables.GITHUB_RUN_ATTEMPT,
      suiteRevision: variables.GITHUB_SHA,
    },
    startedAt: timestamp(),
    status: 'failed',
    cleanup: 'passed',
    scenarios: [],
  };
  function evidence(name, value) {
    const bytes = Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
    writeFileSync(join(output, name), bytes, { flag: 'wx' });
    return { file: name, sha256: digest(bytes) };
  }
  async function scenario(id, work) {
    const scope = requiredScenarios(contract, target).find((item) => item.id === id).scope;
    log(`${targetId}: ${id}: started`);
    try {
      const files = await work();
      report.scenarios.push({ id, scope, status: 'passed', evidence: files });
      log(`${targetId}: ${id}: passed`);
      return true;
    } catch (error) {
      report.scenarios.push({
        id,
        scope,
        status: 'failed',
        error: error.message,
        evidence: [evidence(`${id}.failure.json`, { message: error.message, stack: error.stack,
          measurementFailure: error.measurementFailure })],
      });
      log(`${targetId}: ${id}: failed: ${error.message.slice(0, 500)}`);
      if (id === 'GRAPH-01' && target.os === 'linux') {
        try { collectLinuxUiProblems(output); }
        catch (diagnosticError) {
          evidence('installed-ui-problems-error.json', { message: diagnosticError.message });
        }
      }
      return false;
    }
  }
  const { api, operation } = ports.runtime.engine();
  const driverAdmission = [];
  const driver = ports.runtime.driver(value => driverAdmission.push(value));
  let stop,
    ui,
    original,
    applied,
    graphId,
    recorderPath,
    upgradeState,
    packageInstalled = false,
    unconfirmedMutation = false;
  const recorderAdmission = [];
  const controlTake = request => controlRecorder(recorderPath, request, {
    api, operation, graphId, observe: value => recorderAdmission.push(value),
  });
  const active = (revision) =>
    eventually(
      () => api('graph/status'),
      (status) => status.activeRevision === revision,
      'Graph did not activate',
    );
  async function apply(document) {
    unconfirmedMutation = true;
    await applyNativeGraph(document, { operation });
    await active(document.revision);
    const observed = await api('graph');
    unconfirmedMutation = false;
    return observed;
  }
  try {
    report.environment = observeEnvironment();
    validateEnvironment(report.environment, target, contract);
    requireDisposableHost(target.os, disposable);
    if (baselineDirectory) {
      assert.ok(
        existsSync(join(baselineDirectory, 'baseline.json')),
        `Missing baseline selection: ${baselineDirectory}`,
      );
      const baseline = JSON.parse(readFileSync(join(baselineDirectory, 'baseline.json')));
      const baselineArtifact = validateBaseline(
        baseline,
        candidate,
        baselineDirectory,
        target.artifact,
      );
      report.baseline = { ...baseline, artifactSha256: baselineArtifact.sha256 };
      report.isolatedFreshState = isolateFreshState(target.os, output, { disposable });
      const baselineOutput = join(output, 'baseline');
      mkdirSync(baselineOutput);
      installAndVerify(baselineArtifact, definition, baselineDirectory, baselineOutput, target.os, {
        onInstalled: () => { packageInstalled = true; },
      });
      stop = await launchInstalled(target.os, baselineOutput);
      await eventually(
        driver,
        (value) => value.state === 'ready' && value.installedVersion === baseline.version,
        'Baseline Driver not ready',
      );
      const catalog = await eventually(
        () => api('graphs'),
        (value) => !!value.document,
        'Baseline graph unavailable',
      );
      graphId = catalog.activeId;
      const baselineDriver = await prepareUpgradeDriver(driver);
      unconfirmedMutation = true;
      const authored = upgradeGraph(catalog.document, graphId);
      await api('graph', authored);
      const path = join(output, 'upgrade-session.mix');
      const saved = await saveWorkspace(api, operation, active, 'save', path);
      verifyAuthoredGraph(authored, saved);
      unconfirmedMutation = false;
      upgradeState = preservedState(saved, await driver(), path, baselineDriver.defaults);
      evidence('upgrade-state-before.json', upgradeState);
      await stop();
      stop = undefined;
    }
    const installed = await scenario('PKG-01', async () => {
      installAndVerify(artifact, definition, directory, output, target.os, {
        onInstalled: () => { packageInstalled = true; },
      });
      report.environmentBeforeInstall = report.environment;
      report.environment = observeEnvironment();
      validateEnvironment(report.environment, target, contract);
      assert.deepEqual(artifactBytes(directory, artifact), {
        sha256: artifact.sha256,
        size: artifact.size,
      });
      return [
        {
          file: 'installed-binaries.json',
          sha256: digest(readFileSync(join(output, 'installed-binaries.json'))),
        },
      ];
    });
    if (!installed) throw new Error('Installation or installed artifact identity failed');
    stop = await launchInstalled(target.os, output);
    const ready = await scenario('APP-01', async () => {
      const status = await installedReadyStatus(api, { os: target.os });
      const snapshot = await driver();
      assert.equal(snapshot.state, 'ready');
      assert.equal(snapshot.installedVersion, candidate.version);
      return [evidence('app-ready.json', { status, driver: snapshot })];
    });
    if (!ready) {
      const status = await api('status'),
        snapshot = await driver();
      const proof = evidence('readiness-disagreement.json', {
        status,
        driver: snapshot,
        note: 'APP-01 remains failed. Independent native scenarios run only with a live installed Driver and engine.',
      });
      report.scenarios.find((item) => item.id === 'APP-01').evidence.push(proof);
      assert.equal(snapshot.state, 'ready', 'Installed Driver is unavailable');
      assert.equal(snapshot.installedVersion, candidate.version);
      assert.equal(status.protocolVersion, 1);
      assert.equal(status.enginePower?.enabled, true);
      assert.equal(
        status.audioSession?.capabilities?.backend,
        target.os === 'macos' ? 'core_audio' : 'pipe_wire',
      );
    }
    if (upgradeState) {
      const current = await api('graphs');
      verifyPreservedState(upgradeState, current, await driver());
      const reopened = await saveWorkspace(api, operation, active, 'open', upgradeState.path);
      verifyPreservedState(upgradeState, reopened, await driver());
      evidence(
        'upgrade-state-after.json',
        preservedState(reopened, await driver(), upgradeState.path, upgradeState.defaultDevices),
      );
      await restoreUpgradeDriver(driver, upgradeState.defaultDevices);
      graphId = reopened.activeId;
      const empty = await apply({
        ...reopened.document,
        nodes: [],
        edges: [],
        expectedGraphId: graphId,
        revision: reopened.document.revision + 1,
      });
      await stop();
      stop = undefined;
      const uiSession = join(output, 'ui-session');
      mkdirSync(uiSession);
      stop = await launchInstalled(target.os, uiSession);
      await installedReadyStatus(api, { os: target.os });
      const restarted = await api('graphs');
      assert.equal(restarted.activeId, graphId);
      assert.deepEqual(restarted.document.nodes, empty.nodes);
      assert.deepEqual(restarted.document.edges, empty.edges);
      evidence('upgrade-ui-start.json', { beforeRestart: empty, afterRestart: restarted.document });
    }
    const catalog = await api('graphs');
    original = catalog.document;
    graphId = catalog.activeId;
    assert.deepEqual(original.nodes, [], 'Reset QA workspace: existing graph is preserved');
    assert.deepEqual(original.edges, []);
    const trace = target.os === 'linux' && variables.MIXRELAY_E2E_CONTROL_TRACE === 'true'
      ? await startLinuxControlTrace(output) : undefined;
    try {
      ui = await startInstalledUi(target.os, output);
      await scenario('GRAPH-01', async () => {
        await ui.ready();
        const current = await eventually(
          () => api('graphs'),
          (value) => value.document.nodes.length === 2 && value.document.edges.length > 0,
          'Installed UI did not commit its connection',
        );
        const source = current.document.nodes.find((node) => node.type === 'compressor');
        const recorder = current.document.nodes.find((node) => node.type === 'recorder');
        assert.ok(source && recorder, 'Installed UI created different nodes');
        assert.ok(
          current.document.edges.every(
            (edge) => edge.source.nodeId === source.id && edge.destination.nodeId === recorder.id,
          ),
          'Installed UI connected different nodes',
        );
        await active(current.document.revision);
        const proof = evidence('installed-ui-graph.json', current);
        const files = ['installed-ui.png', 'installed-ui-tree.txt'].map((file) => ({
          file,
          sha256: digest(readFileSync(join(output, file))),
        }));
        return [proof, ...files];
      });
    } finally { await trace?.stop(); }
    const afterUi = await api('graphs');
    assert.equal(afterUi.activeId, graphId);
    assert.ok(
      afterUi.document.nodes.every((node) => ['compressor', 'recorder'].includes(node.type)),
      'Unexpected external graph change',
    );
    applied = await apply({
      ...original,
      expectedGraphId: graphId,
      revision: afterUi.document.revision + 1,
    });
    const snapshot = await driver();
    assert.equal(
      snapshot.settings.devices.length,
      1,
      'Reset QA device settings: expected the fresh default device',
    );
    const device = snapshot.settings.devices[0];
    const spec = device.spec;
    assert.equal(spec.displayName, 'MixRelay IO');
    assert.equal(spec.render?.channels, 2);
    assert.equal(spec.capture?.channels, 2);
    assert.equal(device.observedSampleRate, 48000);
    assert.ok(device.nativeDeviceIds.length > 0, 'Native device was not published');
    try {
      applied = await apply(signalGraph(applied, graphId, spec));
    } catch (error) {
      report.scenarios.push({
        id: 'AUDIO-01',
        scope: 'native-audio',
        status: 'failed',
        error: error.message,
        evidence: [
          evidence('audio-graph-setup.failure.json', {
            stage: 'native graph activation',
            message: error.message,
          }),
        ],
      });
      throw error;
    }
    await scenario('GRAPH-02', async () => {
      const path = join(output, 'qualification.mix');
      async function workspace(action) {
        const current = await api('graphs');
        const reply = await operation(
          'POST',
          '/api/v1/workspace',
          JSON.stringify({
            expectedRevision: current.document.revision,
            expectedCatalogRevision: current.catalogRevision,
            operation: { action, path },
          }),
        );
        assert.equal(reply.status, 200, reply.body);
        await active(reply.json.document.revision);
        return reply.json;
      }
      await workspace('save');
      const before = applied;
      applied = await apply({
        ...applied,
        expectedGraphId: graphId,
        revision: applied.revision + 1,
        edges: [],
      });
      const opened = await workspace('open');
      applied = opened.document;
      graphId = opened.activeId;
      assert.deepEqual(applied.nodes, before.nodes);
      assert.deepEqual(applied.edges, before.edges);
      return [
        evidence('graph-reopen.json', { before, after: applied }),
        { file: 'qualification.mix', sha256: digest(readFileSync(path)) },
      ];
    });
    recorderPath = `graphs/${encodeURIComponent(graphId)}/recorders/qa-recorder`;
    const recording = join(output, 'recording.wav');
    const start = await api(recorderPath);
    assert.equal(start.state, 'stopped');
    await controlTake({
      expectedRevision: applied.revision,
      expectedRecording: { state: start.state, path: start.path },
      request: { action: 'record', path: recording, sampleRate: 48000, encoding: 'float32' },
    });
    await runNativeAudioCases({
      directory: output,
      measure: (diagnostics) => scenario('AUDIO-01', async () => {
        await nativeSignal(device, output, target.os, { ...diagnostics,
          measurementBoundary: ui.measurementBoundary,
          schedulerTrace: Boolean(!baselineDirectory && variables.MIXRELAY_E2E_SCHEDULER_TRACE === 'true') });
        return [
          {
            file: 'native-signal.json',
            sha256: digest(readFileSync(join(output, 'native-signal.json'))),
          },
          ...(target.os === 'macos' ? ['native-probe-authorization.json', 'native-permission-observation.json']
            .map(file => ({ file, sha256: digest(readFileSync(join(output, file))) })) : []),
        ];
      }),
      finalize: () => scenario('REC-02', async () => {
        const take = await api(recorderPath);
        const stopped = await controlTake({
          expectedRevision: applied.revision,
          expectedRecording: { state: take.state, path: take.path },
          request: { action: 'stop' },
        });
        assert.equal(stopped.state, 'stopped');
        assert.equal(stopped.error, null);
        const analysis = analyseRecording(recording);
        const product = target.os === 'macos' ? readProbeRecordingEvidence(output) : null;
        if (product) {
          evidence('recording-pcm-contract.json', product);
          assert.ok(product.passed, 'Recorded PCM does not preserve the observed native input; see recording-pcm-contract.json');
        }
        return [
          evidence('recording-analysis.json', { stopped, analysis }),
          { file: 'recording.wav', sha256: digest(readFileSync(recording)) },
          ...(product ? [{ file: 'recording-pcm-contract.json', sha256: digest(readFileSync(join(output, 'recording-pcm-contract.json'))) }] : []),
        ];
      }),
    });
    if (target.os === 'macos') {
      await scenario('AUDIO-12', async () => {
        await recordSourceComparison({ api, controlTake, recorderPath, revision: applied.revision,
          device, directory: join(output, 'source-comparison'), measurementBoundary: ui.measurementBoundary });
        return ['comparison.json', 'recording-contract.json', 'recording.wav',
          'silent-audible/analysis.json', 'audible-silent/analysis.json']
          .map(file => ({ file: join('source-comparison', file), sha256: digest(readFileSync(join(output, 'source-comparison', file))) }));
      });
    }
    if (target.os === 'macos')
      await scenario('AUDIO-11', async () => {
        await wholeSystemCapture({
          api,
          operation,
          firstUse: !baselineDirectory,
          graphId,
          document: applied,
          device,
          output,
          apply: async (document) => {
            unconfirmedMutation = true;
            applied = await applyDesktopGraph(document, { invoke, active, api });
            unconfirmedMutation = false;
            return applied;
          },
        });
        return ['whole-system-analysis.json', 'whole-system.wav', 'whole-system-consent.json',
          'whole-system-ready.json', 'whole-system-sources.json'].map((file) => ({
          file,
          sha256: digest(readFileSync(join(output, file))),
        }));
      });
  } catch (error) {
    report.error = error.message;
    report.errorEvidence = evidence('prerequisite.failure.json', {
      message: error.message, stack: error.stack,
      cause: error.cause && { message: error.cause.message, stack: error.cause.stack, code: error.cause.code },
    });
  } finally {
    const cleanupFailed = (error) => {
      report.cleanup = 'failed';
      report.cleanupError = [report.cleanupError, error.message].filter(Boolean).join('\n');
    };
    try {
      await restoreQualificationGraph({ unconfirmedMutation, applied, original, graphId, recorderPath },
        { api, controlTake, apply });
    } catch (error) {
      cleanupFailed(error);
    }
    if (
      target.os === 'linux' &&
      (report.error || report.scenarios.some((item) => item.status === 'failed'))
    ) {
      try {
        collectLinux(output);
      } catch (error) {
        cleanupFailed(error);
      }
    }
    if (
      target.os === 'macos' &&
      (report.error || report.scenarios.some((item) => item.status === 'failed'))
    ) {
      try {
        collectMacos(output, 'live-');
      } catch (error) {
        cleanupFailed(error);
      }
    }
    const failedUi = (error) => {
      const result = report.scenarios.find((item) => item.id === 'GRAPH-01');
      if (result) {
        result.status = 'failed';
        result.error = error.message;
        result.evidence.push(
          evidence('installed-ui-completion.failure.json', { message: error.message }),
        );
      }
    };
    const cleanup = async () => {
      try {
        await stop?.();
      } catch (error) {
        cleanupFailed(error);
        if (target.os === 'macos') {
          try {
            collectMacos(output, 'shutdown-');
          } catch (diagnosticError) {
            cleanupFailed(diagnosticError);
          }
        }
      }
      if (packageInstalled)
        await scenario('PKG-02', async () => {
          const retained = ['qualification.mix', 'recording.wav', 'upgrade-session.mix']
            .filter((file) => existsSync(join(output, file)))
            .map((file) => ({ file, sha256: digest(readFileSync(join(output, file))) }));
          removeInstalled(target.os, definition.format, output);
          for (const item of retained)
            assert.equal(
              digest(readFileSync(join(output, item.file))),
              item.sha256,
              `Removal changed ${item.file}`,
            );
          return [{ file: 'removal.log', sha256: digest(readFileSync(join(output, 'removal.log'))) }];
        });
    };
    try {
      const result = await finishAfterCleanup(ui, cleanup);
      if (result && !result.passed) failedUi(new Error(result.error));
    } catch (error) {
      failedUi(error);
      cleanupFailed(error);
    }
    for (const expected of requiredScenarios(contract, target).filter(
      (item) => item.id !== 'UPGRADE-01',
    ))
      if (!report.scenarios.some((item) => item.id === expected.id)) {
        report.scenarios.push({
          id: expected.id,
          scope: expected.scope,
          status: 'blocked',
          evidence: [],
          error: 'A prerequisite failed',
        });
      }
    report.status =
      !report.error &&
      report.cleanup === 'passed' &&
      report.scenarios.every((item) => item.status === 'passed')
        ? 'passed'
        : 'failed';
    report.endedAt = timestamp();
    writeFileSync(join(output, 'driver-admission.json'), `${JSON.stringify(driverAdmission, null, 2)}\n`, { flag: 'wx' });
    writeFileSync(join(output, 'recorder-admission.json'), `${JSON.stringify(recorderAdmission, null, 2)}\n`, { flag: 'wx' });
    writeFileSync(join(output, `${targetId}.mode.json`), `${JSON.stringify(report, null, 2)}\n`, {
      flag: 'wx',
    });
  }
  return report;
}
