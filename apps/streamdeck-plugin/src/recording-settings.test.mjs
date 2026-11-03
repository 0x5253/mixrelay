import assert from 'node:assert/strict';
import test from 'node:test';
import { recordingSettingsSummary } from './recording-settings.mjs';

test('linked settings display the shared input-rate and PCM16 defaults', () => {
  const node = {
    recordingSettings: { folder: '/Users/person/Music/takes', sampleRate: null, encoding: 'pcm16' },
  };
  assert.deepEqual(recordingSettingsSummary(node, 'en'), {
    folder: '/Users/person/Music/takes',
    format: 'Follow input · 16-bit PCM',
  });
  assert.deepEqual(recordingSettingsSummary(node, 'ko'), {
    folder: '/Users/person/Music/takes',
    format: '입력과 동일 · 16비트 PCM',
  });
});

test('the summary reflects the current desktop settings without local copies', () => {
  const node = { recordingSettings: { folder: '/takes/A', sampleRate: 44100, encoding: 'pcm24' } };
  assert.deepEqual(recordingSettingsSummary(node, 'en'), {
    folder: '/takes/A',
    format: '44.1 kHz · 24-bit PCM',
  });
  node.recordingSettings = { folder: '/takes/B', sampleRate: 48000, encoding: 'float32' };
  assert.deepEqual(recordingSettingsSummary(node, 'en'), {
    folder: '/takes/B',
    format: '48 kHz · 32-bit float',
  });
});

test('missing target or settings does not manufacture a recording configuration', () => {
  assert.deepEqual(recordingSettingsSummary(undefined, 'en'), {
    folder: 'Unavailable',
    format: 'Unavailable',
  });
  assert.equal(
    recordingSettingsSummary(
      { recordingSettings: { folder: '', sampleRate: null, encoding: 'pcm16' } },
      'ko',
    ).folder,
    'MixRelay에서 녹음 폴더를 선택하세요.',
  );
});
