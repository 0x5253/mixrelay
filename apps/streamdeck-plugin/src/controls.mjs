import { operations } from './capabilities.mjs';
import { targetNode } from './control-target.mjs';

export function presentation(snapshot, settings) {
  const node = targetNode(snapshot, settings);
  if (!snapshot) return { title: 'MixRelay', value: 'Disconnected' };
  if (!node || !operations(node).includes(settings.operation))
    return { title: 'MixRelay', value: 'Unavailable' };
  const title = node.label || node.nodeId;
  if (snapshot.activeRevision < snapshot.revision) return { title, value: 'Pending' };
  if (!node.control.enabled) return { title, value: 'OFF' };
  if (snapshot.enginePower && (!snapshot.enginePower.enabled || snapshot.enginePower.transitioning))
    return { title, value: 'Engine OFF' };
  if (settings.operation === 'mute')
    return { title, value: node.control.muted ? 'MUTED' : 'Unmuted' };
  if (settings.operation?.startsWith('gain_'))
    return { title, value: `${node.gainDb.toFixed(1)} dB` };
  if (['record', 'pause', 'stop'].includes(settings.operation))
    return { title, value: node.recorder?.state ?? 'Unavailable' };
  return { title, value: 'ON' };
}
