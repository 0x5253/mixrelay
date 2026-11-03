import { afterEach, expect, it, vi } from 'vitest';
import { themeStorageKey } from '../domain/appearance';
import { themeCss } from './theme';
import { themeBootstrapScript, themeDocumentTags } from './themeDocument';

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
});

it('puts the token CSS and the bootstrap at the start of the built document head', () => {
  const tags = themeDocumentTags();
  expect(tags.map((tag) => [tag.tag, tag.injectTo])).toEqual([
    ['style', 'head-prepend'],
    ['script', 'head-prepend'],
  ]);
  expect(tags[0]?.children).toBe(themeCss);
  expect(tags[1]?.children).toBe(themeBootstrapScript);
});

it.each([
  ['dark', 'dark'],
  ['light', 'light'],
  ['system', 'system'],
  ['sepia', 'system'],
  [null, 'system'],
])('applies a saved %s preference as %s before the app mounts', (saved, applied) => {
  if (saved !== null) localStorage.setItem(themeStorageKey, saved);
  new Function(themeBootstrapScript)();
  expect(document.documentElement.getAttribute('data-theme')).toBe(applied);
});

it('falls back to the system appearance when storage is blocked', () => {
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  new Function(themeBootstrapScript)();
  expect(document.documentElement.getAttribute('data-theme')).toBe('system');
});
