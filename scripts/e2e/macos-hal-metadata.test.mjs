import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('HAL factory registration coexists with the NullAudio plugin on hosted macOS', () => {
  const metadata = readFileSync(new URL('../../native/macos/hal-driver/driver/Info.plist', import.meta.url), 'utf8');
  const factories = metadata.match(/<key>CFPlugInFactories<\/key>\s*<dict>([\s\S]*?)<\/dict>/)?.[1];
  const factory = factories?.match(/<key>([^<]+)<\/key>\s*<string>MixRelayHAL_Create<\/string>/)?.[1];
  assert.match(factory ?? '', /^[A-F0-9]{8}(?:-[A-F0-9]{4}){3}-[A-F0-9]{12}$/);
  // This is the independently shipped factory in the hosted image's NullAudio.driver.
  const registry = new Map([['99A15A8B-DA3C-42C3-BD5D-D035A0C2377A', 'NullAudio_Create']]);
  assert.equal(registry.has(factory), false, 'MixRelay must own a distinct CFPlugIn factory identity');
  registry.set(factory, 'MixRelayHAL_Create');
  const types = metadata.match(/<key>CFPlugInTypes<\/key>\s*<dict>([\s\S]*?)<\/dict>/)?.[1];
  const type = types?.match(/<key>([^<]+)<\/key>\s*<array>\s*<string>([^<]+)<\/string>/);
  assert.equal(type?.[1], '443ABAB8-E7B3-491A-B985-BEB9187030DB', 'Apple AudioServerPlugIn type remains shared');
  assert.equal(registry.get(type?.[2]), 'MixRelayHAL_Create', 'The advertised factory must resolve to MixRelay');
});
