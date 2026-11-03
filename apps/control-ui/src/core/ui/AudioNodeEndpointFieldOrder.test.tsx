import { fireEvent, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';

it('keeps visual and keyboard field order in the node, with fixed capabilities disabled', () => {
  const format = { sampleRate: 48000, channels: 2, sampleFormat: 'float32' as const };
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Device"
        icon="IO"
        title="Monitor"
        inputCount={2}
        outputCount={0}
        onEndpointChange={vi.fn()}
        endpointControl={{
          displayName: 'Monitor',
          nameLabel: 'Device name',
          canRename: false,
          renderChannels: 2,
          captureChannels: 2,
          channelsLabel: 'Channels',
          allowedRenderChannels: [2],
          allowedCaptureChannels: [2],
          observedRate: { label: 'Sample rate', value: 48000 },
          sampleRateOptions: [48000],
          nativeFormats: {
            render: { current: format, supported: [format] },
            capture: { current: format, supported: [format] },
          },
          nativeFormatLabels: { bitDepth: 'Bit depth', unknown: 'Unknown' },
        }}
      />
    </MixRelayProvider>,
  );
  fireEvent.click(view.getByRole('button', { name: (name) => name.endsWith('Options') }));
  const name = view.getByRole('textbox', { name: 'Device name' });
  const channels = view.getByRole('button', { name: /Channels$/ });
  const rate = view.getByRole('button', { name: /Sample rate$/ });
  const depth = view.getByRole('button', { name: /Bit depth$/ });
  const fields = [name, channels, rate, depth];
  for (const [index, field] of fields.entries()) {
    const next = fields[index + 1];
    if (next)
      expect(field.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  }
  expect(name).toHaveAttribute('readonly');
  expect(channels).toBeDisabled();
  expect(rate).toBeDisabled();
  expect(depth).toBeDisabled();
});
