import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { prepareMacosUiRunner } from '../e2e/macos-ui-runner.mjs';

test('native observation control acknowledges only between scans and preserves the exact request token', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-observer-control-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'main.swift');
  writeFileSync(fixture, `
import Foundation
let directory = URL(fileURLWithPath: CommandLine.arguments[1])
let control = PermissionObservationControl(directory: directory)
let command = directory.appendingPathComponent("permission-observation.command.json")
let acknowledgement = directory.appendingPathComponent("permission-observation.ack.json")
let initial = try control.shouldObserve()
precondition(initial)
for (generation, phase, expected) in [("lowercase-token", "paused", false), ("resume-token", "observing", true)] {
    try JSONSerialization.data(withJSONObject: ["generation": generation, "phase": phase]).write(to: command, options: .atomic)
    let observed = try control.shouldObserve()
    precondition(observed == expected)
    let bytes = try Data(contentsOf: acknowledgement)
    let state = try JSONSerialization.jsonObject(with: bytes) as! [String: String]
    precondition(state["generation"] == generation && state["phase"] == phase)
    precondition(ISO8601DateFormatter().date(from: state["at"]!) != nil)
    let unchanged = try control.shouldObserve()
    let unchangedBytes = try Data(contentsOf: acknowledgement)
    precondition(unchanged == expected && unchangedBytes == bytes)
}
try Data("{\\\"generation\\\":\\\"invalid\\\",\\\"phase\\\":\\\"unrecognized\\\"}".utf8).write(to: command, options: .atomic)
do { _ = try control.shouldObserve(); fatalError("Invalid commands cannot resume scans") }
catch is DecodingError {}
`);
  const source = fileURLToPath(new URL('../e2e/macos-ui/PermissionObservationControl.swift', import.meta.url));
  const executable = join(directory, 'observer-control');
  const built = spawnSync('xcrun', ['swiftc', '-swift-version', '5', source, fixture, '-o', executable],
    { encoding: 'utf8', timeout: 120000 });
  assert.equal(built.status, 0, built.stderr);
  const result = spawnSync(executable, [directory], { encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 0, result.stderr);
});

test('native WebKit preference bridge changes the real feature and rejects absent keys and invalid objects', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-webkit-feature-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = fileURLToPath(new URL('../../apps/desktop/src-tauri/src/macos_webview/MRWebKitPreferences.m', import.meta.url));
  const fixture = join(directory, 'main.m');
  writeFileSync(fixture, `
#import <WebKit/WebKit.h>
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
extern int32_t mr_webkit_set_bool_feature(void *, const char *, bool);
int main(void) { @autoreleasepool {
  WKPreferences *preferences = [WKPreferences new];
  const char *key = "UseGPUProcessForDOMRenderingEnabled";
  assert(mr_webkit_set_bool_feature((__bridge void *)preferences, key, false) == 0);
  assert(mr_webkit_set_bool_feature((__bridge void *)preferences, key, true) == 0);
  assert(mr_webkit_set_bool_feature((__bridge void *)preferences, "MissingQualificationFeature", true) == 3);
  assert(mr_webkit_set_bool_feature(NULL, key, true) == 1);
  assert(mr_webkit_set_bool_feature((__bridge void *)preferences, NULL, true) == 1);
  assert(mr_webkit_set_bool_feature((__bridge void *)[NSObject new], key, true) == 1);
} return 0; }
`);
  const executable = join(directory, 'feature');
  const built = spawnSync('xcrun', ['clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror', source,
    fixture, '-framework', 'WebKit', '-framework', 'Foundation', '-o', executable],
  { encoding: 'utf8', timeout: 60000 });
  assert.equal(built.status, 0, built.stderr);
  const result = spawnSync(executable, [], { encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 0, result.stderr);
});

