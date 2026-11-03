import type {
  CapturableApplication,
  RuntimeApplicationSelector,
} from '../../core/domain/control/inventory';

/** Presentation identity follows the selected source, never a display name or a current PID guess. */
export function applicationSelectorKey(selector: unknown): string | undefined {
  if (!selector || typeof selector !== 'object') return undefined;
  const value = selector as Record<string, unknown>;
  if (
    value.match === 'bundle_id' &&
    typeof value.bundle_id === 'string' &&
    value.bundle_id &&
    typeof value.include_descendants === 'boolean'
  ) {
    return JSON.stringify(['bundle', value.bundle_id]);
  }
  if (
    value.match === 'process_id' &&
    Number.isInteger(value.process_id) &&
    Number(value.process_id) > 0 &&
    Number(value.process_id) <= 0xffff_ffff &&
    typeof value.include_descendants === 'boolean'
  ) {
    return JSON.stringify(['process', value.process_id]);
  }
  if (value.match === 'executable_path' && typeof value.path === 'string' && value.path) {
    return JSON.stringify(['path', value.path]);
  }
  // All-except is a set of applications, not one app whose icon can be inferred.
  return undefined;
}

/** Current native inventory always carries an exact selector; reject incomplete entries. */
export function applicationInventorySelection(
  application: CapturableApplication,
): { id: string; key: string; selector: RuntimeApplicationSelector } | undefined {
  const selector = application.selector;
  const key = applicationSelectorKey(selector);
  if (!selector || !key) return undefined;
  if (selector.match === 'bundle_id' && selector.bundle_id !== application.bundleId)
    return undefined;
  if (selector.match === 'process_id' && selector.process_id !== application.processId)
    return undefined;
  return { id: `application:${key}`, key, selector };
}
