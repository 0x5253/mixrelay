import type { PropsWithChildren } from 'react';
import { vi } from 'vitest';
import type { NativeShellPort } from '../core/ports/NativeShellPort';
import { NativeShellContext } from '../core/shell/NativeShellContext';

/** A desktop shell whose menu events the test fires and whose listener registrations the
 * test can hold open, to reproduce registration finishing after the owner went away. */
export function fakeNativeShell(version = '1.2.3') {
  const listeners = { about: new Set<() => void>(), graphList: new Set<() => void>() };
  const removals = { about: vi.fn(), graphList: vi.fn() };
  let held: PromiseWithResolvers<void> | undefined;
  const register = (event: keyof typeof listeners, listener: () => void) => {
    const removal = () => {
      listeners[event].delete(listener);
      removals[event]();
    };
    const registration = held?.promise ?? Promise.resolve();
    return registration.then(() => {
      listeners[event].add(listener);
      return removal;
    });
  };
  const shell = {
    version: vi.fn(async () => version),
    onAbout: vi.fn((listener: () => void) => register('about', listener)),
    onGraphListToggle: vi.fn((listener: () => void) => register('graphList', listener)),
    setGraphListVisible: vi.fn(async (_visible: boolean) => undefined),
    setMenuLocale: vi.fn(async (_locale: string) => undefined),
    setWindowTheme: vi.fn(async () => undefined),
  } satisfies NativeShellPort;
  return {
    shell,
    removals,
    fire: (event: keyof typeof listeners) => {
      for (const listener of listeners[event]) listener();
    },
    listening: (event: keyof typeof listeners) => listeners[event].size,
    /** Registrations made from now on wait until `release` is called. */
    hold: () => {
      held = Promise.withResolvers<void>();
      return () => held?.resolve();
    },
    Provider: ({ children }: PropsWithChildren) => (
      <NativeShellContext.Provider value={shell}>{children}</NativeShellContext.Provider>
    ),
  };
}
