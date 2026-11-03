import assert from 'node:assert/strict';
import test from 'node:test';
import { applyDesktopGraph, applyNativeGraph, signalGraph, installedReadyStatus } from './installed.mjs';
import { digest } from '../support-contract.mjs';

test('cold HAL and emulated startup wait for actual Driver readiness without accepting a missing observation', async () => {
  for (const [os, accelerator, cold] of [['linux', 'kvm', false], ['linux', 'tcg', true], ['macos', undefined, true]]) {
    let elapsed = 0;
    const ready = installedReadyStatus(async path => {
      assert.equal(path, 'status');
      return { driverState: elapsed >= 35000 ? 'ready' : 'installed_not_loaded' };
    }, { os, accelerator, now: () => elapsed, wait: async () => { elapsed += 5000; } });
    if (cold) assert.equal((await ready).driverState, 'ready');
    else await assert.rejects(ready, /Installed engine\/Driver not ready/);
  }
  for (const options of [{ accelerator: 'tcg' }, { os: 'macos' }]) {
    let elapsed = 0;
    await assert.rejects(installedReadyStatus(async () => ({ driverState: 'installed_not_loaded' }),
      { ...options, now: () => elapsed, wait: async () => { elapsed += 5000; } }), /not ready/);
    assert.equal(elapsed, 120000);
  }
});

const pendingConsent = { status: 503, json: { code: 'audio_permission_pending',
  parameters: { capability: 'microphone', configurationOutcome: 'not_applied' } } };

test('native input collects the original consent-waiting operation without another submission', async () => {
  const document = { revision: 4, nodes: [], edges: [] };
  let submissions = 0;
  let approve;
  const consent = new Promise(resolve => { approve = resolve; });
  const result = applyNativeGraph(document, {
    operation: async (method, path, body) => {
      submissions++;
      assert.equal(method, 'PUT'); assert.equal(path, '/api/v1/graph');
      assert.deepEqual(JSON.parse(body), document);
      await consent;
      return { status: 202, json: { revision: 4, state: 'pending' } };
    },
  });
  assert.equal(submissions, 1);
  approve();
  await result;
  assert.equal(submissions, 1);
});

test('native input fails pending, denied and uncertain operations without replaying them', async () => {
  const document = { revision: 4, nodes: [], edges: [] };
  for (const reply of [pendingConsent,
    { status: 503, json: { code: 'audio_permission_denied' } },
    { status: 503, json: { code: 'control_request_failed' } },
  ]) {
    let submits = 0;
    await assert.rejects(applyNativeGraph(document, {
      operation: async () => { submits++; return reply; },
    }));
    assert.equal(submits, 1);
  }
});

test('desktop pending graph publications are acknowledged before readback without resubmission', async () => {
  const document = { revision: 10, nodes: [{ id: 'system' }], edges: [] };
  for (const status of [200, 202]) {
    const calls = [];
    const result = await applyDesktopGraph(document, {
      invoke: async (command, args) => {
        calls.push(command);
        assert.deepEqual(args, { document });
        return { status, body: { protocolVersion: 1, revision: 10, state: status === 202 ? 'pending' : 'active' } };
      },
      active: async revision => { calls.push('active'); assert.equal(revision, 10); },
      api: async path => { calls.push(path); return document; },
    });
    assert.deepEqual(result, document);
    assert.deepEqual(calls, ['control_apply_graph', 'active', 'graph']);
  }
});

test('desktop acceptance rejects failed, foreign and unacknowledged graph publications', async () => {
  const document = { revision: 10, nodes: [], edges: [] };
  for (const response of [
    { status: 409, body: {} },
    { status: 202, body: { protocolVersion: 1, revision: 9, state: 'pending' } },
    { status: 202, body: { protocolVersion: 1, revision: 10, state: 'unknown' } },
  ]) {
    await assert.rejects(applyDesktopGraph(document, {
      invoke: async () => response,
      active: async () => assert.fail('must not await a rejected response'),
    }));
  }
  await assert.rejects(applyDesktopGraph(document, {
    invoke: async () => ({ status: 202, body: { protocolVersion: 1, revision: 10, state: 'pending' } }),
    active: async () => { throw new Error('publication never activated'); },
    api: async () => assert.fail('must not read an unacknowledged graph'),
  }), /never activated/);
  await assert.rejects(applyDesktopGraph(document, {
    invoke: async () => ({ status: 200, body: { protocolVersion: 1, revision: 10, state: 'active' } }),
    active: async () => {},
    api: async () => ({ ...document, nodes: [{ id: 'foreign' }] }),
  }));
});

