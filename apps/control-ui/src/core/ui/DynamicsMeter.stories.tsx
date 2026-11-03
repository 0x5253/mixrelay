import type { Meta, StoryObj } from '@storybook/react-vite';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { AudioMeterProvider } from '../audio-meter/AudioMeterProvider';
import { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import { DynamicsMeter } from './DynamicsMeter';

function Demo({
  state = 'compressed',
}: {
  state?: 'compressed' | 'below_threshold' | 'bypassed' | 'waiting';
}) {
  const { t } = useTranslation();
  const store = useMemo(() => {
    const value = new AudioMeterStore();
    if (state !== 'waiting')
      value.update({
        protocolVersion: 1,
        revision: 1,
        nodes: [
          {
            nodeId: 'compressor',
            portId: 'audio',
            peaks:
              state === 'compressed'
                ? [0.8414, 0.4207]
                : state === 'below_threshold'
                  ? [0.05, 0.025]
                  : [1, 0.5],
            dynamics: {
              inputPeaks: state === 'below_threshold' ? [0.05, 0.025] : [1, 0.5],
              gainReductionDb: state === 'compressed' ? 13.5 : 0,
              thresholdDb: -18,
            },
          },
        ],
      });
    return value;
  }, [state]);
  return (
    <div
      style={{
        width: 'var(--node-dynamics-width)',
        maxWidth: '100%',
        boxSizing: 'border-box',
        padding: 'var(--node-inset)',
      }}
    >
      <AudioMeterProvider store={store}>
        <DynamicsMeter
          contract={{
            nodeId: 'compressor',
            portId: 'audio',
            thresholdId: 'thresholdDb',
            inputLabel: t('effects.dynamicsInput'),
            outputLabel: t('effects.dynamicsOutput'),
            reductionLabel: t('effects.gainReduction'),
            unavailableLabel: t('effects.dynamicsUnavailable'),
          }}
          controls={[
            {
              id: 'thresholdDb',
              label: t('routing.nodes.threshold'),
              value: -18,
              minimum: -60,
              maximum: 0,
              step: 0.5,
              suffix: 'dB',
            },
          ]}
          bypassed={state === 'bypassed'}
        />
      </AudioMeterProvider>
    </div>
  );
}
const meta = {
  title: 'Core/Dynamics Meter',
  component: Demo,
  parameters: {
    docs: {
      description: {
        component:
          'Deterministic native telemetry fixtures: 13.5 dB attenuation with 12 dB makeup, below threshold, bypass and unavailable. These fixtures do not estimate reduction from input/output peaks.',
      },
    },
  },
} satisfies Meta<typeof Demo>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Compressing: Story = {};
export const BelowThreshold: Story = { args: { state: 'below_threshold' } };
export const Bypassed: Story = { args: { state: 'bypassed' } };
export const Waiting: Story = { args: { state: 'waiting' } };
