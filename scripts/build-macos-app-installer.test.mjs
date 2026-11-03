import assert from 'node:assert/strict';
import test from 'node:test';
import { buildAppInstaller, parseAppInstallerArgs } from './build-macos-app-installer.mjs';
import { buildPackages, distribution } from './build-macos-installer-pkgs.mjs';

test('product build uses the full native transaction, never a separate app/optional Driver transaction', () => {
  assert.equal(buildAppInstaller, buildPackages);
  const xml = distribution('MixRelay.component.pkg', '0.1.1');
  assert.match(xml, /<title>MixRelay<\/title>/);
  assert.match(xml, /customize="never"/);
  // Without it Installer re-executes as Intel and asks Apple silicon Macs for Rosetta.
  assert.match(xml, /hostArchitectures="arm64,x86_64"/);
  assert.equal((xml.match(/<line choice=/g) ?? []).length, 1);
  assert.doesNotMatch(xml, /driverNeedsInstall|active=|choice="driver"|MixRelay Driver/);
});
test('complete product packaging requires one prepared payload and rejects legacy split inputs', () => {
  assert.throws(() => parseAppInstallerArgs(['--app', '/app', '--driverPkg', '/driver.pkg']), /unknown|unsupported|argument/i);
  assert.throws(() => parseAppInstallerArgs(['--payload', '/payload']), /missing/i);
});

test('every macOS build and the Installer share the supported minimum macOS version', async () => {
  const { readFileSync } = await import('node:fs');
  const { execFileSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const { MINIMUM_MACOS } = await import('./macos-minimum.mjs');
  const root = fileURLToPath(new URL('..', import.meta.url));
  assert.equal(MINIMUM_MACOS, '15.0');
  assert.match(distribution('MixRelay.component.pkg', '0.1.1'), new RegExp(`<allowed-os-versions><os-version min="${MINIMUM_MACOS.replace('.', '\\.')}"/></allowed-os-versions>`));
  assert.equal(execFileSync(process.execPath, ['scripts/macos-minimum.mjs'], { cwd: root, encoding: 'utf8' }).trim(), MINIMUM_MACOS);
  // Scan all tracked build inputs, including native fixtures and CI helpers, so a
  // new compiler invocation cannot silently diverge from the Installer's floor.
  const deleted = new Set(execFileSync('git', ['ls-files', '--deleted', '-z'], { cwd: root, encoding: 'utf8' }).split('\0'));
  const paths = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z', '--', 'apps', 'crates', 'native', 'scripts'], { cwd: root, encoding: 'utf8' }).split('\0').filter(path => path && !deleted.has(path));
  // Shipped native code without a floor would build for the SDK's default instead.
  const shipped = new Set([
    'native/macos/engine/build.rs', 'native/macos/transport/build.rs', 'native/macos/process-tap/build.rs',
    'native/macos/driver/build.rs', 'native/macos/hal-driver/bundle/src/build.rs',
  ]);
  for (const path of paths.filter(path => /\.(?:rs|sh|mjs|yml|json|plist)$/.test(path))) {
    const source = readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
    const targets = [...source.matchAll(/(?:version-min=|MACOSX_DEPLOYMENT_TARGET["']?\s*[,=]\s*["']?|DEPLOYMENT_TARGET:\s*&str\s*=\s*"|LSMinimumSystemVersion['"]?\s*[:,]\s*['"])(\d+\.\d+)/g)].map(match => match[1]);
    if (shipped.delete(path)) assert.ok(targets.length > 0, `${path} declares its macOS target`);
    for (const target of targets) assert.equal(target, MINIMUM_MACOS, path);
    if (/version-min=|MACOSX_DEPLOYMENT_TARGET=/.test(source) && /\.(?:sh|mjs)$/.test(path) && !path.endsWith('.test.mjs')) {
      assert.match(source, /macos-minimum\.mjs/, `${path} reads the supported minimum`);
    }
  }
  assert.deepEqual([...shipped], [], 'every shipped native build input is inspected');
  const project = readFileSync(new URL('./e2e/macos-ui/project.yml', import.meta.url), 'utf8');
  assert.match(project, new RegExp(`macOS: '${MINIMUM_MACOS.replace('.', '\\.')}'`));
  const manifest = JSON.parse(readFileSync(new URL('../apps/streamdeck-plugin/manifest.json', import.meta.url), 'utf8'));
  assert.deepEqual(manifest.OS, [{ Platform: 'mac', MinimumVersion: MINIMUM_MACOS }]);
  const removal = readFileSync(new URL('./build-macos-removal-app.mjs', import.meta.url), 'utf8');
  assert.match(removal, /LSMinimumSystemVersion:MINIMUM_MACOS/);
});
