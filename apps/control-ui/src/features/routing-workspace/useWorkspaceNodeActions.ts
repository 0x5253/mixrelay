import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { PlaceGraphNodes } from '../../core/audio-graph/nodePlacement';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import type { DeviceDriverAction } from './driverEndpointControls';
import type { GraphPresentation } from './graphPresentation';
import { routingNodeRequest } from './nodeRequests';
import { useProcessorLayoutChange } from './processorLayouts';
import { addRoutingNode } from './routingNodeCreation';
import { ADD_PROCESSOR_DIRECTORY_ITEM_ID, RESCAN_PROCESSORS_ITEM_ID } from './routingNodeMenus';
import {
  parameterChangeKind,
  updateRoutingNodeGain,
  updateRoutingNodeParameter,
  updateRoutingNodeParameters,
} from './routingNodeParameters';
import { useVirtualEndpointCreation } from './useVirtualEndpointCreation';
import { type VirtualEndpointUpdate, virtualEndpointIdentity } from './virtualEndpointNodes';
import type { WorkspaceState } from './WorkspaceProblems';
import type { UpdateModel } from './workspaceViewProps';

interface RoutingNodeCreationOptions {
  tryVirtualAdd: (
    itemId: string,
    position: { x: number; y: number },
    placeNodes?: PlaceGraphNodes,
  ) => boolean;
  audioSession?: AudioSessionStatus;
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  model: AudioGraphModel | null;
  processors: readonly ExternalProcessorDescriptor[];
  presentation: GraphPresentation;
  refreshProcessors: (selectDirectory: boolean) => void;
  updateModel: UpdateModel;
}

function useRoutingNodeCreation(options: RoutingNodeCreationOptions) {
  const { t } = useTranslation();
  const {
    audioSession,
    applications,
    audioDevices,
    model,
    presentation,
    processors,
    refreshProcessors,
    updateModel,
    tryVirtualAdd,
  } = options;
  return useCallback(
    (itemId: string, position: { x: number; y: number }, placeNodes?: PlaceGraphNodes) => {
      if (tryVirtualAdd(itemId, position, placeNodes)) return;
      if (itemId === ADD_PROCESSOR_DIRECTORY_ITEM_ID) {
        refreshProcessors(true);
        return;
      }
      if (itemId === RESCAN_PROCESSORS_ITEM_ID) {
        refreshProcessors(false);
        return;
      }
      const request = routingNodeRequest(itemId, {
        applications,
        audioDevices,
        processors,
        systemAudioTitle:
          audioSession?.capabilities.backend === 'core_audio'
            ? t('studioGraph.nodes.systemAudio.name')
            : undefined,
      });
      if (!model || !request) return;
      const next = addRoutingNode(
        model,
        request,
        position,
        presentation,
        audioSession?.virtualEndpointSettings,
      );
      updateModel(placeNodes?.(next) ?? next, 'runtime');
    },
    [
      applications,
      audioDevices,
      audioSession,
      model,
      presentation,
      processors,
      refreshProcessors,
      updateModel,
      tryVirtualAdd,
      t,
    ],
  );
}

function useNodeValueEdits(model: AudioGraphModel | null, updateModel: UpdateModel) {
  return useMemo(() => {
    if (!model) return {};
    return {
      updateNodeGain: (nodeId: string, gainDb: number) =>
        updateModel(updateRoutingNodeGain(model, nodeId, gainDb), 'level'),
      updateNodeParameter: (nodeId: string, parameterId: string, value: number) => {
        updateModel(
          updateRoutingNodeParameter(model, nodeId, parameterId, value),
          parameterChangeKind(parameterId),
        );
      },
      updateNodeParameters: (nodeId: string, updates: readonly { id: string; value: number }[]) =>
        updateModel(updateRoutingNodeParameters(model, nodeId, updates), 'parameter_group'),
    };
  }, [model, updateModel]);
}

// Endpoint settings are a Driver transaction; they are never authored into the graph.
function useVirtualEndpointUpdate(model: AudioGraphModel | null, action?: DeviceDriverAction) {
  return useMemo(() => {
    if (!model || !action) return undefined;
    return (nodeId: string, update: VirtualEndpointUpdate) => {
      const node = model.nodes.find((candidate) => candidate.id === nodeId);
      const endpointId = node ? virtualEndpointIdentity(node) : null;
      if (action.enabled && endpointId !== null) action.change(endpointId, update);
    };
  }, [model, action]);
}

export interface WorkspaceNodeActionOptions {
  audioDevices: readonly AudioDeviceDescriptor[];
  audioSession?: AudioSessionStatus;
  applications: readonly CapturableApplication[];
  controlPort: ControlPort;
  deviceDriverAction?: DeviceDriverAction;
  interactionDisabled: boolean;
  model: AudioGraphModel | null;
  presentation: GraphPresentation;
  processors: readonly ExternalProcessorDescriptor[];
  refreshProcessors: (selectDirectory: boolean) => void;
  state: WorkspaceState;
  updateModel: UpdateModel;
}

/** The canvas commands that add nodes and edit their values, endpoints and layouts. */
export function useWorkspaceNodeActions(options: WorkspaceNodeActionOptions) {
  const virtualCreation = useVirtualEndpointCreation({
    owner: options.controlPort,
    model: options.model,
    settings: options.deviceDriverAction
      ? options.deviceDriverAction.settings
      : options.audioSession?.virtualEndpointSettings,
    createDevice: options.deviceDriverAction?.create,
    presentation: options.presentation,
    enabled:
      options.state === 'active' &&
      !options.interactionDisabled &&
      (options.deviceDriverAction
        ? options.deviceDriverAction.enabled
        : options.audioSession?.capabilities.virtualEndpoint === true),
    updateModel: (next) => options.updateModel(next, 'runtime'),
  });
  const addNode = useRoutingNodeCreation({
    tryVirtualAdd: virtualCreation.tryAdd,
    audioSession: options.audioSession,
    applications: options.applications,
    audioDevices: options.audioDevices,
    model: options.model,
    presentation: options.presentation,
    processors: options.processors,
    refreshProcessors: options.refreshProcessors,
    updateModel: options.updateModel,
  });
  const valueEdits = useNodeValueEdits(options.model, options.updateModel);
  const updateChannelLayout = useProcessorLayoutChange(
    options.model,
    options.processors,
    options.presentation,
    options.updateModel,
  );
  const updateEndpoint = useVirtualEndpointUpdate(options.model, options.deviceDriverAction);
  return { addNode, updateChannelLayout, updateEndpoint, ...valueEdits, virtualCreation };
}
