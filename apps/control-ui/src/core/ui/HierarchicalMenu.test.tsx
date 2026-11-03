import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { MixRelayProvider } from '../theme/MixRelayProvider';
import { HierarchicalContextMenu, HierarchicalMenuButton } from './HierarchicalMenu';
import styles from './HierarchicalMenu.module.css';

const viewport = { width: 800, height: 600 };
const menuSize = { width: 200, height: 120 };

function isMenu(element: HTMLElement) {
  return (
    element.classList.contains(styles.dropdown ?? '') || element.getAttribute('role') === 'menu'
  );
}

const initialViewport = { width: window.innerWidth, height: window.innerHeight };
beforeEach(() => {
  window.innerWidth = viewport.width;
  window.innerHeight = viewport.height;
  // jsdom has no layout. Supply measured DOM geometry; React Aria calculates placement.
  vi.spyOn(document.documentElement, 'clientWidth', 'get').mockReturnValue(viewport.width);
  vi.spyOn(document.documentElement, 'clientHeight', 'get').mockReturnValue(viewport.height);
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return isMenu(this) ? menuSize.height : viewport.height;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return isMenu(this) ? menuSize.width : viewport.width;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return isMenu(this) ? menuSize.height : viewport.height;
  });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.classList.contains(styles.cursorAnchor ?? ''))
      return new DOMRect(
        Number.parseFloat(this.style.left),
        Number.parseFloat(this.style.top),
        0,
        0,
      );
    if (isMenu(this)) {
      return new DOMRect(
        Number.parseFloat(this.style.left) || 0,
        Number.parseFloat(this.style.top) || 0,
        menuSize.width,
        menuSize.height,
      );
    }
    return new DOMRect(0, 0, viewport.width, viewport.height);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  window.innerWidth = initialViewport.width;
  window.innerHeight = initialViewport.height;
});

async function openAt(x: number, y: number) {
  render(
    <MixRelayProvider>
      <HierarchicalContextMenu items={[{ id: 'delete', label: 'Delete' }]} onSelect={vi.fn()}>
        <div data-testid="canvas">Canvas</div>
      </HierarchicalContextMenu>
    </MixRelayProvider>,
  );
  fireEvent.contextMenu(screen.getByTestId('canvas'), { clientX: x, clientY: y });
  const menu = await screen.findByRole('menu');
  if (!menu.parentElement) throw new Error('Menu needs a positioned popover');
  return menu.parentElement;
}

describe('Context menu cursor placement', () => {
  it('starts at the cursor x coordinate, immediately below the cursor', async () => {
    const menu = await openAt(300, 200);
    await waitFor(() => {
      expect(menu).toHaveAttribute('data-placement', 'bottom');
      expect(menu).toHaveStyle({ left: '300px', top: '202px' });
    });
  });

  it('flips above the cursor near the bottom edge', async () => {
    const menu = await openAt(300, 590);
    await waitFor(() => {
      expect(menu).toHaveAttribute('data-placement', 'top');
      expect(menu).toHaveStyle({ left: '300px', bottom: '12px' });
    });
  });

  it('keeps the entire menu inside the bottom-right viewport edge', async () => {
    const menu = await openAt(798, 598);
    await waitFor(() => {
      const left = Number.parseFloat(menu.style.left);
      const top = menu.style.top
        ? Number.parseFloat(menu.style.top)
        : viewport.height - Number.parseFloat(menu.style.bottom) - menuSize.height;
      expect(menu).toHaveAttribute('data-placement', 'top');
      expect(left).toBeGreaterThanOrEqual(0);
      expect(left + menuSize.width).toBeLessThanOrEqual(viewport.width);
      expect(top).toBeGreaterThanOrEqual(0);
      expect(top + menuSize.height).toBeLessThan(598);
    });
  });
});

