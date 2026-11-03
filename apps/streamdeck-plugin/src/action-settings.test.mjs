import assert from 'node:assert/strict';
import test from 'node:test';
import { actionIds, actionSettings, supportsAction, volumeStep } from './action-settings.mjs';
import { commandFor } from './control-command.mjs';
import { translate } from './localization.mjs';

const gain = {
  nodeId: 'n',
  gainDb: 0,
  control: { enabled: true, muted: false },
  capabilities: { power: true, mute: true, recorder: false },
};
const settings = { graphId: 'g', nodeId: 'n' };
const snapshot = {
  session: 'a',
  revision: 1,
  activeRevision: 1,
  graphs: [{ id: 'g', nodes: [gain] }],
};

test('distinct action identity fixes its command family and preserves existing power targets', () => {
  assert.equal(
    actionSettings(actionIds.power, { ...settings, operation: 'record' }).operation,
    'power',
  );
  assert.equal(actionSettings(actionIds.mute, settings).operation, 'mute');
  assert.equal(actionSettings(actionIds.recording, settings).operation, 'record');
  assert.equal(
    actionSettings(actionIds.volume, { ...settings, direction: 'down' }).operation,
    'gain_down',
  );
  assert.equal(
    actionSettings(actionIds.volume, { ...settings, direction: 'down' }, true).operation,
    'gain_up',
  );
});

test('volume buttons and signed dial ticks use the configured step', () => {
  for (const stepDb of [0.5, 1, 2, 3]) {
    const button = actionSettings(actionIds.volume, { ...settings, stepDb, direction: 'down' });
    assert.equal(commandFor(snapshot, button).request.gainDb, -stepDb);
    assert.equal(commandFor(snapshot, button, 'gain_up', 3).request.gainDb, 3 * stepDb);
  }
  assert.equal(volumeStep({}), 0.5);
  assert.throws(() => volumeStep({ stepDb: 100 }));
});

test('discovery filters each action to real capabilities without invented volume or recorders', () => {
  assert.equal(supportsAction(actionIds.volume, gain), true);
  assert.equal(supportsAction(actionIds.recording, gain), false);
  assert.equal(supportsAction(actionIds.volume, { ...gain, gainDb: undefined }), false);
  assert.equal(
    supportsAction(actionIds.recording, {
      ...gain,
      capabilities: { ...gain.capabilities, recorder: true },
    }),
    true,
  );
  assert.equal(
    supportsAction(actionIds.mute, {
      ...gain,
      capabilities: { ...gain.capabilities, mute: false },
    }),
    false,
  );
  assert.equal(supportsAction(actionIds.volume, undefined), false);
});

test('property inspector uses host Korean or English without translating node identities', () => {
  assert.equal(translate('ko', 'Volume'), '볼륨');
  assert.equal(translate('ko-KR', 'Folder'), '폴더');
  assert.equal(translate('ko-KR', 'Format'), '형식');
  assert.equal(translate('en', 'Volume'), 'Volume');
  assert.equal(translate('ko', 'User node name'), 'User node name');
});
