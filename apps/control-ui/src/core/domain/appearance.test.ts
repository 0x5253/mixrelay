import { expect, it } from 'vitest';
import { defaultThemePreference, themePreference } from './appearance';

it('keeps an explicit color scheme and follows the system for anything else', () => {
  expect(themePreference('light')).toBe('light');
  expect(themePreference('dark')).toBe('dark');
  for (const value of ['system', 'sepia', null, undefined, 1])
    expect(themePreference(value)).toBe(defaultThemePreference);
  expect(defaultThemePreference).toBe('system');
});
