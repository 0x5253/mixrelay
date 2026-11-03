/** Use the same platform identity for displayed shortcuts and canvas gestures. */
export function platformModifier(platform = navigator.platform): 'Meta' | 'Control' {
  return /Mac|iPhone|iPad|iPod/i.test(platform) ? 'Meta' : 'Control';
}

/** Only the current platform's primary modifier invokes application shortcuts. */
export function hasPlatformModifier(event: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey'>): boolean {
  return platformModifier() === 'Meta'
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
}

/** Preserve native editing shortcuts, including descendants of editable containers. */
export function isNativeTextEntryTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLTextAreaElement) return true;
  if (
    target instanceof HTMLInputElement &&
    ['email', 'number', 'password', 'search', 'tel', 'text', 'url'].includes(target.type)
  ) {
    return true;
  }
  for (let element: HTMLElement | null = target; element; element = element.parentElement) {
    const editable = element.getAttribute('contenteditable');
    if (editable === 'false') return false;
    if (
      element.isContentEditable ||
      editable === '' ||
      editable === 'true' ||
      editable === 'plaintext-only'
    ) {
      return true;
    }
  }
  return false;
}

/** `Mod` is the primary application modifier, not a literal platform key. */
export function formatPlatformShortcut(shortcut: string, platform = navigator.platform): string {
  const modifier = platformModifier(platform) === 'Meta' ? '⌘' : 'Ctrl+';
  return shortcut.replace(/\bMod\+/g, modifier);
}
