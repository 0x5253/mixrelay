import type { ControlStateSnapshot } from '../core/domain/control/state';
import type { StateListener } from '../core/ports/control/TelemetryPort';

export function controlState(generation = 1, revision = 2): ControlStateSnapshot {
  return {
    generation,
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    graph: { protocolVersion: 1, activeRevision: revision, submittedRevision: revision },
    controls: {
      revision,
      activeRevision: revision,
      structureGeneration: 1,
      graphs: [{ id: 'a', nodes: [] }],
    },
    editors: [],
  };
}

export function statePush(initial?: ControlStateSnapshot) {
  let listener: StateListener | undefined;
  return {
    subscribeState(receive: StateListener) {
      listener = receive;
      receive(initial);
      return () => {
        listener = undefined;
      };
    },
    push(state?: ControlStateSnapshot, engine?: string) {
      initial = state;
      listener?.(state, engine);
    },
  };
}

export function packedTelemetry(state = controlState()) {
  return {
    engineIncarnation: 'a',
    sequence: state.generation,
    state,
    meters: { protocolVersion: 1, revision: 2, bindings: [], samples: '' },
  };
}
