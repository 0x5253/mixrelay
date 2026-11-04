export function objectValue(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : null;
}

export function directNumber(value: unknown, key: string): number | null {
  const candidate = objectValue(value)?.[key];
  return typeof candidate === 'number' ? candidate : null;
}

export function directString(value: unknown, key: string): string | null {
  const candidate = objectValue(value)?.[key];
  return typeof candidate === 'string' ? candidate : null;
}

export function nestedNumber(value: unknown, parent: string, key: string): number | null {
  return directNumber(objectValue(value)?.[parent], key);
}

export function nestedString(value: unknown, parent: string, key: string): string | null {
  const candidate = objectValue(objectValue(value)?.[parent])?.[key];
  return typeof candidate === 'string' ? candidate : null;
}

export function directStringArray(value: unknown, key: string): string[] {
  const candidate = objectValue(value)?.[key];
  return Array.isArray(candidate)
    ? candidate.filter((item): item is string => typeof item === 'string')
    : [];
}
