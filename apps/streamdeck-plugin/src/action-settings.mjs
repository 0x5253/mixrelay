import { operations } from './capabilities.mjs';

export const actionIds = {
  power: 'com.rawsector.mixrelay.power',
  mute: 'com.rawsector.mixrelay.mute',
  volume: 'com.rawsector.mixrelay.volume',
  recording: 'com.rawsector.mixrelay.recording',
};
export const volumeSteps = [0.5, 1, 2, 3];

export function volumeStep(settings) {
  const step = settings.stepDb ?? 0.5;
  if (!volumeSteps.includes(step)) throw new Error('Invalid volume step');
  return step;
}

/** Action identity selects the command family; settings cannot turn a power key into a recorder. */
export function actionSettings(actionId, settings, dial = false) {
  switch (actionId) {
    case actionIds.power:
      return { ...settings, operation: 'power' };
    case actionIds.mute:
      return { ...settings, operation: 'mute' };
    case actionIds.volume:
      return {
        ...settings,
        operation: !dial && settings.direction === 'down' ? 'gain_down' : 'gain_up',
      };
    case actionIds.recording:
      return {
        ...settings,
        operation: ['record', 'pause', 'stop'].includes(settings.operation)
          ? settings.operation
          : 'record',
      };
    default:
      throw new Error('Unknown action');
  }
}

export function supportsAction(actionId, node) {
  const available = operations(node);
  if (actionId === actionIds.volume) return available.includes('gain_up');
  if (actionId === actionIds.recording) return available.includes('record');
  return available.includes(actionSettings(actionId, {}).operation);
}
