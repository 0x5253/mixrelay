import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { HierarchicalContextMenu, HierarchicalMenuButton } from './HierarchicalMenu';

afterEach(() => vi.useRealTimers());

describe('hierarchical menu boundary behavior', () => {
  it('keeps a tooltip-wrapped trigger and an actual application icon while preserving the controlled open callback', async () => {
    const changed = vi.fn();
    render(
      <MixRelayProvider>
        <HierarchicalMenuButton
          label="Add source"
          buttonContent="Add"
          tooltip="Choose an audio source"
          onOpenedChange={changed}
          items={[{ id: 'firefox', label: 'Firefox', iconUrl: 'data:image/png;base64,aA==' }]}
          onSelect={vi.fn()}
        />
      </MixRelayProvider>,
    );
    const trigger = screen.getByRole('button', { name: 'Add source' });
    fireEvent.click(trigger);
    expect(await screen.findByRole('menuitem', { name: 'Firefox' })).toContainElement(
      document.querySelector('img'),
    );
    expect(document.querySelector('img')).toHaveAttribute('src', 'data:image/png;base64,aA==');
    expect(changed).toHaveBeenCalledWith(true);
  });

  it('ignores empty and disabled touches without opening a context menu', async () => {
    vi.useFakeTimers();
    const changed = vi.fn();
    render(
      <MixRelayProvider>
        <HierarchicalContextMenu
          disabled
          onOpenedChange={changed}
          items={[{ id: 'remove', label: 'Remove' }]}
          onSelect={vi.fn()}
        >
          <div data-testid="surface">Surface</div>
        </HierarchicalContextMenu>
      </MixRelayProvider>,
    );
    fireEvent.touchStart(screen.getByTestId('surface'), { touches: [] });
    await act(() => vi.advanceTimersByTimeAsync(600));
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    fireEvent.touchStart(screen.getByTestId('surface'), {
      touches: [{ clientX: 12, clientY: 20 }],
    });
    await act(() => vi.advanceTimersByTimeAsync(600));
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(changed).not.toHaveBeenCalled();
  });
});
