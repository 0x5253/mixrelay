import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { useSustainedProblem } from '../../core/ui/useSustainedProblem';
import { presentRoutingApplicationIcons } from './applicationIcons';
import { presentDriverEndpointControls } from './driverEndpointControls';
import { presentEqualizerSampleRate } from './equalizerPresentation';
import { presentHardwareNodeStatus } from './hardwareNodeStatus';
import { presentProcessorFailures } from './processorFailures';
import { presentProcessorLayouts } from './processorLayouts';
import { presentProcessorTimings } from './processorTiming';
import { presentNodeDisplayNames } from './useNodeDisplayNameEditor';
import { useVirtualDeviceAppActivity } from './useVirtualDeviceAppActivity';
import { presentVirtualDeviceAppActivity } from './virtualDeviceAppActivity';
import { presentApplicationReselection, presentVirtualNodeStatus } from './virtualNodeStatus';
import type { WorkspaceViewProps } from './workspaceViewProps';

type PresentedModelInputs = Pick<
  WorkspaceViewProps,
  | 'applications'
  | 'audioDevices'
  | 'audioSession'
  | 'controlPort'
  | 'deviceDriverAction'
  | 'failedProcessors'
  | 'graphId'
  | 'inventory'
  | 'layout'
  | 'model'
  | 'processors'
  | 'processorTimings'
  | 'state'
>;

function useDeviceActivityModel(props: PresentedModelInputs) {
  const { t, i18n } = useTranslation();
  const settings = props.audioSession?.virtualEndpointSettings;
  const activity = useVirtualDeviceAppActivity(props.controlPort, settings);
  return useMemo(
    () =>
      props.model &&
      presentVirtualDeviceAppActivity(props.model, settings, activity, t, i18n.language),
    [props.model, settings, activity, t, i18n.language],
  );
}

/** The canvas model with device, application, Driver, processor and naming status applied. */
export function usePresentedModel(props: PresentedModelInputs) {
  const { t } = useTranslation();
  const model = useDeviceActivityModel(props);
  const { applications, audioDevices, inventory, audioSession, deviceDriverAction } = props;
  // A graph apply usually settles well under a second; only a lasting one is a warning.
  const state = useSustainedProblem(props.state, (current) => current !== 'active');
  const presented = useMemo(
    () =>
      model
        ? presentVirtualNodeStatus(
            presentDriverEndpointControls(
              presentApplicationReselection(
                presentRoutingApplicationIcons(
                  presentHardwareNodeStatus(model, audioDevices, inventory.devicesObserved, t),
                  applications,
                ),
                t,
              ),
              deviceDriverAction,
              t,
            ),
            state,
            audioSession,
            t,
          )
        : null,
    [
      applications,
      audioDevices,
      inventory.devicesObserved,
      model,
      audioSession,
      state,
      t,
      deviceDriverAction,
    ],
  );
  return useMemo(
    () =>
      presented &&
      presentNodeDisplayNames(
        presentProcessorFailures(
          presentProcessorLayouts(
            presentEqualizerSampleRate(
              presentProcessorTimings(presented, props.processorTimings, props.graphId),
              audioSession?.format.sampleRate,
            ) as AudioGraphModel,
            props.processors,
            t,
          ),
          props.failedProcessors,
          props.graphId,
          t,
        ),
        props.layout.layout.nodeLabels,
      ),
    [
      presented,
      props.processors,
      props.failedProcessors,
      t,
      props.processorTimings,
      props.graphId,
      audioSession?.format.sampleRate,
      props.layout.layout.nodeLabels,
    ],
  );
}
