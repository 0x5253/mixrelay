import { fireEvent, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { NodeMuteButton, NodePowerButton } from './NodeSignalControls';

it('exposes a power button and independent mute without native tooltips', () => {
  const onPower = vi.fn();
  const onMute = vi.fn();
  const view = render(
    <>
      <NodePowerButton enabled disabled={false} label="Disable Gain" onChange={onPower} />
      <NodeMuteButton muted={false} disabled={false} label="Mute Gain" onChange={onMute} />
    </>,
  );
  const power = view.getByRole('button', { name: 'Disable Gain' });
  const mute = view.getByRole('button', { name: 'Mute Gain' });
  expect(power).toHaveAttribute('aria-pressed', 'true');
  expect(mute).toHaveAttribute('aria-pressed', 'false');
  expect(view.container.querySelector('[title]')).toBeNull();
  fireEvent.click(power);
  fireEvent.click(mute);
  expect(onPower).toHaveBeenCalledTimes(1);
  expect(onMute).toHaveBeenCalledTimes(1);
});

it('prevents clicks while a command is awaiting acknowledgement', () => {
  const onPower = vi.fn();
  const view = render(
    <NodePowerButton enabled={false} disabled label="Enable Gain" onChange={onPower} />,
  );
  fireEvent.click(view.getByRole('button', { name: 'Enable Gain' }));
  expect(onPower).not.toHaveBeenCalled();
});

it('shows a muted node independently of power and restores its owner through the mute action', () => {
  const onMute = vi.fn();
  const view = render(
    <NodeMuteButton muted disabled={false} label="Unmute Gain" onChange={onMute} />,
  );
  const mute = view.getByRole('button', { name: 'Unmute Gain' });
  expect(mute).toHaveAttribute('aria-pressed', 'true');
  fireEvent.click(mute);
  expect(onMute).toHaveBeenCalledOnce();
});