test('native screenshot validation rejects a blank body with window chrome and a cursor but accepts rendered content', t => {
  assert.equal(process.platform, 'darwin');
  const directory = mkdtempSync(join(tmpdir(), 'mr-window-pixels-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const fixture = join(directory, 'main.swift');
  writeFileSync(fixture, `
import AppKit
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let canvas = CGContext(data: nil, width: 256, height: 256, bitsPerComponent: 8,
    bytesPerRow: 256 * 4, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
func fill(_ rect: CGRect, _ components: [CGFloat]) {
    canvas.setFillColor(CGColor(colorSpace: space, components: components)!)
    canvas.fill(rect)
}
func rendered() -> Bool { WindowScreenshot.hasRenderedContent(NSBitmapImageRep(cgImage: canvas.makeImage()!)) }
fill(CGRect(x: 0, y: 0, width: 256, height: 256), [0.5, 0.5, 0.5, 1])
fill(CGRect(x: 0, y: 0, width: 256, height: 32), [1, 0, 0, 1])
fill(CGRect(x: 0, y: 224, width: 256, height: 32), [0, 0, 1, 1])
precondition(!rendered(), "Window chrome cannot establish rendered app content")
fill(CGRect(x: 126, y: 126, width: 4, height: 4), [0, 0, 0, 1])
precondition(!rendered(), "A cursor on a blank body is insufficient")
fill(CGRect(x: 90, y: 80, width: 100, height: 110), [1, 1, 1, 1])
precondition(rendered(), "Rendered node content must be accepted")
for path in CommandLine.arguments.dropFirst() {
    let image = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: path)))!
    print(path, WindowScreenshot.hasRenderedContent(image))
}
`);
  const source = fileURLToPath(new URL('../e2e/macos-ui/WindowScreenshot.swift', import.meta.url));
  const executable = join(directory, 'pixels');
  const built = spawnSync('xcrun', ['swiftc', '-swift-version', '5', source, fixture, '-o', executable],
    { encoding: 'utf8', timeout: 120000 });
  assert.equal(built.status, 0, built.stderr);
  const result = spawnSync(executable, [], { encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 0, result.stderr);
});

test('native ad-hoc cdhash requirement compiles to a blob that validates only the signed code',
  () => {
    assert.equal(process.platform, 'darwin', 'Run the native signing boundary on macOS');
    const directory = mkdtempSync(join(tmpdir(), 'mr-ui-csreq-'));
    const runner = join(directory, 'DerivedData/Build/Products/Debug/InstalledUITests-Runner.app');
    const executable = join(runner, 'Contents/MacOS/runner');
    const blob = join(directory, 'runner.csreq');
    const command = (program, args) => {
      const result = spawnSync(program, args, { encoding: 'utf8' });
      assert.equal(result.error, undefined);
      assert.equal(result.status, 0, result.stderr);
      return result;
    };
    try {
      mkdirSync(join(runner, 'Contents/MacOS'), { recursive: true });
      copyFileSync('/usr/bin/true', executable);
      writeFileSync(join(runner, 'Contents/Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>io.mixrelay.qualification.ui.xctrunner</string>
<key>CFBundleExecutable</key><string>runner</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>`);
      let sql;
      let pidReads = 0;
      // Run the production signing/requirement path on a local app fixture.
      // Xcode and TCC remain disposable-session boundaries; no host grant is written.
      prepareMacosUiRunner(directory, {
        list: () => ['InstalledUI_macosx.xctestrun'],
        runCommand(program, args) {
          if (program === 'xcodebuild') return { stdout: '', stderr: '' };
          if (program === 'plutil' && args.includes('json')) return { stdout: JSON.stringify({ InstalledUITests: {
            IsUITestBundle: true, PreferredScreenCaptureFormat: 'screenshots', SystemAttachmentLifetime: 'keepNever',
          } }) };
          if (program === 'sudo') {
            if (args[1] === 'sqlite3') sql = args.at(-1);
            return { stdout: '', stderr: '' };
          }
          if (program === 'launchctl') return { stdout: `pid = ${++pidReads}\n` };
          return command(program, args);
        },
      });
      const { requirement } = JSON.parse(readFileSync(join(directory, 'prepared-runner.json')));
      assert.match(requirement, /^cdhash H"[0-9a-f]+"(?: or cdhash H"[0-9a-f]+")*$/);
      const bytes = readFileSync(blob);
      assert.ok(bytes.length > 0);
      assert.ok(sql.includes(`'io.mixrelay.qualification.ui.xctrunner',0,2,4,1,X'${bytes.toString('hex')}'`));
      assert.match(sql, /'kTCCServiceAccessibility'/);
      assert.match(sql, /'kTCCServiceScreenCapture'/);
      assert.doesNotMatch(sql, /kTCCServiceMicrophone|kTCCServiceAudioCapture/);
      command('codesign', ['--verify', '--strict', '-R', blob, runner]);
      // Replacing the code must invalidate the exact blob used for the grant.
      copyFileSync('/usr/bin/false', executable);
      command('codesign', ['--force', '--sign', '-', runner]);
      const rejected = spawnSync('codesign', ['--verify', '--strict', '-R', blob, runner], { encoding: 'utf8' });
      assert.notEqual(rejected.status, 0);
      assert.match(rejected.stderr, /failed to satisfy specified code requirement/);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

