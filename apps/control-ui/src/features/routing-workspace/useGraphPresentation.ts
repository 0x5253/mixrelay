import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { defaultEffectLabels } from './builtinEffects';
import type { GraphPresentation } from './graphPresentation';

/** The graph's node and port names in the current language. */
export function useGraphPresentation(): GraphPresentation {
  const { t } = useTranslation();
  return useMemo<GraphPresentation>(
    () => ({
      effectLabels: Object.fromEntries(
        Object.keys(defaultEffectLabels).map((key) => [key, t(`effects.${key}`)]),
      ),
      application: t('routing.nodes.application'),
      attack: t('routing.nodes.attack'),
      audioInput: t('routing.nodes.audioInput'),
      audioOutput: t('routing.nodes.audioOutput'),
      recorder: t('recorder.title'),
      channels: t('driverManagement.channels'),
      ceiling: t('routing.nodes.ceiling'),
      compressor: t('routing.nodes.compressor'),
      deviceName: t('routing.nodes.deviceName'),
      virtualSourcePort: t('routing.nodes.virtualSourcePort'),
      gain: t('routing.nodes.gain'),
      limiter: t('routing.nodes.limiter'),
      makeupGain: t('routing.nodes.makeupGain'),
      mixer: t('routing.nodes.mixer'),
      options: t('routing.nodes.options'),
      outputGain: t('routing.nodes.outputGain'),
      processor: t('routing.nodes.processor'),
      processorLayoutUnavailable: t('routing.nodes.processorLayoutUnavailable'),
      ports: {
        application: t('routing.ports.application'),
        deviceInput: t('routing.ports.deviceInput'),
        deviceOutput: t('routing.ports.deviceOutput'),
        effectInput: t('routing.ports.effectInput', { name: '{{name}}' }),
        effectOutput: t('routing.ports.effectOutput', { name: '{{name}}' }),
        gainInput: t('routing.ports.gainInput'),
        gainOutput: t('routing.ports.gainOutput'),
        mixerInput: t('routing.ports.mixerInput'),
        mixerOutput: t('routing.ports.mixerOutput'),
      },
      providerBusKinds: {
        main: t('routing.processorLayouts.main'),
        sidechain: t('routing.processorLayouts.sidechain'),
        auxiliary: t('routing.processorLayouts.auxiliary'),
      },
      ratio: t('routing.nodes.ratio'),
      release: t('routing.nodes.release'),
      routing: t('routing.groups.routing'),
      sourceGain: t('routing.nodes.sourceGain'),
      virtualSinkPort: t('routing.nodes.virtualSinkPort'),
      threshold: t('routing.nodes.threshold'),
      unsupported: t('routing.nodes.unsupported'),
      utility: t('routing.groups.utility'),
      virtualInput: t('routing.nodes.virtualInput'),
      virtualOutput: t('routing.nodes.virtualOutput'),
    }),
    [t],
  );
}