test('unity-level acceptance records the Driver mix without adding a feedback return', () => {
  const original = { schemaVersion: 1, revision: 10, nodes: [], edges: [] };
  const spec = { id: 'default-device', render: { channels: 2 }, capture: { channels: 2 } };
  const graph = signalGraph(original, 'selected-graph', spec);
  const endpoint = graph.nodes.find((node) => node.type === 'virtual_endpoint');
  const recorder = graph.nodes.find((node) => node.type === 'recorder');
  assert.equal(endpoint.config.id, spec.id);
  assert.deepEqual(endpoint.config.reference, { render: true, capture: false });
  assert.deepEqual(
    new Set(
      graph.edges
        .filter((edge) => edge.source.nodeId === endpoint.id && edge.source.portId === 'from_os')
        .map((edge) => `${edge.destination.nodeId}/${edge.destination.portId}`),
    ),
    new Set([`${recorder.id}/audio`]),
  );
  assert.equal(graph.expectedGraphId, 'selected-graph');
  assert.equal(graph.revision, 11);
  assert.deepEqual(original.nodes, []);
});

test('fresh and upgrade share the target, retain independent full suites and produce one gate report', async (t) => {
  const { writeFileSync, mkdirSync } = await import('node:fs');
  const { dirname, join } = await import('node:path');
  const { qualifyInstalled } = await import('./installed.mjs');
  const { qualificationFixture } = await import('./qualification.fixture.mjs');
  const { readReports, evaluateQualification } = await import('../qualification.mjs');
  const f = qualificationFixture(t),
    calls = [];
  const target = f.reports[2];
  const output = join(f.directory, 'dual-mode');
  const runMode = async (options) => {
    calls.push(options);
    mkdirSync(options.output);
    writeFileSync(join(options.output, 'unit-evidence.txt'), 'Synthetic scenario evidence\n');
    const nested = ['source-comparison/comparison.json', 'source-comparison/silent-audible/analysis.json'];
    for (const file of nested) {
      mkdirSync(dirname(join(options.output, file)), { recursive: true });
      writeFileSync(join(options.output, file), '{"passed":true}\n');
    }
    return {
      ...structuredClone(target),
      scenarios: target.scenarios.filter((item) => item.id !== 'UPGRADE-01').map(item => item.id === 'AUDIO-12'
        ? { ...item, evidence: nested.map(file => ({ file, sha256: digest(Buffer.from('{"passed":true}\n')) })) }
        : item),
    };
  };
  const report = await qualifyInstalled(
    { targetId: target.targetId, candidate: f.candidate, directory: f.directory, output },
    runMode,
  );
  assert.equal(calls.length, 2);
  assert.equal(calls[0].baselineDirectory, undefined);
  assert.equal(calls[1].baselineDirectory, join(f.directory, 'baseline'));
  assert.equal(calls[0].targetId, calls[1].targetId);
  assert.equal(report.status, 'passed');
  const reports = readReports(output);
  assert.equal(reports.length, 1);
  assert.ok(
    report.scenarios
      .find((item) => item.id === 'UPGRADE-01')
      .evidence.some((item) => item.file === 'upgrade-report.json'),
  );
  const all = f.reports.map((item) => (item.targetId === target.targetId ? reports[0] : item));
  assert.equal(evaluateQualification(f.candidate, all).passed, true);
  assert.deepEqual(report.scenarios.find(item => item.id === 'AUDIO-12').evidence.map(item => item.file),
    ['fresh-source-comparison%2Fcomparison.json', 'fresh-source-comparison%2Fsilent-audible%2Fanalysis.json']);
  writeFileSync(join(output, 'upgrade-source-comparison%2Fsilent-audible%2Fanalysis.json'), 'changed upgrade evidence');
  assert.throws(() => readReports(output), /changed raw evidence/);
});

