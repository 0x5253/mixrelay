import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { sourceComparisonPlan, sourceComparisonPcm, sourceOrders } from './source-comparison-plan.mjs';
import { analyseSourceNative } from './source-comparison-native.mjs';
import { analyseSourceComparison } from './source-comparison-analysis.mjs';
import { analyseSourceRecording } from './source-recording-analysis.mjs';
import { analyseHalPcmContract } from './hal-pcm-contract.mjs';
import { requireObservedSegments, sourcePcmReference } from './macos-pcm-evidence.mjs';
import { analyseRecordingPcmContract } from './recording-pcm-contract.mjs';

const { values, positionals } = parseArgs({ allowPositionals: true,
  options: { help: { type: 'boolean' }, output: { type: 'string' } } });
if (values.help) console.log('node scripts/e2e/analyse-source-comparison.mjs SOURCE_COMPARISON_DIRECTORY [--output NEW_FILE]\nChecks retained native publication and read coverage for both source states offline. Original PCM verdicts and evidence are preserved.');
else {
  assert.equal(positionals.length, 1);
  const reports = sourceOrders.map(order => {
    const directory = join(positionals[0], order);
    const plan = JSON.parse(readFileSync(join(directory, 'plan.json'))), source = readFileSync(join(directory, 'source.f32'));
    const canonical = sourceComparisonPlan(order);
    for (const [key, value] of Object.entries(canonical)) assert.deepEqual(plan[key], value, 'Original comparison plan changed');
    assert.deepEqual(source, sourceComparisonPcm(canonical), 'Original source PCM changed');
    assert.equal(createHash('sha256').update(source).digest('hex'), plan.sourceSha256);
    const metrics = JSON.parse(readFileSync(join(directory, 'native.json'))), trace = JSON.parse(readFileSync(join(directory, 'hal-io.json')));
    const originalEvidenceSha256 = Object.fromEntries(['plan.json', 'source.f32', 'native.json', 'timing.csv',
      'capture.f32', 'hal-io.json', 'analysis.json', 'native-permission-observation.json'].map(file => [file,
      createHash('sha256').update(readFileSync(join(directory, file))).digest('hex')]));
    const product = requireObservedSegments(analyseHalPcmContract(trace, { source, origin: metrics.origin, channels: 2 }), canonical.segments);
    return { order, originalEvidenceSha256, product, reference: sourcePcmReference(canonical, product),
      callbackAndPayload: analyseSourceComparison(plan, source, metrics, readFileSync(join(directory, 'timing.csv'), 'utf8'), readFileSync(join(directory, 'capture.f32'))),
      ...analyseSourceNative(trace, plan, source, metrics.origin) };
  });
  const recording = readFileSync(join(positionals[0], 'recording.wav'));
  const result = { analysisRevision: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    qualificationReplacement: false, reports,
    recording: { recordingSha256: createHash('sha256').update(recording).digest('hex'), ...analyseSourceRecording(recording) },
    recordingContract: analyseRecordingPcmContract(recording, reports.map(report => report.reference)) };
  const json = JSON.stringify(result, null, 2) + '\n';
  if (values.output) writeFileSync(values.output, json, { flag: 'wx' });
  else process.stdout.write(json);
}