describe.each(['context', 'button'] as const)('%s nested menu', (entry) => {
  it.each(['mouse', 'keyboard'] as const)(
    'keeps all ancestors open until a leaf is selected with %s',
    async (input) => {
      const onSelect = vi.fn();
      const items = [
        {
          id: 'source',
          label: 'Source',
          children: [
            {
              id: 'virtual',
              label: 'Virtual IO',
              children: [{ id: 'create', label: 'New device' }],
            },
            ...Array.from({ length: 30 }, (_, index) => ({
              id: `app-${index}`,
              label: `Application ${index}`,
            })),
          ],
        },
      ];
      render(
        <MixRelayProvider>
          {entry === 'context' ? (
            <HierarchicalContextMenu items={items} onSelect={onSelect}>
              <div data-testid="canvas">Canvas</div>
            </HierarchicalContextMenu>
          ) : (
            <HierarchicalMenuButton
              items={items}
              onSelect={onSelect}
              label="Add"
              buttonContent="Add"
            />
          )}
        </MixRelayProvider>,
      );
      if (entry === 'context')
        fireEvent.contextMenu(screen.getByTestId('canvas'), { clientX: 300, clientY: 200 });
      else fireEvent.click(screen.getByRole('button', { name: 'Add' }));
      for (const name of ['Source', 'Virtual IO']) {
        const item = await screen.findByRole('menuitem', { name });
        if (input === 'mouse') {
          fireEvent.pointerMove(document.body);
          fireEvent.pointerOver(item);
          await waitFor(() => expect(item).toHaveAttribute('aria-expanded', 'true'));
        } else {
          item.focus();
          fireEvent.keyDown(item, { key: 'ArrowRight' });
        }
      }
      const leaf = await screen.findByRole('menuitem', { name: 'New device' });
      const menus = screen.getAllByRole('menu');
      expect(menus).toHaveLength(3);
      // Each submenu escapes its parent's scroll viewport and stays in the overlay group.
      for (const [index, menu] of menus.entries()) {
        expect(menu).toHaveClass(styles.scrollArea ?? '');
        if (index > 0) expect(menus[index - 1]).not.toContainElement(menu);
      }
      expect(onSelect).not.toHaveBeenCalled();
      if (input === 'mouse') {
        fireEvent.mouseDown(leaf);
        fireEvent.mouseUp(leaf);
        fireEvent.click(leaf);
      } else {
        leaf.focus();
        fireEvent.keyDown(leaf, { key: 'Enter' });
        // jsdom does not synthesize a native button click from keyboard activation.
        fireEvent.click(leaf, { detail: 0 });
      }
      expect(onSelect).toHaveBeenCalledExactlyOnceWith('create');
      await waitFor(() => expect(screen.queryAllByRole('menu')).toHaveLength(0));
    },
  );
});

it('cancels long-press on movement, release and disabling, then executes a stationary touch action', async () => {
  vi.useFakeTimers();
  const selected = vi.fn();
  const started = vi.fn();
  const moved = vi.fn();
  const ended = vi.fn();
  const menu = (disabled = false) => (
    <MixRelayProvider>
      <HierarchicalContextMenu
        disabled={disabled}
        items={[{ id: 'mute', label: 'Mute' }]}
        onSelect={selected}
      >
        <div
          data-testid="touch-canvas"
          onTouchStart={started}
          onTouchMove={moved}
          onTouchEnd={ended}
        >
          Canvas
        </div>
      </HierarchicalContextMenu>
    </MixRelayProvider>
  );
  const view = render(menu());
  const canvas = screen.getByTestId('touch-canvas');
  const press = () => fireEvent.touchStart(canvas, { touches: [{ clientX: 200, clientY: 150 }] });
  try {
    press();
    fireEvent.touchMove(canvas);
    await act(() => vi.advanceTimersByTimeAsync(600));
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    press();
    fireEvent.touchEnd(canvas);
    await act(() => vi.advanceTimersByTimeAsync(600));
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    press();
    view.rerender(menu(true));
    await act(() => vi.advanceTimersByTimeAsync(600));
    fireEvent.contextMenu(canvas);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    view.rerender(menu());
    press();
    await act(() => vi.advanceTimersByTimeAsync(600));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Mute' }));
    expect(selected).toHaveBeenCalledExactlyOnceWith('mute');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(started).toHaveBeenCalledTimes(4);
    expect(moved).toHaveBeenCalledOnce();
    expect(ended).toHaveBeenCalledOnce();
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});

it('shows the reason a disabled menu leaf cannot be selected', async () => {
  const onSelect = vi.fn();
  render(
    <MixRelayProvider>
      <HierarchicalMenuButton
        items={[
          {
            id: 'unavailable',
            label: 'Unavailable input',
            description: 'Device is in use',
            disabled: true,
          },
        ]}
        onSelect={onSelect}
        label="Add"
        buttonContent="Add"
      />
    </MixRelayProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Add' }));
  const item = await screen.findByRole('menuitem', { name: /Unavailable input/ });
  expect(item).toHaveAttribute('aria-disabled', 'true');
  expect(item).toHaveAttribute('data-disabled', 'true');
  expect(screen.getByText('Device is in use')).toBeInTheDocument();
  fireEvent.click(item);
  expect(onSelect).not.toHaveBeenCalled();
});

it('uses the owner anchor supplied for a coordinate-less accessibility invocation', async () => {
  render(
    <MixRelayProvider>
      <HierarchicalContextMenu
        getAnchor={() => ({ x: 400, y: 250 })}
        items={[{ id: 'rename', label: 'Rename display name' }]}
        onSelect={vi.fn()}
      >
        <section aria-label="Graph">
          <fieldset aria-label="Compressor" />
        </section>
      </HierarchicalContextMenu>
    </MixRelayProvider>,
  );
  fireEvent.contextMenu(screen.getByRole('group', { name: 'Compressor' }));
  const menu = await screen.findByRole('menu');
  await waitFor(() => expect(menu.parentElement).toHaveStyle({ left: '400px', top: '252px' }));
});
