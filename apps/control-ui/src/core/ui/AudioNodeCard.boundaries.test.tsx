import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard, type AudioNodeCardProps } from './AudioNodeCard';
import type { EndpointControl } from './AudioNodeEndpointOptions';

function fixture(props: Partial<AudioNodeCardProps> = {}) {
  const onEndpointChange = vi.fn();
  const view = render(
    <MixRelayProvider>
      <AudioNodeCard
        title="MixRelay IO"
        category="Device"
        icon="IO"
        inputCount={2}
        outputCount={2}
        onEndpointChange={onEndpointChange}
        {...props}
      />
    </MixRelayProvider>,
  );
  return { ...view, onEndpointChange };
}

function endpoint(overrides: Partial<EndpointControl> = {}): EndpointControl {
  return {
    displayName: 'MixRelay IO',
    nameLabel: 'Device name',
    renderChannels: 2,
    captureChannels: 2,
    ...overrides,
  };
}

function openOptions() {
  fireEvent.click(screen.getByRole('button', { name: (name) => name.endsWith('Options') }));
}

describe('native endpoint options boundaries', () => {
  it('rejects a blank name and commits a trimmed changed name on Enter', () => {
    const { onEndpointChange } = fixture({ endpointControl: endpoint() });
    openOptions();
    const name = screen.getByRole('textbox', { name: 'Device name' });
    fireEvent.change(name, { target: { value: '  ' } });
    fireEvent.blur(name);
    expect(name).toHaveValue('MixRelay IO');
    expect(onEndpointChange).not.toHaveBeenCalled();
    fireEvent.change(name, { target: { value: '  Monitoring  ' } });
    name.focus();
    fireEvent.keyDown(name, { key: 'Enter' });
    expect(onEndpointChange).toHaveBeenCalledExactlyOnceWith({
      displayName: 'Monitoring',
      captureChannels: 2,
      renderChannels: 2,
    });
  });

  it('keeps names read-only when renaming is unavailable', () => {
    const { onEndpointChange } = fixture({ endpointControl: endpoint({ canRename: false }) });
    openOptions();
    const name = screen.getByRole('textbox', { name: 'Device name' });
    expect(name).toHaveAttribute('readonly');
    fireEvent.blur(name);
    expect(onEndpointChange).not.toHaveBeenCalled();
  });

  it.each(['render', 'capture'] as const)(
    'changes channel counts on a %s-only device without adding the other direction',
    async (direction) => {
      const { onEndpointChange } = fixture({
        endpointControl: endpoint({
          renderChannels: direction === 'render' ? 2 : 0,
          captureChannels: direction === 'capture' ? 2 : 0,
        }),
      });
      openOptions();
      fireEvent.click(screen.getByRole('button', { name: /Channels/ }));
      fireEvent.click(await screen.findByRole('option', { name: /^4$/ }));
      expect(onEndpointChange).toHaveBeenCalledExactlyOnceWith({
        displayName: 'MixRelay IO',
        renderChannels: direction === 'render' ? 4 : 0,
        captureChannels: direction === 'capture' ? 4 : 0,
      });
    },
  );
});

describe('processor and meter presentation boundaries', () => {
  it('sends generic integer parameter changes through the owner callback', () => {
    const onParameterChange = vi.fn();
    fixture({
      faceMode: 'compact',
      onParameterChange,
      parameterControls: [
        {
          id: 'ratio',
          label: 'Ratio',
          minimum: 1,
          maximum: 10,
          step: 1,
          suffix: ':1',
          value: 3,
        },
      ],
    });
    const slider = screen.getByRole('slider', { name: 'Ratio' });
    slider.focus();
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    expect(onParameterChange).toHaveBeenCalledExactlyOnceWith('ratio', 4);
    expect(slider).toHaveAttribute('aria-valuetext', '4 :1');
  });

  it('keeps compact native content and a zero-channel meter bank from exposing a fictitious meter', () => {
    fixture({
      icon: 'DAC',
      faceMode: 'compact',
      meterBanks: [{ nodeId: 'node', portId: 'out', channels: 0 }],
      content: <div>Native editor</div>,
    });
    expect(screen.getByText('Native editor')).toBeInTheDocument();
    expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  });
});

it('does not rename a device when its unchanged name loses focus or receives an ordinary key', () => {
  const { onEndpointChange } = fixture({ endpointControl: endpoint() });
  openOptions();
  const name = screen.getByRole('textbox', { name: 'Device name' });
  name.focus();
  fireEvent.keyDown(name, { key: 'ArrowLeft' });
  expect(name).toHaveFocus();
  fireEvent.blur(name);
  expect(onEndpointChange).not.toHaveBeenCalled();
});

it('does not apply an observed channel count that the device no longer advertises as configurable', async () => {
  const { onEndpointChange } = fixture({
    endpointControl: endpoint({
      allowedCaptureChannels: [4, 8],
      allowedRenderChannels: [4, 8],
    }),
  });
  openOptions();
  fireEvent.click(screen.getByRole('button', { name: /Channels/ }));
  fireEvent.click(await screen.findByRole('option', { name: /^2$/ }));
  expect(onEndpointChange).not.toHaveBeenCalled();
});
