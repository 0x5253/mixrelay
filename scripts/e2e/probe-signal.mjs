// MRRoundTripProbe's integer-coded signal, before lead/tail placement.
export function probeSampleBits(frame, channel) {
  let value = ((Math.floor(frame / 4) >>> 0) ^ Math.imul(channel + 1, 0x9e3779b9)) >>> 0;
  value ^= value >>> 16;
  value = Math.imul(value, 0x7feb352d);
  value ^= value >>> 15;
  value = Math.imul(value, 0x846ca68b);
  value ^= value >>> 16;
  const payload = (value >>> ((frame % 4) * 8)) & 255;
  let code = ((payload << 6) | channel) - 8192;
  if (code >= 0) code++;
  const magnitude = Math.abs(code), exponent = 31 - Math.clz32(magnitude);
  return ((code < 0 ? 0x80000000 : 0) | ((exponent + 112) << 23)
    | ((magnitude - 2 ** exponent) << (23 - exponent))) >>> 0;
}

export function probeRangeHash(start, frames, channels) {
  let hash = 0xcbf29ce484222325n;
  for (let frame = start; frame < start + frames; frame++) {
    for (let channel = 0; channel < channels; channel++) {
      hash = BigInt.asUintN(64, (hash ^ BigInt(probeSampleBits(frame, channel))) * 0x100000001b3n);
    }
  }
  return hash.toString(16).padStart(16, '0');
}
