import { volumeStep } from './action-settings.mjs';
import { operations } from './capabilities.mjs';
import { targetNode } from './control-target.mjs';
import { clampGainDb } from './gain.mjs';

function requireRunning(snapshot, node) {
  if (snapshot.enginePower && (!snapshot.enginePower.enabled || snapshot.enginePower.transitioning))
    throw new Error('Audio engine is off or changing');
  if (!node.control.enabled) throw new Error('Node is off');
}

/** Derive an absolute, revision-fenced command from the state shown on the device. */
export function commandFor(snapshot, settings, operation = settings.operation, ticks = 1) {
  const node = targetNode(snapshot, settings);
  if (
    !snapshot?.session ||
    !Number.isSafeInteger(snapshot.revision) ||
    !Number.isSafeInteger(snapshot.activeRevision) ||
    snapshot.activeRevision < snapshot.revision ||
    !operations(node).includes(operation)
  )
    throw new Error('Target unavailable');
  const base = {
    session: snapshot.session,
    target: { graphId: settings.graphId, nodeId: settings.nodeId },
  };
  const request = { expectedRevision: snapshot.revision };
  if (operation === 'power') request.enabled = !node.control.enabled;
  else {
    requireRunning(snapshot, node);
    if (operation === 'mute') request.muted = !node.control.muted;
    else if (operation.startsWith('gain_')) {
      if (!Number.isFinite(ticks)) throw new Error('Invalid dial value');
      request.gainDb = clampGainDb(
        node.gainDb + ticks * volumeStep(settings) * (operation === 'gain_up' ? 1 : -1),
      );
    } else return recorderCommand(base, request, node, operation);
  }
  return { type: 'node', ...base, request };
}

function recorderCommand(base, request, node, action) {
  const recording = node.recorder;
  if (!recording) throw new Error('Recorder unavailable');
  if (
    (action === 'record' && recording.state === 'recording') ||
    (action === 'pause' && recording.state !== 'recording') ||
    (action === 'stop' && !['recording', 'paused'].includes(recording.state))
  )
    throw new Error('Recording state changed');
  return {
    type: 'recorder',
    ...base,
    request: {
      ...request,
      request: { action },
      expectedRecording: { state: recording.state, path: recording.path ?? null },
    },
  };
}
