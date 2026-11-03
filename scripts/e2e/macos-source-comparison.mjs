import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { analyseHalIo } from './analyse-hal-io.mjs';
import { prepareSourceComparison } from './prepare-source-comparison.mjs';
import { sourceComparisonPlan, sourceComparisonPcm, sourceOrders } from './source-comparison-plan.mjs';
import { analyseSourceComparison } from './source-comparison-analysis.mjs';
import { analyseSourceNative } from './source-comparison-native.mjs';
import { analyseHalPcmContract } from './hal-pcm-contract.mjs';
import { requireObservedSegments, sourcePcmReference } from './macos-pcm-evidence.mjs';

const save = (directory, file, value) => writeFileSync(join(directory, file), JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });

export async function compareMacosSources(device, directory, { measurementBoundary, execute = execFileSync,
  prepare = prepareSourceComparison } = {}) {
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Source comparison requires a disposable hosted Mac');
  const uid = device.nativeDeviceIds.find(id => id.startsWith('io.mixrelay.audio.endpoint.'));
  assert.ok(uid, 'Missing canonical CoreAudio input device identity');
  mkdirSync(directory, { recursive: true });
  const bundle = prepare(join(directory, 'tools'), execute);
  const executable = `${bundle}/Contents/MacOS/MRSourceControl`;
  const executableSha256 = createHash('sha256').update(readFileSync(executable)).digest('hex');
  const results = [], errors = [];
  try {
    execute('open', ['-W', '-n', '-g', '--stdout', join(directory, 'authorization.json'),
      '--stderr', join(directory, 'authorization.stderr.log'), bundle, '--args', '--authorize-input', '--json'], { timeout: 60000 });
    const authorization = JSON.parse(readFileSync(join(directory, 'authorization.json'), 'utf8'));
    assert.equal(authorization.inputAuthorization?.final, 3, 'Source comparison consent must finish before observer suspension');
    // Both orders are fixed before execution. A failed order is retained and never replayed.
    for (const order of sourceOrders) {
      const output = join(directory, order), plan = sourceComparisonPlan(order), source = sourceComparisonPcm(plan);
      mkdirSync(output);
      writeFileSync(join(output, 'source.f32'), source, { flag: 'wx' });
      save(output, 'plan.json', { ...plan, sourceSha256: createHash('sha256').update(source).digest('hex'), uid });
      let launchFailed = false;
      try {
        await measurementBoundary(() => {
          try {
            execute('open', ['-W', '-n', '-g', '--stdout', join(output, 'native.json'),
              '--stderr', join(output, 'timing.csv'), bundle, '--args', uid,
              join(output, 'source.f32'), join(output, 'capture.f32'), join(output, 'hal-io.json')], { timeout: 60000 });
          } catch (error) { launchFailed = true; throw error; }
        }, { receiptDirectory: output });
      } catch (error) { errors.push({ order, boundary: 'native_process_or_observer', message: error.message }); }
      try {
        const metrics = JSON.parse(readFileSync(join(output, 'native.json'), 'utf8'));
        assert.equal(metrics.error, undefined, 'Native source comparison failed before drain');
        const analysis = analyseSourceComparison(plan, source, metrics, readFileSync(join(output, 'timing.csv'), 'utf8'), readFileSync(join(output, 'capture.f32')));
        const rawTrace = JSON.parse(readFileSync(join(output, 'hal-io.json'), 'utf8'));
        assert.equal(rawTrace.version, 5, 'Loaded HAL does not match the candidate trace schema');
        const trace = analyseHalIo(rawTrace), native = analyseSourceNative(rawTrace, plan, source, metrics.origin);
        const traceComplete = trace.retention.issued > 0 && trace.retention.writerDropped === 0
          && trace.retention.notRetainedRecentRows === 0 && trace.retention.observationsBeyondCapacity === 0;
        const product = requireObservedSegments(analyseHalPcmContract(rawTrace, { source, origin: metrics.origin, channels: 2 }), plan.segments);
        const passed = analysis.nativeErrors.length === 0 && product.passed;
        save(output, 'analysis.json', { ...analysis, passed, callbackAndPayloadPassed: analysis.passed, trace, traceComplete, native, product });
        results.push({ order, passed, segments: analysis.segments, reference: sourcePcmReference(plan, product),
          traceRetention: trace.retention });
      } catch (error) { errors.push({ order, boundary: 'evidence_analysis', message: error.message }); }
      // A timed-out LaunchServices wait may leave the app running. Reap it before any further IO.
      if (launchFailed) break;
    }
  } catch (error) { errors.push({ boundary: 'authorization', message: error.message }); }
  finally {
    const pattern = `^${executable.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}( |$)`;
    try { execute('pkill', ['-TERM', '-f', pattern], { timeout: 5000 }); }
    catch (error) { if (error.status !== 1) errors.push({ boundary: 'process_cleanup', message: error.message }); }
    save(directory, 'comparison.json', { uid, executableSha256, orders: sourceOrders, results, errors,
      passed: errors.length === 0 && results.length === 2 && results.every(result => result.passed),
      qualificationReplacement: false });
  }
  assert.equal(errors.length, 0, `Source comparison execution failed: ${JSON.stringify(errors)}`);
  assert.ok(results.length === 2 && results.every(result => result.passed), 'Silent/audible source comparison failed; see comparison.json');
  return results;
}
