import type { TFunction } from 'i18next';
import type { AudioGraphNodeMenuItem } from '../../core/audio-graph/audioGraphCanvasTypes';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import { applicationInventorySelection } from './applicationIdentity';
import { chainKinds, effectKinds } from './builtinEffects';
import { createProcessorMenuItems } from './processorCatalogMenu';
import { virtualDeviceMenuItem } from './virtualEndpointMenu';
export const ADD_PROCESSOR_DIRECTORY_ITEM_ID = 'add-processor-directory';
export const RESCAN_PROCESSORS_ITEM_ID = 'rescan-processors';
export const PROCESSOR_SCAN_REPORT_ITEM_ID = 'processor-scan-report';
export const SYSTEM_AUDIO_ITEM_ID = 'system-audio';
/**
 * macOS bundle identities of MixRelay's own processes. System Audio excludes them so it
 * never captures the product's own output. `scripts/macos-identity.test.mjs` checks them
 * against the app and engine bundle definitions.
 */
export const SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS: readonly string[] = [
  'io.mixrelay.desktop',
  'io.mixrelay.service',
];
export function audioInputItemId(device: AudioDeviceDescriptor): string {
  return `audio-input:${device.uid}`;
}

export function audioOutputItemId(device: AudioDeviceDescriptor): string {
  return `audio-output:${device.uid}`;
}

function createApplicationMenuItems(
  applications: readonly CapturableApplication[],
  t: TFunction,
  supported = true,
): AudioGraphNodeMenuItem[] {
  const items = new Map<string, AudioGraphNodeMenuItem>();
  for (const application of applications) {
    const selection = applicationInventorySelection(application);
    if (!selection) continue;
    const iconUrl = application.iconDataUrl || undefined;
    const previous = items.get(selection.id);
    if (previous) {
      if (!previous.iconUrl && iconUrl) previous.iconUrl = iconUrl;
      continue;
    }
    items.set(selection.id, {
      disabled: !supported,
      description: supported ? undefined : t('audioSession.captureUnavailable'),
      iconUrl,
      id: selection.id,
      label: application.displayName,
    });
  }
  return items.size
    ? [...items.values()]
    : [{ disabled: true, id: 'no-applications', label: t('routing.noApplications') }];
}

interface MenuOptions {
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  processors: readonly ExternalProcessorDescriptor[];
  model: AudioGraphModel | null;
  t: TFunction;
  session?: AudioSessionStatus;
  processorEmptyLabel: string;
  processorReportDescription?: string;
}
function menuChoices({
  applications,
  audioDevices,
  processors,
  t,
  session,
  processorEmptyLabel,
}: MenuOptions) {
  const virtualIds = new Set(
    session?.virtualEndpointSettings?.devices.flatMap((device) => device.nativeDeviceIds),
  );
  const hostDevices = audioDevices.filter((device) => !virtualIds.has(device.uid));
  const inputs = hostDevices.filter((device) => device.inputChannels > 0);
  const outputs = hostDevices.filter((device) => device.outputChannels > 0);
  const applicationItems = createApplicationMenuItems(
    applications,
    t,
    session?.capabilities.applicationCapture,
  );
  const inputItems = hardwareItems(inputs, 'input', session?.capabilities.hardwareInput, t);
  const outputItems = hardwareItems(outputs, 'output', session?.capabilities.hardwareOutput, t);
  const processorItems = createProcessorMenuItems(processors, t, processorEmptyLabel);
  const systemItems: AudioGraphNodeMenuItem[] =
    session?.capabilities.backend === 'core_audio'
      ? [
          {
            id: SYSTEM_AUDIO_ITEM_ID,
            label: t('studioGraph.nodes.systemAudio.name'),
            disabled: !session.capabilities.applicationCapture,
            description: session.capabilities.applicationCapture
              ? t('studioGraph.nodes.systemAudio.description')
              : t('audioSession.captureUnavailable'),
          },
        ]
      : [];
  return { applicationItems, systemItems, inputItems, outputItems, processorItems };
}
function hardwareItems(
  devices: readonly AudioDeviceDescriptor[],
  side: 'input' | 'output',
  supported: boolean | undefined,
  t: TFunction,
): AudioGraphNodeMenuItem[] {
  return devices.length
    ? devices.map((device) => ({
        id: side === 'input' ? audioInputItemId(device) : audioOutputItemId(device),
        label: device.displayName,
        disabled: supported === false,
        description:
          supported === false
            ? t('audioSession.unsupported')
            : `${side === 'input' ? device.inputChannels : device.outputChannels} CH`,
      }))
    : [
        {
          disabled: true,
          id: `no-${side}-devices`,
          label: t(side === 'input' ? 'routing.noInputDevices' : 'routing.noOutputDevices'),
        },
      ];
}
export function createNodeMenuItems(options: MenuOptions): AudioGraphNodeMenuItem[] {
  const { t } = options;
  const { applicationItems, systemItems, inputItems, outputItems, processorItems } =
    menuChoices(options);
  return [
    {
      children: [
        virtualDeviceMenuItem('input', options.model, t, options.session),
        { id: 'virtual-input-divider', kind: 'divider', label: '' },
        ...systemItems,
        {
          id: 'running-applications-label',
          kind: 'label',
          label: t('routing.groups.runningApplications'),
        },
        ...applicationItems,
        { id: 'source-divider', kind: 'divider', label: '' },
        { id: 'audio-inputs-label', kind: 'label', label: t('routing.groups.audioInputs') },
        ...inputItems,
      ],
      id: 'source',
      label: t('routing.groups.source'),
    },
    {
      children: [
        {
          id: 'compressor',
          label: t('routing.nodes.compressor'),
        },
        {
          id: 'limiter',
          label: t('routing.nodes.limiter'),
        },
        { id: 'processor-divider', kind: 'divider', label: '' },
        ...effectKinds.map((kind) => ({ id: `builtin:${kind}`, label: t(`effects.${kind}`) })),
        {
          id: 'effect-chains',
          label: t('effects.chains'),
          children: chainKinds.map((kind) => ({
            id: `effect-chain:${kind}`,
            label: t(`effects.${kind}`),
          })),
        },
        ...processorItems,
        { id: 'processor-actions-divider', kind: 'divider', label: '' },
        {
          description: t('routing.addProcessorDirectoryDescription'),
          id: ADD_PROCESSOR_DIRECTORY_ITEM_ID,
          label: t('routing.addProcessorDirectory'),
        },
        { id: RESCAN_PROCESSORS_ITEM_ID, label: t('routing.rescanProcessors') },
        {
          id: PROCESSOR_SCAN_REPORT_ITEM_ID,
          label: t('routing.processorControl.viewReport'),
          description: options.processorReportDescription,
        },
      ],
      id: 'processor',
      label: t('routing.groups.processor'),
    },
    {
      children: [
        virtualDeviceMenuItem('output', options.model, t, options.session),
        {
          id: 'recorder',
          label: t('recorder.title'),
        },
        { id: 'virtual-output-divider', kind: 'divider', label: '' },
        { id: 'audio-outputs-label', kind: 'label', label: t('routing.groups.audioOutputs') },
        ...outputItems,
      ],
      id: 'output',
      label: t('routing.groups.output'),
    },
  ];
}
