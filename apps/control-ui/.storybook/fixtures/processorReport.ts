import type { StorybookControlState } from '../../src/core/adapters/fake/StorybookMockControlPort';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../src/core/domain/control/processors';

export const processorReportPorts: ProviderPorts = {
  layoutId: 'layout-v1:stereo',
  inputs: [{ id: 'audio', name: 'Main Input', kind: 'main', channels: 2, channelFormat: 'stereo' }],
  outputs: [
    { id: 'audio', name: 'Main Output', kind: 'main', channels: 2, channelFormat: 'stereo' },
  ],
};
export const reportProcessor: ExternalProcessorDescriptor = {
  typeId: 'vst3:mixrelay:report-fixture',
  displayName: 'Studio Effect',
  vendor: 'Example Audio',
  version: '1.0',
  format: 'vst3',
  hasEditor: false,
  layouts: [processorReportPorts],
  defaultLayoutId: processorReportPorts.layoutId,
  parameters: [],
};

export function processorReportState({
  failed = false,
  editor = false,
} = {}): StorybookControlState {
  return {
    graph: {
      schemaVersion: 2,
      revision: 1,
      edges: [],
      nodes: ['voice', 'music'].map((id) => ({
        id,
        label: id === 'voice' ? 'Voice effect' : 'Music effect',
        type: 'provider',
        config: { typeId: reportProcessor.typeId, ports: processorReportPorts },
      })),
    },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
      failedProcessors: failed
        ? [{ graphId: 'graph-1', nodeId: 'voice', instanceId: 'failed-instance' }]
        : [],
    },
    processors: {
      protocolVersion: 1,
      processors: [{ ...reportProcessor, hasEditor: editor }],
      diagnostics: Array.from({ length: 19 }, (_, index) => ({
        format: 'vst3',
        stage: index === 0 ? 'scan' : 'probe',
        pluginName: `Unavailable Effect ${index + 1}`,
        pluginId: `vst3:example:unavailable-${index + 1}`,
        message:
          'Plug-in validation did not complete. Native process exited with code 1.\n/Library/Audio/Plug-Ins/VST3/Example.vst3',
      })),
    },
  };
}
