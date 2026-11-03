import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
// Core components resolve accessible names through the app's i18n instance, as at runtime.
import './core/i18n';

// Testing Library detects fake timers through this Jest-shaped boundary, including
// the zero-delay microtask drain after user-event. Without it, awaiting an input
// under Vitest's fake clock hangs even when user-event has advanceTimers configured.
Object.assign(globalThis, { jest: { advanceTimersByTime: vi.advanceTimersByTime } });

afterEach(cleanup);

// React Aria reports broken component contracts only as console warnings. Fail the test
// instead, so a tooltip trigger without a role or a controlled/uncontrolled switch cannot
// ship silently.
const ariaContract = /<Focusable> child|changed from (?:un)?controlled to (?:un)?controlled/;
const ariaViolations: string[] = [];
for (const level of ['warn', 'error'] as const) {
  const original = console[level].bind(console);
  console[level] = (...args: unknown[]) => {
    const message = args.map(String).join(' ');
    if (ariaContract.test(message)) ariaViolations.push(message);
    original(...args);
  };
}
afterEach(() => {
  if (ariaViolations.length === 0) return;
  const found = ariaViolations.splice(0);
  throw new Error(`React Aria contract warnings:\n${found.join('\n')}`);
});

Object.defineProperty(window, 'matchMedia', {
  configurable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    addEventListener: vi.fn(),
    addListener: vi.fn(),
    dispatchEvent: vi.fn(),
    matches: false,
    media: query,
    onchange: null,
    removeEventListener: vi.fn(),
    removeListener: vi.fn(),
  })),
});

class TestResizeObserver implements ResizeObserver {
  public disconnect() {}
  public observe() {}
  public unobserve() {}
}

globalThis.ResizeObserver = TestResizeObserver;

// jsdom has no layout/scrolling engine. Combobox calls this browser API when
// highlighting an option; model only that platform boundary, not selection.
Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
  configurable: true,
  value: vi.fn(),
});

// jsdom lacks PointerEvent. Model browser event fields, keeping React Aria handlers real.
class TestPointerEvent extends MouseEvent {
  readonly pointerType: string;
  readonly pointerId: number;
  readonly isPrimary: boolean;
  constructor(type: string, init: PointerEventInit = {}) {
    super(type, init);
    this.pointerType = init.pointerType ?? 'mouse';
    this.pointerId = init.pointerId ?? 1;
    this.isPrimary = init.isPrimary ?? true;
  }
}
Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: TestPointerEvent });

Object.defineProperty(window, 'PointerEvent', { configurable: true, value: TestPointerEvent });

// jsdom has no DOMMatrixReadOnly. React Flow reads the viewport zoom (m22) from it, as in
// React Flow's own jsdom testing guide; only the scale component is modelled.
class TestDOMMatrixReadOnly {
  readonly m22: number;
  constructor(transform?: string) {
    const scale = transform?.match(/scale\(([\d.]+)\)/u)?.[1];
    this.m22 = scale === undefined ? 1 : Number(scale);
  }
}
Object.defineProperty(window, 'DOMMatrixReadOnly', {
  configurable: true,
  value: TestDOMMatrixReadOnly,
});
