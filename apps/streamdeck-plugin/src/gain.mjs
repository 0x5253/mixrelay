const MINIMUM_DB = -60;
const MAXIMUM_DB = 12;

export function gainDb(value) {
  if (!Number.isFinite(value) || value < MINIMUM_DB || value > MAXIMUM_DB)
    throw new Error('Invalid absolute gain');
  return value;
}

export function clampGainDb(value) {
  if (!Number.isFinite(value)) throw new Error('Invalid absolute gain');
  return Math.max(MINIMUM_DB, Math.min(MAXIMUM_DB, value));
}
