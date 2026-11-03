import type { Meta, StoryObj } from '@storybook/react-vite';
import { type ComponentProps, useEffect, useState } from 'react';
import type { NativeDeviceFormats, NativeSampleFormat } from '../domain/control/nativeFormat';
import { NativeSampleFormatField } from './NativeSampleFormatField';

const encodings: NativeSampleFormat[] = ['signed16', 'signed24', 'signed32', 'float32'];
const stream = {
  current: { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const },
  supported: encodings.map((sampleFormat) => ({ sampleRate: 48000, channels: 2, sampleFormat })),
};
const formats: NativeDeviceFormats = { render: stream, capture: stream };

function Demo({
  formats,
  ...props
}: Omit<ComponentProps<typeof NativeSampleFormatField>, 'onChange'>) {
  const [confirmed, setConfirmed] = useState(formats);
  useEffect(() => setConfirmed(formats), [formats]);
  return (
    <NativeSampleFormatField
      {...props}
      formats={confirmed}
      onChange={(sampleFormat) =>
        setConfirmed((previous) => ({
          render: {
            ...previous.render,
            current: previous.render.current ? { ...previous.render.current, sampleFormat } : null,
          },
          capture: {
            ...previous.capture,
            current: previous.capture.current
              ? { ...previous.capture.current, sampleFormat }
              : null,
          },
        }))
      }
    />
  );
}

const meta = {
  title: 'Core/Audio Nodes/Bit Depth',
  component: Demo,
  args: {
    formats,
    render: true,
    capture: true,
    label: 'Bit depth',
    unknownLabel: 'Unknown',
    mixedLabel: 'Mixed',
  },
} satisfies Meta<typeof Demo>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Shared: Story = {};
export const Mixed: Story = {
  args: {
    formats: {
      render: stream,
      capture: { ...stream, current: { ...stream.current, sampleFormat: 'signed16' } },
    },
  },
};
export const Unknown: Story = {
  args: { formats: { render: stream, capture: { current: null, supported: [] } } },
};
export const Disabled: Story = { args: { disabled: true } };
export const Rejected: Story = { args: { error: 'The device changed. Refresh and try again.' } };
export const NarrowLongTranslation: Story = {
  args: {
    label: '[Bît dëpth · shârëd îñpût âñd ôûtpût]',
    mixedLabel: '[Mîxëd · ñôt sûñchrôñîzëd]',
  },
  parameters: { viewport: { value: 'mobile' } },
};

const singleFormat = { ...stream, supported: [stream.current] };
/** One confirmed choice is disabled; a mixed readback still allows reconciliation. */
export const SingleConfirmed: Story = {
  args: { formats: { render: singleFormat, capture: singleFormat } },
};
export const SingleMixed: Story = {
  args: {
    formats: {
      render: singleFormat,
      capture: { ...singleFormat, current: { ...stream.current, sampleFormat: 'signed16' } },
    },
  },
};
