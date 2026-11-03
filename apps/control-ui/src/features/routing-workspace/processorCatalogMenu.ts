import type { TFunction } from 'i18next';
import type { AudioGraphNodeMenuItem } from '../../core/audio-graph/AudioGraphCanvas';
import type {
  ExternalProcessorDescriptor,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { providerPortsFrom } from './model';
import type { useProcessorCatalog } from './useProcessorCatalog';

/** Stable leaf identity shared by menu construction and processor insertion. */
export function processorItemId(processor: ExternalProcessorDescriptor): string {
  return `processor:${processor.typeId}`;
}

export function processorLayoutLabel(layout: ProviderPorts, t: TFunction): string {
  const mainInputs = layout.inputs.filter((bus) => bus.kind === 'main');
  const mainOutputs = layout.outputs.filter((bus) => bus.kind === 'main');
  const inputLabel = mainBusLabel(mainInputs, t);
  const outputLabel = mainBusLabel(mainOutputs, t);
  const mainLabel =
    inputLabel && outputLabel
      ? inputLabel === outputLabel
        ? inputLabel
        : t('routing.processorLayouts.inputOutput', { input: inputLabel, output: outputLabel })
      : (inputLabel ?? outputLabel);
  const extras = (['sidechain', 'auxiliary'] as const).flatMap((kind) => {
    const buses = [...layout.inputs, ...layout.outputs].filter((bus) => bus.kind === kind);
    if (!buses.length) return [];
    const channels = buses.reduce((total, bus) => total + bus.channels, 0);
    const count = buses.length;
    const label = t(`routing.processorLayouts.${kind}`);
    const channelCount = t('routing.processorLayouts.channelCount', { count: channels });
    return [
      t(
        count === 1
          ? 'routing.processorLayouts.singleBus'
          : 'routing.processorLayouts.multipleBuses',
        { label, channels: channelCount, count },
      ),
    ];
  });
  return (
    [mainLabel, ...extras].filter((label): label is string => Boolean(label)).join(' + ') ||
    t('routing.processorLayouts.noPorts')
  );
}

function mainBusLabel(buses: ProviderPorts['inputs'], t: TFunction) {
  const [first] = buses;
  if (!first) return null;
  if (buses.length === 1) return channelFormatLabel(first, t);
  return t('routing.processorLayouts.channelCount', {
    count: buses.reduce((total, bus) => total + bus.channels, 0),
  });
}

function channelFormatLabel(bus: ProviderPorts['inputs'][number], t: TFunction) {
  switch (bus.channelFormat) {
    case 'mono':
      return t('routing.processorLayouts.mono');
    case 'stereo':
      return t('routing.processorLayouts.stereo');
    case 'surround5_1':
      return t('routing.processorLayouts.surround5_1');
    case 'surround7_1':
      return t('routing.processorLayouts.surround7_1');
    case 'discrete':
      return t('routing.processorLayouts.channelCount', { count: bus.channels });
  }
}

export function processorLayoutDescription(layout: ProviderPorts, t: TFunction) {
  // Menu descriptions only receive layouts validated with non-empty inputs and outputs.
  const describe = (buses: ProviderPorts['inputs']) =>
    buses
      .map((bus) =>
        t('routing.processorLayouts.busDescription', {
          kind: t(`routing.processorLayouts.${bus.kind}`),
          name: bus.name,
          channels: t('routing.processorLayouts.channelCount', { count: bus.channels }),
        }),
      )
      .join(', ');

  return t('routing.processorLayouts.directions', {
    inputs: describe(layout.inputs),
    outputs: describe(layout.outputs),
  });
}

/** Resolves the menu action to its descriptor and the layout a new node starts with. */
export function processorSelectionForMenuItem(
  itemId: string,
  processors: readonly ExternalProcessorDescriptor[],
): { ports: ProviderPorts; processor: ExternalProcessorDescriptor } | null {
  const processor = processors.find((candidate) => processorItemId(candidate) === itemId);
  const ports = processor ? initialProcessorLayout(processor) : null;
  return processor && ports ? { ports, processor } : null;
}

/** New nodes start stereo when the plug-in offers a plain 2-in/2-out layout; otherwise
 * with the plug-in's own default. The layout is then changed on the node. */
export function initialProcessorLayout(processor: ExternalProcessorDescriptor) {
  const layouts = selectableProcessorLayouts(processor);
  // Exactly one stereo main bus each way; dual mono or a discrete pair is not "stereo".
  const stereo = (buses: ProviderPorts['inputs']) =>
    buses.length === 1 &&
    buses[0]?.kind === 'main' &&
    buses[0].channelFormat === 'stereo' &&
    buses[0].channels === 2;
  return (
    layouts.find((layout) => stereo(layout.inputs) && stereo(layout.outputs)) ??
    layouts.find((layout) => layout.layoutId === processor.defaultLayoutId) ??
    layouts[0] ??
    null
  );
}

/** One entry per plug-in directly below Processor; the channel layout is chosen on the node. */
export function createProcessorMenuItems(
  processors: readonly ExternalProcessorDescriptor[],
  t: TFunction,
  emptyLabel: string,
): AudioGraphNodeMenuItem[] {
  if (!processors.length) return [{ disabled: true, id: 'no-processors', label: emptyLabel }];
  return [...processors]
    .sort(
      (a, b) =>
        a.displayName.localeCompare(b.displayName) ||
        a.format.localeCompare(b.format) ||
        a.vendor.localeCompare(b.vendor) ||
        a.typeId.localeCompare(b.typeId),
    )
    .flatMap((processor) => processorMenuItems(processor, t));
}

function processorMenuItems(processor: ExternalProcessorDescriptor, t: TFunction) {
  const initial = initialProcessorLayout(processor);
  if (!initial) return [];
  const identity = [t(`routing.groups.${processor.format}`), processor.vendor]
    .filter(Boolean)
    .join(' · ');
  return [
    {
      description: `${identity} · ${processorLayoutLabel(initial, t)}`,
      id: processorItemId(processor),
      label: processor.displayName,
    },
  ];
}

/** Layouts with a unique ID that validate as provider ports, in catalog order. */
export function selectableProcessorLayouts(
  processor: ExternalProcessorDescriptor,
): ProviderPorts[] {
  const counts = new Map<string, number>();
  for (const layout of processor.layouts) {
    counts.set(layout.layoutId, (counts.get(layout.layoutId) ?? 0) + 1);
  }
  return processor.layouts.flatMap((layout) => {
    if (counts.get(layout.layoutId) !== 1) return [];
    const validated = providerPortsFrom(layout);
    return validated ? [validated] : [];
  });
}

/** An empty list is authoritative only after discovery finished without an error. */
export function processorCatalogPlaceholder(
  catalog: Pick<ReturnType<typeof useProcessorCatalog>, 'busy' | 'failure' | 'diagnostics'>,
) {
  if (catalog.busy) return 'routing.processorControl.scanning';
  if (catalog.failure || catalog.diagnostics.length) return 'routing.processorControl.incomplete';
  return 'routing.noProcessors';
}
