import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import test from 'node:test';

const workflow = name => readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), 'utf8');

test('workflow references match the free product and the public repository', () => {
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml'))) {
    const source = readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /mixrelay\/mixrelay|MIXRELAY_POLAR|polar-configuration|installer-configuration|linux-dbus-link/);
    // Merges to main run only the release-please bookkeeping, never checks or builds.
    if (name !== 'release-please.yml') assert.doesNotMatch(source, /^\s+push:/m, name);
  }
});

test('official release builds only a verified version tag and publishes all checked artifacts', () => {
  const source = workflow('release');
  assert.match(source, /^on:\n  workflow_call:\n    inputs:\n      release_tag:/m);
  assert.match(source, /workflow_dispatch:/);
  assert.doesNotMatch(source, /^\s+push:/m);
  assert.match(source, /RELEASE_TAG: \$\{\{ inputs\.release_tag \}\}/);
  assert.match(source, /github\.repository == '0x5253\/mixrelay'/);
  assert.match(source, /product-version\.mjs check/);
  assert.match(source, /git merge-base --is-ancestor "\$REVISION" origin\/main/);
  assert.match(source, /source_ref: \$\{\{ needs\.source\.outputs\.revision \}\}/);
  assert.match(source, /needs: \[source, macos, linux\]/);
  // Published file names come from the support contract, and exactly those are published.
  assert.match(source, /support-contract\.mjs assets "\$RELEASE_VERSION" > expected-assets\.txt/);
  assert.match(source, /diff <\(sort expected-assets\.txt\) <\(ls release-assets \| sort\)/);
  assert.match(source, /sha256sum --check SHA256SUMS-/);
  assert.match(source, /sha256sum \* > SHA256SUMS && sha256sum --check SHA256SUMS/);
  assert.match(source, /gh release create .*--verify-tag --draft/);
  assert.match(source, /gh release edit .*--draft=false --prerelease=false --latest/);
  assert.match(source, /gh release view "\$RELEASE_TAG" --json isDraft,isPrerelease > release\.json/);
  assert.match(source, /\.isDraft == true and \.isPrerelease == false/);
  // The tags endpoint never returns drafts, so it cannot find release-please's draft.
  assert.doesNotMatch(source, /releases\/tags\//);
  assert.doesNotMatch(source, /--skip-notarization|secrets: inherit|windows/i);
});

test('signing assets are scoped to the macOS build and cleaned before uploading only distributables', () => {
  const source = workflow('release');
  const macos = source.slice(source.indexOf('\n  macos:'), source.indexOf('\n  publish:'));
  assert.match(macos, /prepare-ci-macos-signing\.mjs --output candidate/);
  assert.match(macos, /\n    environment:\n      name: release\n      deployment: false\n/);
  assert.equal(source.match(/environment:/g).length, 1);
  assert.doesNotMatch(source, /pull_request/);
  assert.match(macos, /if: always\(\)\n\s*run: node scripts\/prepare-ci-macos-signing\.mjs --cleanup/);
  assert.ok(macos.indexOf('--cleanup') < macos.indexOf('actions/upload-artifact@'));
  assert.match(macos, /"notarized-artifacts-built"/);
  assert.match(macos, /pkgutil --check-signature/);
  assert.match(macos, /stapler validate/);
  assert.doesNotMatch(macos, /path:.*(?:keychain|p12|p8|config\.json)/);
  assert.doesNotMatch(source.slice(source.indexOf('\n  publish:')), /secrets\.MIXRELAY/);
});

test('Linux release reuses inspected native-CPU packages and the checked Arch payload', () => {
  const source = workflow('build-linux-release');
  assert.match(source, /workflow_call:/);
  assert.match(source, /linuxBuild/);
  assert.match(source, /inputs\.source_ref \|\| github\.sha/);
  assert.match(source, /BUNDLES: \$\{\{ matrix\.bundles \}\}/);
  assert.match(source, /pnpm desktop:build --bundles "\$BUNDLES"/);
  assert.match(source, /dpkg-deb --field "\$package" Version/);
  assert.match(source, /rpm -qp --queryformat %\{VERSION\} "\$package"/);
  assert.match(source, /prepare-arch-driver-package\.mjs/);
  assert.match(source, /PKGEXT=\.pkg\.tar\.zst makepkg --nodeps --noconfirm/);
  assert.match(source, /bsdtar -xOf "\$package" \.PKGINFO/);
  assert.match(source, /sha256sum \*\.deb > SHA256SUMS-/);
  assert.match(source, /sha256sum \*\.rpm > "SHA256SUMS-\$RPM_ID"/);
  assert.match(source, /name: candidate-\$\{\{ matrix\.rpm \}\}/);
  assert.match(source, /sha256sum \*\.pkg\.tar\.zst/);
});

test('docs, ci and chore pull requests run no build, test or release workflow', async () => {
  const { TYPES, NON_BUILD_TYPES } = await import('./check-conventional-title.mjs');
  // startsWith gates stay exact only while no other title type shares a prefix with a skipped type.
  assert.deepEqual(TYPES.filter(type => NON_BUILD_TYPES.some(skip => type !== skip && type.startsWith(skip))), []);
  const gate = NON_BUILD_TYPES.map(type => `!startsWith(github.event.pull_request.title, '${type}')`).join(' && ');
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml'))) {
    const source = readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8');
    assert.doesNotMatch(source, /pull_request_target|workflow_run:/, name);
    if (!/^\s+pull_request:/m.test(source) || name === 'pr-title.yml') continue;
    const jobs = source.slice(source.indexOf('\njobs:\n')).split(/\n  (?=[a-z][a-z0-9_-]*:\n)/).slice(1);
    for (const job of jobs.filter(job => !/\n    needs:/.test(job))) {
      assert.ok(job.includes(gate), `${name}: ${job.split(':')[0]} must skip ${NON_BUILD_TYPES.join(', ')} pull requests`);
    }
  }
});

