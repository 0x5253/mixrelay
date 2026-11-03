import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { AudioNodeCard } from '../../core/ui/AudioNodeCard';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

it('changes both directions together using advertised common channel counts through 64', () => {
  const onChange = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        category="Device"
        icon="IO"
        title={virtualSpec.displayName}
        inputCount={16}
        outputCount={64}
        onSelect={vi.fn()}
        optionsLabel="Options"
        onEndpointChange={onChange}
        endpointControl={{
          displayName: virtualSpec.displayName,
          nameLabel: 'Device name',
          renderChannels: 64,
          captureChannels: 16,
          allowedRenderChannels: virtualPolicy.renderChannels,
          allowedCaptureChannels: virtualPolicy.captureChannels,
        }}
      />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
  fireEvent.click(screen.getByRole('button', { name: /Channels$/ }));
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(
    virtualPolicy.renderChannels.map(String),
  );
  expect(screen.queryByRole('option', { name: '65' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('option', { name: '32' }));
  expect(onChange).toHaveBeenCalledWith({
    displayName: virtualSpec.displayName,
    renderChannels: 32,
    captureChannels: 32,
  });
});
