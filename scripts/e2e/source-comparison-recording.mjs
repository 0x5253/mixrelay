import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { compareMacosSources } from './macos-source-comparison.mjs';
import { analyseSourceRecording } from './source-recording-analysis.mjs';
import { analyseRecordingPcmContract } from './recording-pcm-contract.mjs';

function analyseFinalizedTake(directory, results) {
  const bytes = readFileSync(join(directory, 'recording.wav'));
  const analysis = { recordingSha256: createHash('sha256').update(bytes).digest('hex'), ...analyseSourceRecording(bytes) };
  writeFileSync(join(directory, 'recording-analysis.json'), JSON.stringify(analysis, null, 2) + '\n', { flag: 'wx' });
  const references = results?.map(result => result.reference)
    ?? (existsSync(join(directory, 'comparison.json')) ? JSON.parse(readFileSync(join(directory, 'comparison.json'))).results.map(result => result.reference) : null);
  const product = analyseRecordingPcmContract(bytes, references);
  writeFileSync(join(directory, 'recording-contract.json'), JSON.stringify({ recordingSha256: analysis.recordingSha256, ...product }, null, 2) + '\n', { flag: 'wx' });
  assert.ok(product.passed, 'Engine recording lost, repeated or changed received source PCM; see recording-contract.json');
}

// Keep the same from_os graph active through both source orders and finalize its take.
export async function recordSourceComparison({ api, controlTake, recorderPath, revision,
  device, directory, measurementBoundary, compare = compareMacosSources }) {
  mkdirSync(directory, { recursive: true });
  const before = await api(recorderPath);
  assert.equal(before.state, 'stopped', 'Required recording must be finalized before source diagnosis');
  await controlTake({ expectedRevision: revision,
    expectedRecording: { state: before.state, path: before.path },
    request: { action: 'record', path: join(directory, 'recording.wav'), sampleRate: 48000, encoding: 'float32' } });
  const errors = [];
  let results;
  try { results = await compare(device, directory, { measurementBoundary }); }
  catch (error) { errors.push(error); }
  finally {
    try {
      const current = await api(recorderPath);
      if (current.state === 'failed') {
        // The recorder closes a failed take after draining its accepted prefix.
        // Preserve that terminal failure independently of its finalized PCM analysis.
        writeFileSync(join(directory, 'recording-stop.json'), JSON.stringify(current, null, 2) + '\n', { flag: 'wx' });
        errors.push(new Error(`Recorder failed: ${current.error ?? 'unknown failure'}`));
        analyseFinalizedTake(directory, results);
      } else {
        assert.equal(current.state, 'recording');
        const stopped = await controlTake({ expectedRevision: revision,
          expectedRecording: { state: current.state, path: current.path }, request: { action: 'stop' } });
        writeFileSync(join(directory, 'recording-stop.json'), JSON.stringify(stopped, null, 2) + '\n', { flag: 'wx' });
        assert.equal(stopped.state, 'stopped');
        assert.equal(stopped.error, null);
        analyseFinalizedTake(directory, results);
      }
    } catch (error) { errors.push(error); }
  }
  if (errors.length) throw new AggregateError(errors, errors.map(error => error.message).join('; '));
}