test('every bundled resource is inside the Linux build container context', () => {
  const root = new URL('../', import.meta.url);
  const allowed = readFileSync(new URL('.dockerignore', root), 'utf8').split('\n')
    .filter(line => line.startsWith('!')).map(line => line.slice(1))
    .map(pattern => new RegExp(`^${pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*\*/g, '.*').replace(/(?<!\.)\*/g, '[^/]*')}$`));
  const fromTauri = (file, base) => {
    const config = JSON.parse(readFileSync(new URL(`apps/desktop-shell/src-tauri/${file}`, root), 'utf8'));
    const paths = [...Object.keys(config.bundle?.resources ?? {})];
    for (const format of Object.values(config.bundle?.linux ?? {})) paths.push(...Object.values(format.files ?? {}));
    return paths.map(path => new URL(path, base).pathname.slice(new URL(root).pathname.length));
  };
  const resources = [...fromTauri('tauri.conf.json', new URL('apps/desktop-shell/src-tauri/', root)),
    ...fromTauri('tauri.linux.conf.json', new URL('apps/desktop-shell/src-tauri/', root))].filter(path => !path.startsWith('target/'));
  assert.ok(resources.includes('third-party/THIRD-PARTY-NOTICES.txt'));
  for (const path of resources) assert.ok(allowed.some(pattern => pattern.test(path)), `${path} is excluded by .dockerignore`);
});

test('pull requests build every product unsigned and never reach signing or notarization', () => {
  const linux = workflow('pr-build');
  assert.match(linux, /^on:\n  pull_request:\n  workflow_dispatch:\n\n/m);
  assert.match(linux, /uses: \.\/\.github\/workflows\/build-linux-release\.yml/);
  // The arm64 pull request leg of native-lab builds the unsigned macOS app on its runner.
  const macos = workflow('native-lab');
  const build = macos.slice(macos.indexOf('- name: Build the universal app'));
  assert.match(build, /if: github\.event_name == 'pull_request' && matrix\.arch == 'arm64'/);
  assert.match(build, /APPLE_SIGNING_IDENTITY: '-'\n\s+MIXRELAY_CODESIGN_IDENTITY: '-'/);
  assert.match(build, /zsh scripts\/build-macos-universal-app\.sh/);
  assert.match(build, /mixrelay-macos-hal-bundle/);
  for (const source of [linux, macos]) {
    const code = source.split('\n').filter(line => !line.trimStart().startsWith('#')).join('\n');
    assert.doesNotMatch(code, /secrets\.|vars\.|environment:|prepare-ci-macos-signing|release-macos|notar|productbuild|contents: write/i);
  }
  // Only release.yml may reach the signing environment.
  for (const name of readdirSync(new URL('../.github/workflows/', import.meta.url)).filter(name => name.endsWith('.yml') && name !== 'release.yml')) {
    const other = readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), 'utf8');
    assert.doesNotMatch(other, /environment:\s*(?:\n\s+name:\s*)?release\n/, name);
    const secrets = [...other.matchAll(/secrets\.([A-Z0-9_]+)/g)].map(match => match[1]);
    assert.deepEqual([...new Set(secrets)], name === 'release-please.yml' ? ['RELEASE_PLEASE_TOKEN'] : [], name);
  }
});

