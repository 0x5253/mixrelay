import type { TFunction } from 'i18next';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import { getAudioSessionPresentation } from '../../core/audio-session/presentation';
import type { AudioSessionStatus } from '../../core/domain/control/status';

export type GraphAvailability = 'active' | 'applying' | 'loading' | 'unavailable' | 'blocked';
type NodeStatus = Pick<AudioGraphNode['appearance'], 'status' | 'statusTone'>;

function sessionPresentation(
  state: GraphAvailability,
  session: AudioSessionStatus | undefined,
  t: TFunction,
): NodeStatus {
  if (state === 'unavailable')
    return { status: t('routing.states.unavailable'), statusTone: 'critical' };
  const health = getAudioSessionPresentation(session, t);
  if (health.tone === 'critical') return { status: health.label, statusTone: health.tone };
  // A failed DSP owner cannot activate a pending graph; applying must not mask that failure.
  if (state !== 'active') return { status: t(`routing.states.${state}`), statusTone: 'warning' };
  return { status: health.label, statusTone: health.tone };
}

/** Presentation only: refreshing status publishes no audio edit and never infers health from meters. */
export function presentVirtualNodeStatus(
  model: AudioGraphModel,
  state: GraphAvailability,
  session: AudioSessionStatus | undefined,
  t: TFunction,
): AudioGraphModel {
  const status = sessionPresentation(state, session, t);
  return {
    ...model,
    nodes: model.nodes.map((node) =>
      node.runtime?.type === 'virtual_endpoint'
        ? { ...node, appearance: { ...node.appearance, ...status } }
        : node,
    ),
  };
}

/** Restored PID selectors cannot safely follow a new process until explicitly selected again. */
export function presentApplicationReselection(
  model: AudioGraphModel,
  t: TFunction,
): AudioGraphModel {
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      const config = node.runtime?.config;
      if (
        node.runtime?.type !== 'application_capture' ||
        typeof config !== 'object' ||
        config === null ||
        !('requiresReselection' in config) ||
        config.requiresReselection !== true
      )
        return node;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          status: t('routing.nodeStatus.reselectApplication'),
          statusTone: 'warning' as const,
        },
      };
    }),
  };
}
