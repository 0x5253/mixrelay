import {
  defaultThemePreference,
  explicitColorSchemes,
  themeStorageKey,
} from '../domain/appearance';
import { themeCss } from './theme';

/** Runs before first paint: applies the saved preference so a choice that differs from
 * the OS appearance never flashes the OS theme while the bundle loads. It runs before any
 * module loads, so it inlines the same key, accepted values and fallback as the store. */
export const themeBootstrapScript = `(()=>{let t=${JSON.stringify(defaultThemePreference)};try{const v=localStorage.getItem(${JSON.stringify(
  themeStorageKey,
)});if(${JSON.stringify(explicitColorSchemes)}.includes(v))t=v}catch{}document.documentElement.setAttribute('data-theme',t)})()`;

/** Tags for the built document <head>: the token CSS and the bootstrap, so the first
 * paint already has the theme. MixRelayProvider keeps owning later changes. */
export function themeDocumentTags() {
  return [
    {
      tag: 'style',
      attrs: { 'data-mixrelay-theme': 'document' },
      children: themeCss,
      injectTo: 'head-prepend' as const,
    },
    { tag: 'script', children: themeBootstrapScript, injectTo: 'head-prepend' as const },
  ];
}