test('macOS builds select Xcode 26 before compiling Icon Composer assets', () => {
  const release = workflow('release');
  const select = release.indexOf('ls -d /Applications/Xcode_26*.app');
  assert.ok(select > 0);
  assert.ok(release.indexOf('DEVELOPER_DIR=$xcode/Contents/Developer" >> "$GITHUB_ENV"') > select);
  assert.ok(select < release.indexOf('Install the universal build toolchain'));
  assert.match(release, /runs-on: macos-26\n/);
  assert.doesNotMatch(release, /runs-on: macos-1[0-9]/);
  // The review build accepts any Xcode from 26 on, whose actool reads Icon Composer files.
  const lab = workflow('native-lab');
  const labSelect = lab.indexOf("awk -F'[_.]' '$2 >= 26'");
  assert.ok(labSelect > 0 && labSelect < lab.indexOf('- name: Build the universal app'));
});

test('only a merged release PR tags a version and starts the official build', () => {
  const source = workflow('release-please');
  assert.match(source, /^on:\n  push:\n    branches: \[main\]\n/m);
  const release = source.slice(source.indexOf('\n  release:\n'));
  assert.match(release, /if: needs\.release-please\.outputs\.release_created == 'true'/);
  assert.match(release, /uses: \.\/\.github\/workflows\/release\.yml/);
  assert.match(release, /release_tag: \$\{\{ needs\.release-please\.outputs\.tag_name \}\}/);
  // Without inherit, the called release workflow receives empty environment secrets.
  assert.match(release, /\n    secrets: inherit\n/);
  // Nothing else in the bookkeeping workflow builds, tests or signs.
  assert.doesNotMatch(source.slice(0, source.indexOf('\n  release:\n')), /pnpm (install|check|build|test)|cargo |build-macos|build-linux|uses: \.\//);
  // The broad bot token is readable only by jobs inside the main-only release-please environment.
  const jobs = source.slice(source.indexOf('\njobs:\n')).split(/\n  (?=[a-z][a-z0-9_-]*:\n)/).slice(1);
  for (const job of jobs.filter(job => job.includes('RELEASE_PLEASE_TOKEN'))) {
    assert.match(job, /\n    environment:\n      name: release-please\n      deployment: false\n/);
  }
  assert.doesNotMatch(source, /github\.token/);
  const config = JSON.parse(readFileSync(new URL('../release-please-config.json', import.meta.url), 'utf8')).packages['.'];
  assert.equal(config['pull-request-title-pattern'], 'chore: release ${version}');
  assert.equal(config.draft, true);
  // Versions now follow commits since the published release; a pinned version would repeat it.
  assert.equal(config['release-as'], undefined);
  // Merging the release PR is the approval; the header must not promise another one.
  assert.doesNotMatch(config['pull-request-header'], /approv/i);
  // release-please writes '---' on the next line; without a blank line the header renders as a heading.
  assert.match(config['pull-request-header'], /[^\n]\n$/);
});

test('each pull request workflow reports one required check that skipped work passes', () => {
  for (const name of ['ui-quality', 'native-lab', 'qualification-hosts', 'pr-build']) {
    const source = workflow(name);
    const passed = source.slice(source.indexOf('\n  passed:\n'));
    assert.ok(passed.length > 1, name);
    assert.match(passed, /\n    if: always\(\)\n/, name);
    const jobs = [...source.slice(source.indexOf('\njobs:\n')).matchAll(/^  ([a-z][a-z0-9_-]*):$/gm)].map(match => match[1]).filter(job => job !== 'passed');
    assert.deepEqual(passed.match(/needs: \[([^\]]+)\]/)[1].split(', ').sort(), jobs.sort(), name);
    // Reads every job of the run: a matrix leg that never got a runner is invisible through `needs`.
    assert.match(passed, /\n    permissions:\n      actions: read\n/, name);
    assert.match(passed, /actions\/runs\/\$GITHUB_RUN_ID\/attempts\/\$GITHUB_RUN_ATTEMPT\/jobs" --paginate/, name);
    assert.equal(passed.match(/SELF: (.+)\n/)[1], passed.match(/\n    name: (.+)\n/)[1], name);
    assert.ok(passed.includes(`$3 != "success" && $3 != "skipped"`), name);
    assert.doesNotMatch(passed, /needs\.\*\.result/, name);
  }
});

test('workflows that report required checks run on every pull request', () => {
  // A path-filtered workflow never reports on unrelated PRs, so it cannot be required.
  for (const name of ['pr-title', 'ui-quality', 'native-lab', 'pr-build']) {
    const triggers = workflow(name).split('\npermissions:')[0];
    assert.doesNotMatch(triggers, /paths(-ignore)?:/, name);
  }
});

test('pull requests run the macOS subset; manual runs keep every supported macOS', () => {
  const source = workflow('native-lab');
  assert.match(source, /macos: \$\{\{ github\.event_name == 'pull_request' && steps\.matrix\.outputs\.macosPullRequest \|\| steps\.matrix\.outputs\.macos \}\}/);
});
