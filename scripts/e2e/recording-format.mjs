import assert from 'node:assert/strict';

// Parse finalized recorder output without imposing a particular signal on its PCM.
export function recordingPcm(bytes) {
  assert.equal(bytes.toString('ascii', 0, 4), 'RF64');
  assert.equal(bytes.toString('ascii', 8, 12), 'WAVE');
  assert.equal(bytes.readUInt32LE(4), 0xffffffff);
  let offset = 12, format, data, sizes;
  while (offset + 8 <= bytes.length) {
    const kind = bytes.toString('ascii', offset, offset + 4);
    const size = bytes.readUInt32LE(offset + 4);
    if (kind === 'data') {
      assert.equal(size, 0xffffffff);
      data = bytes.subarray(offset + 8);
      break;
    }
    assert.ok(size <= bytes.length - offset - 8, 'Truncated recording chunk');
    if (kind === 'ds64') {
      assert.equal(size, 28);
      sizes = { riff: bytes.readBigUInt64LE(offset + 8), data: bytes.readBigUInt64LE(offset + 16), frames: bytes.readBigUInt64LE(offset + 24) };
    }
    if (kind === 'fmt ') {
      assert.ok(size === 16 || size === 40, 'Unsupported recording format extent');
      format = { encoding: bytes.readUInt16LE(offset + 8), channels: bytes.readUInt16LE(offset + 10),
        rate: bytes.readUInt32LE(offset + 12), bits: bytes.readUInt16LE(offset + 22) };
      if (format.encoding === 0xfffe) {
        assert.equal(size, 40);
        assert.equal(bytes.readUInt16LE(offset + 24), 22);
        assert.equal(bytes.readUInt16LE(offset + 26), 32);
        assert.equal(bytes.subarray(offset + 32, offset + 48).toString('hex'), '0300000000001000800000aa00389b71', 'Float32 subformat required');
        format.encoding = 3;
      }
    }
    offset += 8 + size + (size % 2);
  }
  assert.deepEqual(format, { encoding: 3, channels: 2, rate: 48000, bits: 32 });
  assert.ok(data, 'Missing finalized recording data');
  assert.equal(data.length % 8, 0, 'Incomplete stereo Float32 frames');
  assert.deepEqual(sizes, { riff: BigInt(bytes.length - 8), data: BigInt(data.length), frames: BigInt(data.length / 8) }, 'Unfinalized RF64 sizes');
  return { format, data };
}
