/** A value from outside the process did not have the shape its contract promises. */
export class InvalidResponseError extends Error {
  constructor(
    readonly path: string,
    expected: string,
  ) {
    super(`invalid response at ${path}: expected ${expected}`);
    this.name = 'InvalidResponseError';
  }
}

/** Checks `value` and returns it with its contract type, or throws InvalidResponseError. */
export type Decoder<T> = (value: unknown, path: string) => T;

type Literal = string | number | boolean | null;

function fail(path: string, expected: string): never {
  throw new InvalidResponseError(path, expected);
}

export const unknownValue: Decoder<unknown> = (value) => value;

export const string: Decoder<string> = (value, path) =>
  typeof value === 'string' ? value : fail(path, 'a string');

export const boolean: Decoder<boolean> = (value, path) =>
  typeof value === 'boolean' ? value : fail(path, 'a boolean');

/** JSON has no NaN or infinity, so any non-finite number is a malformed reply. */
export const number: Decoder<number> = (value, path) =>
  typeof value === 'number' && Number.isFinite(value) ? value : fail(path, 'a finite number');

export const nonNegativeInteger: Decoder<number> = (value, path) =>
  Number.isSafeInteger(value) && typeof value === 'number' && value >= 0
    ? value
    : fail(path, 'a non-negative integer');

export function literal<const Values extends readonly Literal[]>(
  ...values: Values
): Decoder<Values[number]> {
  return (value, path) =>
    values.some((candidate) => candidate === value)
      ? (value as Values[number])
      : fail(path, values.map((candidate) => JSON.stringify(candidate)).join(' | '));
}

export function nullable<T>(decoder: Decoder<T>): Decoder<T | null> {
  return (value, path) => (value === null ? null : decoder(value, path));
}

/** An absent field. A JSON null is not absent; use `nullable` where the contract sends one. */
export function optional<T>(decoder: Decoder<T>): Decoder<T | undefined> {
  return (value, path) => (value === undefined ? undefined : decoder(value, path));
}

export function array<T>(decoder: Decoder<T>): Decoder<T[]> {
  return (value, path) =>
    Array.isArray(value)
      ? value.map((item, index) => decoder(item, `${path}[${index}]`))
      : fail(path, 'an array');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function record<T>(decoder: Decoder<T>): Decoder<Record<string, T>> {
  return (value, path) => {
    if (!isRecord(value)) return fail(path, 'an object');
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, decoder(item, `${path}.${key}`)]),
    );
  };
}

let unnamedFieldSink: ((path: string) => void) | undefined;

/** Runs `decode` and lists the fields no contract named. Decoding keeps such fields, so only
 * this shows a field the sender renamed; contract tests compare the list with what they expect. */
export async function unnamedFields(decode: () => unknown): Promise<string[]> {
  const found: string[] = [];
  unnamedFieldSink = (path) => found.push(path);
  try {
    await decode();
  } finally {
    unnamedFieldSink = undefined;
  }
  return found;
}

function reportUnnamedFields(
  value: Record<string, unknown>,
  fields: object,
  path: string,
  sink: (path: string) => void,
) {
  for (const key of Object.keys(value)) if (!Object.hasOwn(fields, key)) sink(`${path}.${key}`);
}

/** Decodes every field the contract names. Fields it does not name are kept unchanged, so a
 * document the UI edits and sends back never loses what a newer engine added. */
export function object<T>(fields: { [Key in keyof T]-?: Decoder<T[Key]> }): Decoder<T> {
  return (value, path) => {
    if (!isRecord(value)) return fail(path, 'an object');
    if (unnamedFieldSink) reportUnnamedFields(value, fields, path, unnamedFieldSink);
    const result: Record<string, unknown> = { ...value };
    for (const [key, decoder] of Object.entries<Decoder<unknown>>(fields)) {
      const decoded = decoder(value[key], `${path}.${key}`);
      if (decoded !== undefined) result[key] = decoded;
    }
    // Every contract field was checked by its own decoder above.
    return result as T;
  };
}

/** A union told apart by one literal field, such as `{ mode: 'auto' } | { mode: 'explicit' }`. */
export function tagged<T>(key: string, variants: Record<string, Decoder<T>>): Decoder<T> {
  return (value, path) => {
    const tag = isRecord(value) ? value[key] : undefined;
    // A tag such as `toString` names an inherited property, not a variant.
    const decoder =
      typeof tag === 'string' && Object.hasOwn(variants, tag) ? variants[tag] : undefined;
    return decoder
      ? decoder(value, path)
      : fail(`${path}.${key}`, Object.keys(variants).join(' | '));
  };
}
