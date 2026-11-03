import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard, type AudioNodeCardProps } from './AudioNodeCard';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';

function node(props: Partial<AudioNodeCardProps> = {}) {
  const onPower = vi.fn();
  const onMute = vi.fn();
  render(
    <MixRelayProvider>
      <AudioNodeCard
        title="Gain"
        category="Processor"
        icon="GAIN"
        inputCount={2}
        outputCount={2}
        onSelect={vi.fn()}
        poweredOff
        headerActions={
          <NodePowerButton
            enabled={false}
            disabled={false}
            label="Enable Gain"
            onChange={onPower}
          />
        }
        muteControl={
          <NodeMuteButton muted={false} disabled={false} label="Mute Gain" onChange={onMute} />
        }
        {...props}
      />
    </MixRelayProvider>,
  );
  return { onPower, onMute };
}

it('keeps header power and volume mute independent of Options and usable while OFF', () => {
  const { onPower, onMute } = node({
    gainControl: { label: 'Level', valueDb: 0 },
    onGainChange: vi.fn(),
  });
  const options = screen.getByRole('button', { name: (name) => name.endsWith('Options') });
  const power = screen.getByRole('button', { name: 'Enable Gain' });
  const mute = screen.getByRole('button', { name: 'Mute Gain' });
  expect(options.parentElement?.parentElement).toContainElement(power);
  expect(options).not.toContainElement(power);
  expect(power.closest('[data-header-actions]')).toBeInTheDocument();
  expect(power.closest('button')).toBe(power);
  expect(power.parentElement?.closest('button')).toBeNull();
  expect(power.closest('[inert]')).toBeNull();
  fireEvent.click(power);
  expect(onPower).toHaveBeenCalledOnce();
  expect(options).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(options);
  expect(options).not.toContainElement(mute);
  expect(mute.parentElement).not.toContainElement(screen.getByRole('slider'));
  expect(screen.getByRole('slider')).toBeDisabled();
  expect(mute.closest('[inert]')).toBeNull();
  fireEvent.click(mute);
  expect(onMute).toHaveBeenCalledOnce();
  expect(power.closest('[inert]')).toBeNull();
  fireEvent.click(options);
  expect(screen.getAllByRole('button', { name: 'Enable Gain' })).toHaveLength(1);
});

it('provides controls without a meaningless disclosure when the node has no settings', () => {
  const { onPower } = node({ faceMode: 'compact' });
  expect(screen.queryByRole('button', { name: (name) => name.endsWith('Options') })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Enable Gain' }));
  expect(onPower).toHaveBeenCalledOnce();
});

it('shows generic plugin parameters directly and leaves power outside their disabled panel', () => {
  node({
    faceMode: 'compact',
    parameterControls: [
      { id: 'wet', label: 'Wet', minimum: 0, maximum: 1, step: 0.01, suffix: '', value: 0.5 },
    ],
    onParameterChange: vi.fn(),
  });
  expect(screen.queryByRole('button', { name: (name) => name.endsWith('Options') })).toBeNull();
  expect(screen.getByRole('slider').closest('[inert]')).not.toBeNull();
  expect(screen.getByRole('button', { name: 'Enable Gain' }).closest('[inert]')).toBeNull();
});
