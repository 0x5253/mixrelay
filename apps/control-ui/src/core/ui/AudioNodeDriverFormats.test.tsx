import { fireEvent, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { NativeDeviceFormats, NativeSampleFormat } from '../domain/control/nativeFormat';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';
import type { EndpointControl } from './AudioNodeEndpointOptions';

const stream = {
  current: { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const },
  supported: [
    { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const },
    { sampleRate: 48000, channels: 2, sampleFormat: 'signed32' as const },
    { sampleRate: 44100, channels: 2, sampleFormat: 'signed24' as const },
  ],
};
const formats: NativeDeviceFormats = { render: stream, capture: stream };
const control: EndpointControl = {
  displayName: 'Monitor',
  nameLabel: 'Device name',
  renderChannels: 2,
  captureChannels: 2,
  nativeFormats: formats,
  nativeFormatLabels: { bitDepth: 'Bit depth', unknown: 'Unknown', mixed: 'Mixed' },
};
function card(endpointControl: EndpointControl, onEndpointChange = vi.fn()) {
  return (
    <MixRelayProvider>
      <AudioNodeCard
        category="Device"
        icon="IO"
        title="Monitor"
        inputCount={2}
        outputCount={2}
        poweredOff
        endpointControl={endpointControl}
        onEndpointChange={onEndpointChange}
        onSelect={vi.fn()}
      />
    </MixRelayProvider>
  );
}
it('offers a shared bit depth off graph and retains readback and options after an error', () => {
  const onEndpointChange = vi.fn();
  const view = render(card(control, onEndpointChange));
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  fireEvent.click(view.getByRole('button', { name: /Bit depth$/ }));
  expect(view.getAllByRole('option').map((option) => option.textContent)).toEqual([
    '32-bit PCM',
    '32-bit Float',
  ]);
  fireEvent.click(view.getByRole('option', { name: '32-bit PCM' }));
  expect(onEndpointChange).toHaveBeenCalledExactlyOnceWith({
    displayName: 'Monitor',
    renderChannels: 2,
    captureChannels: 2,
    sampleFormat: 'signed32',
  });
  view.rerender(card({ ...control, formatError: 'Refresh and try again.' }, onEndpointChange));
  expect(view.getByRole('button', { name: /Bit depth$/ })).toHaveTextContent('32-bit Float');
  expect(view.getByRole('alert')).toHaveTextContent('Refresh and try again.');
  expect(view.getByRole('button', { name: (name) => name.endsWith('Options') })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
});
it('does not invent a common bit depth when an active direction has no readback', () => {
  const view = render(
    card({
      ...control,
      nativeFormats: { render: stream, capture: { current: null, supported: [] } },
    }),
  );
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  const field = view.getByRole('button', { name: /Bit depth$/ });
  expect(field).toBeDisabled();
  expect(field).toHaveTextContent('Unknown');
  expect(field).toHaveTextContent('Unknown');
});

it('offers all four driver encodings and selects 16-bit without changing rate or channels', () => {
  const encodings: NativeSampleFormat[] = ['signed16', 'signed24', 'signed32', 'float32'];
  const nativeStream = {
    current: stream.current,
    supported: encodings.map((sampleFormat) => ({ ...stream.current, sampleFormat })),
  };
  const change = vi.fn();
  const view = render(
    card({ ...control, nativeFormats: { render: nativeStream, capture: nativeStream } }, change),
  );
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  fireEvent.click(view.getByRole('button', { name: /Bit depth$/ }));
  expect(view.getAllByRole('option').map((option) => option.textContent)).toEqual([
    '16-bit PCM',
    '24-bit PCM',
    '32-bit PCM',
    '32-bit Float',
  ]);
  fireEvent.click(view.getByRole('option', { name: '16-bit PCM' }));
  expect(change).toHaveBeenCalledExactlyOnceWith({
    displayName: 'Monitor',
    renderChannels: 2,
    captureChannels: 2,
    sampleFormat: 'signed16',
  });
  expect(view.getByRole('button', { name: /Bit depth$/ })).toHaveTextContent('32-bit Float');
});
