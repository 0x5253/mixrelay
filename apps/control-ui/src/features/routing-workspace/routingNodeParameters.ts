import type { AudioGraphChangeKind } from '../../core/audio-graph/audioGraphCanvasTypes';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import {
  effectParameters,
  effectPresets,
  isEffectKind,
  refreshEffectControls,
} from './builtinEffects';
import { objectValue } from './domain/configValues';
import { processorDetail } from './processorNodes';
import type { NodeRuntime, ParameterControls } from './runtimeAudioNode';

export function updateRoutingNodeGain(
  model: AudioGraphModel,
  nodeId: string,
  gainDb: number,
): AudioGraphModel {
  if (!Number.isFinite(gainDb)) return model;
  const normalized = Math.max(-60, Math.min(12, Math.round(gainDb * 2) / 2));
  let changed = false;
  const nodes = model.nodes.map((node) => {
    const config = objectValue(node.runtime?.config);
    if (node.id !== nodeId || !node.appearance.gainControl || !node.runtime || !config) return node;
    if (config.gainDb === normalized) return node;
    changed = true;
    return {
      ...node,
      appearance: {
        ...node.appearance,
        detail:
          node.runtime.type === 'gain' ? `${normalized.toFixed(1)} dB` : node.appearance.detail,
        gainControl: { ...node.appearance.gainControl, valueDb: normalized },
      },
      runtime: { ...node.runtime, config: { ...config, gainDb: normalized } },
    };
  });
  return changed ? { ...model, nodes, revision: model.revision + 1 } : model;
}

export function updateRoutingNodeParameter(
  model: AudioGraphModel,
  nodeId: string,
  parameterId: string,
  value: number,
): AudioGraphModel {
  if (!Number.isFinite(value)) return model;
  let changed = false;
  const nodes = model.nodes.map((node) => {
    const controls = node.appearance.parameterControls;
    if (!controls) return node;
    const control = controls.find((candidate) => candidate.id === parameterId);
    const config = objectValue(node.runtime?.config);
    if (node.id !== nodeId || !node.runtime || !config || !control) return node;
    if (control.value === value) return node;
    const stepped = Number(
      (
        control.minimum +
        Math.round((value - control.minimum) / control.step) * control.step
      ).toPrecision(12),
    );
    const normalized = Math.max(control.minimum, Math.min(control.maximum, stepped));
    if (node.runtime.type === 'effect') {
      const next = updateBuiltinEffect(
        node,
        node.runtime,
        config,
        controls,
        parameterId,
        normalized,
      );
      changed ||= next !== node;
      return next;
    }
    if (node.runtime.type === 'provider') {
      const next = updateProviderParameter(
        node,
        node.runtime,
        config,
        controls,
        parameterId,
        normalized,
      );
      changed ||= next !== node;
      return next;
    }
    const nextValue = dynamicsParameterValue(node.runtime.type, parameterId, normalized);
    if (config[parameterId] === nextValue) return node;
    changed = true;
    const nextConfig = { ...config, [parameterId]: nextValue };
    return {
      ...node,
      appearance: {
        ...node.appearance,
        detail: processorDetail(node.runtime.type, nextConfig),
        parameterControls: controls.map((candidate) =>
          candidate.id === parameterId ? { ...candidate, value: normalized } : candidate,
        ),
      },
      runtime: { ...node.runtime, config: nextConfig },
    };
  });
  return changed ? { ...model, nodes, revision: model.revision + 1 } : model;
}

/** How a single parameter edit enters history: a preset replaces several values at once, and
 * a plug-in value is published to the engine directly rather than applied as a graph. */
export function parameterChangeKind(parameterId: string): AudioGraphChangeKind {
  if (parameterId.startsWith('external:')) return 'processor_parameter';
  return parameterId === 'preset' ? 'runtime' : 'level';
}

function dynamicsParameterValue(type: string, parameterId: string, value: number) {
  return parameterId === 'bypass' && (type === 'compressor' || type === 'limiter')
    ? Boolean(value)
    : value;
}

/** Apply a multi-axis gesture to the latest intermediate model as one graph edit. */
export function updateRoutingNodeParameters(
  model: AudioGraphModel,
  nodeId: string,
  changes: readonly { id: string; value: number }[],
): AudioGraphModel {
  const next = changes.reduce(
    (current, change) => updateRoutingNodeParameter(current, nodeId, change.id, change.value),
    model,
  );
  return next === model ? model : { ...next, revision: model.revision + 1 };
}

function externalParameterIdentity(parameterId: string): number | null {
  const encodedId = parameterId.slice(9);
  const identity = Number(encodedId);
  return parameterId.startsWith('external:') &&
    /^\d+$/u.test(encodedId) &&
    Number.isSafeInteger(identity)
    ? identity
    : null;
}

function updateProviderParameter(
  node: AudioGraphNode,
  runtime: NodeRuntime,
  config: Record<string, unknown>,
  controls: ParameterControls,
  parameterId: string,
  normalized: number,
): AudioGraphNode {
  const externalId = externalParameterIdentity(parameterId);
  if (externalId === null) return node;
  return updateExternalParameter(
    node,
    runtime,
    config,
    controls,
    parameterId,
    externalId,
    normalized,
  );
}

function updateExternalParameter(
  node: AudioGraphNode,
  runtime: NodeRuntime,
  config: Record<string, unknown>,
  controls: ParameterControls,
  parameterId: string,
  externalId: number,
  normalized: number,
): AudioGraphNode {
  const id = String(externalId);
  const { parameters, values } = editableParameterState(config);
  if (values[id] === normalized) return node;
  return {
    ...node,
    appearance: {
      ...node.appearance,
      parameterControls: controls.map((candidate) =>
        candidate.id === parameterId ? { ...candidate, value: normalized } : candidate,
      ),
    },
    runtime: {
      ...runtime,
      config: {
        ...config,
        parameters: { ...parameters, parameterValues: { ...values, [id]: normalized } },
      },
    },
  };
}

function updateBuiltinEffect(
  node: AudioGraphNode,
  runtime: NodeRuntime,
  config: Record<string, unknown>,
  controls: ParameterControls,
  parameterId: string,
  normalized: number,
): AudioGraphNode {
  const effect = objectValue(config.effect);
  if (!effect || !isEffectKind(effect.kind)) return node;
  const parameters = effectParameters(effect.kind, effect.parameters);
  if (!parameters) return node;
  let nextParameters = parameters;
  let nextConfig: Record<string, unknown>;
  if (parameterId === 'preset') {
    const preset = effectPresets[effect.kind]?.[normalized - 1];
    if (!preset) return node;
    nextParameters = { ...parameters, ...preset.parameters };
    nextConfig = { ...config, effect: { ...effect, parameters: nextParameters } };
  } else if (parameterId === 'bypass') {
    if (Number(Boolean(config.bypass)) === normalized) return node;
    nextConfig = { ...config, bypass: normalized === 1 };
  } else {
    if (parameters[parameterId] === normalized) return node;
    nextParameters = { ...parameters, [parameterId]: normalized };
    nextConfig = {
      ...config,
      effect: { ...effect, parameters: nextParameters },
    };
  }
  return {
    ...node,
    runtime: { ...runtime, config: nextConfig },
    appearance: {
      ...node.appearance,
      parameterControls: refreshEffectControls(
        effect.kind,
        nextParameters,
        nextConfig.bypass === true,
        controls,
      ),
    },
  };
}

function editableParameterState(config: Record<string, unknown>) {
  const parameters = objectValue(config.parameters) ?? {};
  const values = objectValue(parameters.parameterValues) ?? {};
  return { parameters, values };
}
