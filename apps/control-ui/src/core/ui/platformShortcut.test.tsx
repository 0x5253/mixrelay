import { afterEach, describe, expect, it } from 'vitest';

import {
  formatPlatformShortcut,
  hasPlatformModifier,
  isNativeTextEntryTarget,
  platformModifier,
} from './platformShortcut';

const originalPlatform = Object.getOwnPropertyDescriptor(navigator, 'platform');
afterEach(() => {
  Reflect.deleteProperty(navigator, 'platform');
  if (originalPlatform) Object.defineProperty(navigator, 'platform', originalPlatform);
});

describe('platform shortcut compatibility', () => {
  it.each(['MacIntel', 'iPhone', 'iPad', 'iPod'])(
    'uses the primary Apple modifier on %s',
    (platform) => {
      Object.defineProperty(navigator, 'platform', { configurable: true, value: platform });
      expect(platformModifier()).toBe('Meta');
      expect(formatPlatformShortcut('Mod+Shift+Z')).toBe('⌘Shift+Z');
      expect(hasPlatformModifier({ metaKey: true, ctrlKey: false })).toBe(true);
      expect(hasPlatformModifier({ metaKey: false, ctrlKey: true })).toBe(false);
      expect(hasPlatformModifier({ metaKey: true, ctrlKey: true })).toBe(false);
    },
  );

  it.each(['Win32', 'Linux x86_64'])(
    'uses Control on %s without accepting both modifiers',
    (platform) => {
      expect(platformModifier(platform)).toBe('Control');
      Object.defineProperty(navigator, 'platform', { configurable: true, value: platform });
      expect(formatPlatformShortcut('Mod+Z')).toBe('Ctrl+Z');
      expect(hasPlatformModifier({ metaKey: false, ctrlKey: true })).toBe(true);
      expect(hasPlatformModifier({ metaKey: true, ctrlKey: true })).toBe(false);
    },
  );
});

describe('native text editing ownership', () => {
  it('accepts text inputs and textareas but leaves buttons, checkboxes and non-elements to app shortcuts', () => {
    const input = document.createElement('input');
    expect(isNativeTextEntryTarget(input)).toBe(true);
    input.type = 'checkbox';
    expect(isNativeTextEntryTarget(input)).toBe(false);
    expect(isNativeTextEntryTarget(document.createElement('textarea'))).toBe(true);
    expect(isNativeTextEntryTarget(document.createElement('button'))).toBe(false);
    expect(isNativeTextEntryTarget(null)).toBe(false);
    expect(isNativeTextEntryTarget(window)).toBe(false);
  });

  it.each(['', 'true', 'plaintext-only'])(
    'preserves shortcuts in nested editable containers (%s)',
    (editable) => {
      const parent = document.createElement('div');
      const child = document.createElement('span');
      parent.setAttribute('contenteditable', editable);
      parent.append(child);
      expect(isNativeTextEntryTarget(child)).toBe(true);
      child.setAttribute('contenteditable', 'false');
      expect(isNativeTextEntryTarget(child)).toBe(false);
    },
  );

  it('honors the browser isContentEditable property', () => {
    const element = document.createElement('div');
    Object.defineProperty(element, 'isContentEditable', { value: true });
    expect(isNativeTextEntryTarget(element)).toBe(true);
  });
});