test('upgrade failures cannot be hidden by passing fresh results', async (t) => {
  const { writeFileSync, mkdirSync } = await import('node:fs');
  const { join } = await import('node:path');
  const { qualifyInstalled } = await import('./installed.mjs');
  const { qualificationFixture } = await import('./qualification.fixture.mjs');
  const { evaluateQualification } = await import('../qualification.mjs');
  const f = qualificationFixture(t),
    target = f.reports[2];
  for (const failure of [
    'Previous app entitlement policy rejected',
    'Missing baseline artifact: macos-universal-pkg',
    'Native audio failed after upgrade',
    'Removal failed after upgrade',
    'Runtime shutdown failed after upgrade',
  ]) {
    const output = join(f.directory, `case-${failure.slice(0, 10)}`);
    const report = await qualifyInstalled(
      { targetId: target.targetId, candidate: f.candidate, directory: f.directory, output },
      async (options) => {
        mkdirSync(options.output);
        writeFileSync(join(options.output, 'unit-evidence.txt'), 'Synthetic scenario evidence\n');
        const result = {
          ...structuredClone(target),
          scenarios: target.scenarios.filter((item) => item.id !== 'UPGRADE-01'),
        };
        if (options.baselineDirectory) {
          result.status = 'failed';
          result.error = failure;
          if (failure.startsWith('Missing baseline'))
            result.scenarios = result.scenarios.map((item) => ({
              ...item,
              status: 'blocked',
              error: 'A prerequisite failed',
            }));
          if (failure.startsWith('Runtime shutdown')) {
            result.error = undefined;
            result.cleanup = 'failed';
            result.cleanupError = failure;
          }
        }
        return result;
      },
    );
    assert.equal(report.status, 'failed');
    const upgrade = report.scenarios.find((item) => item.id === 'UPGRADE-01');
    assert.equal(upgrade.status, 'failed');
    assert.equal(upgrade.error, failure);
    const all = f.reports.map((item) => (item.targetId === target.targetId ? report : item));
    assert.equal(evaluateQualification(f.candidate, all).passed, false);
  }
});

test('a passing upgrade cannot hide a failed fresh-install scenario', async (t) => {
  const { writeFileSync, mkdirSync } = await import('node:fs');
  const { join } = await import('node:path');
  const { qualifyInstalled } = await import('./installed.mjs');
  const { qualificationFixture } = await import('./qualification.fixture.mjs');
  const { evaluateQualification } = await import('../qualification.mjs');
  const f = qualificationFixture(t),
    target = f.reports[2];
  const report = await qualifyInstalled(
    {
      targetId: target.targetId,
      candidate: f.candidate,
      directory: f.directory,
      output: join(f.directory, 'fresh-failure'),
    },
    async (options) => {
      mkdirSync(options.output);
      writeFileSync(join(options.output, 'unit-evidence.txt'), 'Synthetic scenario evidence\n');
      const result = {
        ...structuredClone(target),
        scenarios: target.scenarios.filter((item) => item.id !== 'UPGRADE-01'),
      };
      if (!options.baselineDirectory) {
        result.status = 'failed';
        result.scenarios = result.scenarios.map((item) =>
          item.id === 'GRAPH-01'
            ? { ...item, status: 'failed', error: 'Fresh UI graph failed' }
            : item,
        );
      }
      return result;
    },
  );
  assert.equal(report.scenarios.find((item) => item.id === 'UPGRADE-01').status, 'passed');
  assert.equal(report.scenarios.find((item) => item.id === 'GRAPH-01').status, 'failed');
  assert.equal(report.status, 'failed');
  const all = f.reports.map((item) => (item.targetId === target.targetId ? report : item));
  assert.equal(evaluateQualification(f.candidate, all).passed, false);
});

test('a failed payload check after a fresh installation still runs package removal', async () => {
  const { spawnSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  const result = spawnSync(process.execPath, ['--experimental-test-module-mocks', '--test',
    fileURLToPath(new URL('./installed-cleanup.fixture.mjs', import.meta.url))], { encoding: 'utf8', env });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /PKG-02: passed/);
});
