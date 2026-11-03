import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, assert, expect, it, vi } from 'vitest';
import { reconcileAudioFlowNodes } from '../audio-graph/reactFlowAdapter';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { AudioNodeCard } from './AudioNodeCard';
import { DeviceAppActivity, type DeviceAppActivityProps } from './DeviceAppActivity';

const activity: DeviceAppActivityProps = {
  direction: 'playing',
  apps: ['Safari', 'Music', 'Firefox', 'QuickTime', 'MixRelay'].map((name) => ({
    id: name,
    label: `${name} — playing to Device`,
    iconUrl: name === 'Safari' ? 'data:image/png;base64,real' : undefined,
  })),
  label: 'No apps',
  summary: 'Apps playing to Device: Safari, Music, Firefox, QuickTime, MixRelay',
  overflowLabel: '+2',
  overflowDescription: 'Additional apps playing to Device (2): QuickTime, MixRelay',
};
const card = (value: DeviceAppActivityProps, onSelect?: () => void) => (
  <MixRelayProvider>
    <AudioNodeCard
      title="Device"
      category="Device"
      icon="ADC"
      inputCount={0}
      outputCount={2}
      deviceAppActivity={value}
      onSelect={onSelect}
    />
  </MixRelayProvider>
);
afterEach(() => vi.useRealTimers());

it('shows at most three original icons with fallback and overflow, never adds a Tab stop', () => {
  const view = render(card(activity));
  const group = screen.getByRole('img', { name: activity.summary });
  expect(group.querySelectorAll('img')).toHaveLength(1);
  expect(group.querySelector('img')).toHaveAttribute('src', activity.apps[0]?.iconUrl);
  expect(group.querySelectorAll('svg')).toHaveLength(3); // Direction and two missing-icon fallbacks.
  expect(screen.getByText('+2')).toBeInTheDocument();
  expect(view.container.querySelectorAll('button, [tabindex="0"]')).toHaveLength(0);
  const icon = group.querySelector('img');
  assert(icon);
  fireEvent.error(icon);
  expect(group.querySelectorAll('svg')).toHaveLength(4);
  expect(screen.getByText('2 CH')).toBeVisible();
});

it('exposes the full list on the outer node description and keyboard focus tooltip, preserving the single selection owner', async () => {
  vi.useFakeTimers();
  const node = {
    id: 'device',
    position: { x: 0, y: 0 },
    inputPorts: [],
    outputPorts: [],
    appearance: { title: 'Device', category: 'Device', icon: 'ADC', deviceAppActivity: activity },
  };
  const [flow] = reconcileAudioFlowNodes([], { nodes: [node], edges: [], revision: 1 });
  assert(flow);
  const view = render(
    <>
      <span id="node-instructions">Node keyboard instructions.</span>
      <button
        type="button"
        className="react-flow__node"
        tabIndex={0}
        aria-label={flow.ariaLabel}
        {...flow.domAttributes}
        aria-describedby="node-instructions"
      >
        {card(activity)}
      </button>
    </>,
  );
  const owner = screen.getByRole('button', { name: 'Device' });
  expect(owner).toHaveAccessibleDescription(`Node keyboard instructions. ${activity.summary}`);
  act(() => owner.focus());
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400);
  });
  expect(screen.getByRole('tooltip')).toHaveTextContent(activity.summary);
  expect(view.container.querySelectorAll('[tabindex="0"]')).toHaveLength(1);
  act(() => owner.blur());
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400);
  });
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  view.unmount();
  expect(owner).toHaveAttribute('aria-describedby', 'node-instructions');
});

it('lets Escape dismiss the focus tooltip so the next Escape reaches the graph', async () => {
  vi.useFakeTimers();
  const graphEscape = vi.fn();
  const tree = () => (
    // biome-ignore lint/a11y/noStaticElementInteractions: stands in for the graph's key handler.
    <div
      onKeyDown={(event) => {
        if (event.key === 'Escape') graphEscape();
      }}
    >
      <button type="button" className="react-flow__node" aria-label="Device">
        <MixRelayProvider>
          <DeviceAppActivity {...activity} />
        </MixRelayProvider>
      </button>
    </div>
  );
  const view = render(tree());
  const owner = screen.getByRole('button', { name: 'Device' });
  act(() => owner.focus());
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400);
  });
  expect(screen.getByRole('tooltip')).toBeInTheDocument();
  fireEvent.keyDown(owner, { key: 'Escape' });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(400);
  });
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  fireEvent.keyDown(owner, { key: 'Escape' });
  expect(graphEscape).toHaveBeenCalled();
  // An owner that rewrites its description keeps the activity summary after a render.
  owner.setAttribute('aria-describedby', 'other');
  view.rerender(tree());
  expect(owner.getAttribute('aria-describedby')?.split(' ')).toHaveLength(2);
  expect(owner).toHaveAccessibleDescription(activity.summary);
});

it('renders empty/checking/unavailable text and both directions without inventing active icons', async () => {
  const empty = {
    ...activity,
    apps: [],
    direction: 'recording' as const,
    summary: 'No apps recording from Device',
  };
  const view = render(card(empty, vi.fn()));
  const button = screen.getByRole('button', { name: 'Device, Device' });
  expect(button).toHaveAccessibleDescription(empty.summary);
  expect(screen.getByText('No apps')).toBeVisible();
  expect(screen.queryByText('+2')).not.toBeInTheDocument();
  expect(screen.getByRole('img').querySelectorAll('svg')).toHaveLength(1);
  view.rerender(card({ ...empty, label: 'Checking apps…', summary: 'Checking apps…' }, vi.fn()));
  expect(screen.getByRole('img', { name: 'Checking apps…' })).toHaveTextContent('Checking apps…');
  view.rerender(
    card(
      { ...empty, label: 'App activity unavailable', summary: 'App activity unavailable' },
      vi.fn(),
    ),
  );
  expect(screen.getByRole('img', { name: 'App activity unavailable' })).toHaveTextContent(
    'App activity unavailable',
  );
});

it('supports a readonly standalone group without a focus owner', () => {
  render(
    <MixRelayProvider>
      <DeviceAppActivity {...activity} apps={activity.apps.slice(0, 1)} />
    </MixRelayProvider>,
  );
  expect(screen.getByRole('img')).toHaveAccessibleName(activity.summary);
  expect(screen.queryByText('+2')).toBeNull();
});
