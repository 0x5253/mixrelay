import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { prepareReleaseManifest } from "./prepare-release-manifest.mjs";
import { qualificationFixture } from './e2e/qualification.fixture.mjs';

const publishedAt = "2026-10-06T18:20:30.123456789+09:00";

function fixture(t) {
  const f = qualificationFixture(t);
  const { directory } = f;
  const args = ["--directory", directory, "--product", "mix-relay", "--version", "1.2.3",
    "--published-at", publishedAt, ...f.qualificationArgs];
  return { ...f, args };
}

test("one product produces the exact publisher schema and preserve installer inputs", t => {
  const { directory, args, artifactArgs, artifacts } = fixture(t);
  const file = "MixRelay Install.pkg";
  const original = readFileSync(join(directory, file));
  writeFileSync(join(directory, "Uninstall MixRelay.pkg"), "Uninstaller is not selected\n");
  for (const product of ["mix-relay"]) {
    const selected = args.map(value => value === "mix-relay" ? product : value);
    const path = prepareReleaseManifest([...selected, ...artifactArgs]);
    assert.equal(path, join(directory, `${product}.release.json`));
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), {
      product, version: "1.2.3", published_at: publishedAt,
      artifacts,
    });
  }
  assert.deepEqual(readFileSync(join(directory, file)), original);
  assert.equal(readFileSync(join(directory, "Uninstall MixRelay.pkg"), "utf8"), "Uninstaller is not selected\n");
  assert.equal(readdirSync(directory).length, 11);
});

test("the CLI publishes the complete qualified artifact selection and an explicit public notes URL", t => {
  const { directory, args, artifactArgs, artifacts } = fixture(t);
  const script = fileURLToPath(new URL("./prepare-release-manifest.mjs", import.meta.url));
  const result = spawnSync(process.execPath, [script, ...args,
    "--notes-url", "https://rawsector.com/products/mixrelay",
    ...artifactArgs], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(join(directory, "mix-relay.release.json"), "utf8"));
  assert.deepEqual(manifest, {
    product: "mix-relay", version: "1.2.3", published_at: publishedAt,
    notes_url: "https://rawsector.com/products/mixrelay",
    artifacts,
  });
});

test('qualification cannot be omitted or used to release a subset of tested artifacts', t => {
  const { args, artifactArgs } = fixture(t);
  assert.throws(() => prepareReleaseManifest([...args.slice(0, 10), ...artifactArgs]), /--candidate and --reports/);
  assert.throws(() => prepareReleaseManifest([...args, ...artifactArgs.slice(0, 2)]), /exactly/);
});

test('the update manifest excludes the plugin but publication requires its exact candidate bytes and evidence', t => {
  const f = fixture(t);
  const plugin = f.candidate.artifacts.find(a => a.id === 'macos-universal-streamdeck');
  writeFileSync(join(f.directory, plugin.file), 'changed plugin after qualification');
  assert.throws(() => prepareReleaseManifest([...f.args, ...f.artifactArgs]), /bytes changed/);
  assert.throws(() => prepareReleaseManifest([...f.args, ...f.artifactArgs, '--artifact', `macos:universal:${plugin.file}`]), /PKG/);
});

test("invalid options and artifact declarations cannot write a manifest", t => {
  const { directory, args } = fixture(t);
  const artifact = ["--artifact", "macos:universal:MixRelay Install.pkg"];
  const replace = (name, value) => args.map((item, index) => args[index - 1] === name ? value : item);
  for (const invalid of [
    args.slice(2),
    replace("--directory", join(directory, "missing")),
    replace("--directory", join(directory, "MixRelay Install.pkg")),
    replace("--product", "other"),
    ...["v1.2.3", "01.2.3", "1.2.3-beta", "1.2"].map(value => replace("--version", value)),
    ...["2026-02-30T00:00:00Z", "2026-10-06", "2026-10-06T24:00:00Z", "invalid"]
      .map(value => replace("--published-at", value)),
  ]) {
    assert.throws(() => prepareReleaseManifest([...invalid, ...artifact]));
  }
  for (const value of ["../escape.pkg", "nested/installer.pkg", "nested\\installer.pkg", "..", "", "bad\0.pkg"]) {
    assert.throws(() => prepareReleaseManifest([...args, "--artifact", `macos:universal:${value}`]));
  }
  for (const value of ["windows:x64:MixRelay-arm64.deb", "macos:arm64:MixRelay Install.pkg",
    "linux:universal:MixRelay-arm64.deb", "linux:x64:MixRelay Install.pkg", "macos:universal:app.zip", "pkg"]) {
    assert.throws(() => prepareReleaseManifest([...args, "--artifact", value]));
  }
  for (const url of ["http://mixrelay.com/releases", "https:mixrelay.com/releases",
    "https://user:pass@mixrelay.com/releases", "https://mixrelay.com/#notes"]) {
    assert.throws(() => prepareReleaseManifest([...args, ...artifact, "--notes-url", url]));
  }
  assert.throws(() => prepareReleaseManifest(args), /At least one/);
  assert.throws(() => prepareReleaseManifest([...args, ...artifact, ...artifact]), /Duplicate artifact/);
  assert.throws(() => prepareReleaseManifest([...args, ...artifact, "--unexpected", "value"]));
  assert.equal(existsSync(join(directory, "mix-relay.release.json")), false);
});

test("missing, empty, directory and linked installers are rejected without changing existing manifests", t => {
  const { directory, args } = fixture(t);
  writeFileSync(join(directory, "empty.pkg"), "");
  mkdirSync(join(directory, "directory.pkg"));
  symlinkSync(join(directory, "MixRelay Install.pkg"), join(directory, "linked.pkg"));
  const path = join(directory, "mix-relay.release.json");
  writeFileSync(path, "Existing manifest must remain unchanged\n");
  for (const file of ["missing.pkg", "empty.pkg", "directory.pkg", "linked.pkg"]) {
    assert.throws(() => prepareReleaseManifest([...args, "--artifact", `macos:universal:${file}`]));
  }
  assert.throws(() => prepareReleaseManifest([...args, "--artifact", "macos:universal:MixRelay Install.pkg"]),
    { code: "EEXIST" });
  assert.equal(readFileSync(path, "utf8"), "Existing manifest must remain unchanged\n");
});

test('release notes reject unapproved repositories, unresolved domains and unapproved new hosts before writing', t => {
  const { args, artifactArgs, directory } = fixture(t);
  for (const url of ['https://github.com/other-project/mixrelay/releases', 'https://mixrelay.com/releases',
    'https://new-host.example.org/notes']) {
    assert.throws(() => prepareReleaseManifest([...args, ...artifactArgs, '--notes-url', url]), /unapproved/);
    assert.equal(existsSync(join(directory, 'mix-relay.release.json')), false);
  }
});
