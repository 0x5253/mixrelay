// Matches the HAL observer's FNV hash over canonical Float32 sample bits.
export function pcmHash(frames, channels, sampleBits) {
  let hash = 0xcbf29ce484222325n;
  for (let frame = 0; frame < frames; frame++) for (let channel = 0; channel < channels; channel++)
    hash = BigInt.asUintN(64, (hash ^ BigInt(sampleBits(frame, channel))) * 0x100000001b3n);
  return hash.toString(16).padStart(16, '0');
}
