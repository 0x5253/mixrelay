import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { desktopBuildCommand, linuxBuildIncludes, linuxBundleDirectory, listPackages, main } from "./build-desktop.mjs";
import { DESKTOP_CODE_CHECK_TAURI_CONFIG, desktopCodeCheckEnvironment } from "./core/desktop-build-policy.mjs";
test("desktop code checks use only the canonical no-payload Tauri override", () => {
  const environment = desktopCodeCheckEnvironment({ CARGO_TARGET_DIR: "target/fixture" });
  assert.equal(environment.TAURI_CONFIG, DESKTOP_CODE_CHECK_TAURI_CONFIG);
  assert.equal(environment.MIXRELAY_DESKTOP_CODE_CHECK, "1");
  assert.equal(environment.CARGO_TARGET_DIR, "target/fixture");
  assert.deepEqual(desktopCodeCheckEnvironment({ TAURI_CONFIG: DESKTOP_CODE_CHECK_TAURI_CONFIG }), {
    TAURI_CONFIG: DESKTOP_CODE_CHECK_TAURI_CONFIG,
    MIXRELAY_DESKTOP_CODE_CHECK: "1",
  });
  assert.throws(() => desktopCodeCheckEnvironment({ TAURI_CONFIG: '{"bundle":{"resources":["drivers/"]}}' }),
    /conflicts with the canonical desktop code-check configuration/);
});

test("macOS defaults exclusively to the checked universal build", () => {
  assert.deepEqual(desktopBuildCommand("darwin"), ["zsh", "scripts/build-macos-universal-app.sh"]);
  for (const args of [["--target", "aarch64-apple-darwin"], ["--debug"], ["--config", "other.json"]]) {
    assert.throws(() => desktopBuildCommand("darwin", args), /accepts no overrides/);
  }
});

test("one public bundle supplies the editor and internal background runtime", () => {
  const config = JSON.parse(readFileSync(new URL("../apps/desktop-shell/src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  assert.equal(config.productName, "MixRelay");
  assert.equal(config.identifier, "io.mixrelay.desktop");
  assert.match(config.build.beforeBuildCommand, /desktop:prepare-driver --profile release/);
  assert.match(config.build.beforeBuildCommand, /desktop:prepare-engine --profile release/);
});

test("release builds run with the supplied build environment", async () => {
  for (const platform of ["darwin", "linux"]) {
    const args = platform === "darwin" ? [] : ["--no-bundle"];
    let builds = 0;
    assert.equal(await main(args, platform, (_command, _args, options) => {
      builds++;
      assert.deepEqual(options.env, {});
      return { status: 0 };
    }, undefined, undefined, {}), 0);
    assert.equal(builds, 1);
  }
});

test("Linux retains Tauri build arguments", () => {
  const linux = desktopBuildCommand("linux", ["--bundles", "deb"]);
  assert.equal(linux[0], process.execPath);
  assert.match(linux[1], /tauri\.js$/);
  assert.deepEqual(linux.slice(2), ["build", "--bundles", "deb"]);
  assert.throws(() => desktopBuildCommand("freebsd"), /Unsupported/);
});

test("build runs from the repository and propagates its exit status", async () => {
  const calls = [];
  const status = await main([], "darwin", (...args) => {
    calls.push(args);
    return { status: 17 };
  });
  assert.equal(status, 17);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][2].stdio, "inherit");
  assert.equal(calls[0][2].cwd, fileURLToPath(new URL("..", import.meta.url)).replace(/\/$/, ""));
});

test("native Tauri builds run in the desktop package without a shell", async () => {
  await main(["--no-bundle"], "linux", (_command, _args, options) => {
    assert.equal(options.cwd, fileURLToPath(new URL("../apps/desktop-shell", import.meta.url)));
    assert.equal(options.shell, undefined);
    return { status: 0 };
  });
});

test("desktop packaging refuses the code-check environment before spawning", async t => {
  const errors = [];
  t.mock.method(console, "error", message => errors.push(message));
  const status = await main([], "linux", () => assert.fail("code-check artifacts must not be packaged"),
    undefined, undefined, { MIXRELAY_DESKTOP_CODE_CHECK: "1" });
  assert.equal(status, 1);
  assert.match(errors[0], /disabled in the code-check environment/);
});

test("desktop packaging rejects canonical code-only config even without its marker", async t => {
  const errors = [];
  t.mock.method(console, "error", message => errors.push(message));
  assert.equal(await main([], "linux", () => assert.fail("must not spawn"),
    undefined, undefined, { TAURI_CONFIG: DESKTOP_CODE_CHECK_TAURI_CONFIG }), 1);
  assert.match(errors[0], /disabled in the code-check environment/);
});

test("Linux package selection and target path follow Tauri build options", () => {
  const debian = args => linuxBuildIncludes(args, "deb");
  assert.equal(debian([]), true);
  assert.equal(linuxBuildIncludes([], "rpm"), true);
  assert.equal(debian(["--bundles", "deb", "rpm"]), true);
  assert.equal(linuxBuildIncludes(["--bundles", "deb", "rpm"], "rpm"), true);
  assert.equal(linuxBuildIncludes(["--bundles", "deb"], "rpm"), false);
  assert.equal(debian(["--bundles=appimage,rpm"]), false);
  assert.equal(debian(["-bdeb"]), true);
  assert.equal(debian(["--bundles=rpm", "deb"]), true);
  assert.equal(debian(["-brpm", "deb"]), true);
  assert.equal(debian(["-b=appimage,rpm"]), false);
  assert.equal(debian(["-brpm", "--", "deb"]), false);
  assert.equal(debian(["-b", "rpm"]), false);
  assert.equal(debian(["--no-bundle"]), false);
  assert.equal(debian(["--", "--bundles", "rpm"]), true);

  const defaultDirectory = fileURLToPath(new URL("../target/release/bundle/deb", import.meta.url));
  assert.equal(linuxBundleDirectory([]), defaultDirectory);
  assert.equal(linuxBundleDirectory(["--debug", "--target", "aarch64-unknown-linux-gnu"]),
    fileURLToPath(new URL("../target/aarch64-unknown-linux-gnu/debug/bundle/deb", import.meta.url)));
  const targetedDebianDirectory = fileURLToPath(new URL(
    "../target/aarch64-unknown-linux-gnu/release/bundle/deb", import.meta.url,
  ));
  for (const args of [
    ["--target=aarch64-unknown-linux-gnu"], ["-t", "aarch64-unknown-linux-gnu"],
    ["-taarch64-unknown-linux-gnu"], ["-t=aarch64-unknown-linux-gnu"],
  ]) {
    assert.equal(linuxBundleDirectory(args), targetedDebianDirectory, args.join(" "));
  }
  assert.equal(linuxBundleDirectory(["--", "--debug", "-d", "-taarch64-unknown-linux-gnu"]),
    defaultDirectory);
  assert.equal(linuxBundleDirectory([], { CARGO_TARGET_DIR: "/fixture/mixrelay-target" }),
    "/fixture/mixrelay-target/release/bundle/deb");
  assert.equal(linuxBundleDirectory([], { CARGO_TARGET_DIR: "target/custom" }),
    fileURLToPath(new URL("../target/custom/release/bundle/deb", import.meta.url)));
  assert.equal(linuxBundleDirectory([], {}, "rpm"),
    fileURLToPath(new URL("../target/release/bundle/rpm", import.meta.url)));
});

test("Linux config override without a bundle action fails before stale Debian artifacts can pass", async t => {
  const errors = [];
  t.mock.method(console, "error", message => errors.push(message));
  let builds = 0;
  let listings = 0;
  let inspections = 0;
  const status = await main(["--config", "override.json"], "linux", () => {
    builds += 1;
    return { status: 0 };
  }, async () => { inspections += 1; }, () => {
    listings += 1;
    return ["/fixture/stale-from-previous-build.deb"];
  });
  assert.equal(status, 1);
  assert.equal(builds, 0);
  assert.equal(listings, 0);
  assert.equal(inspections, 0);
  assert.match(errors[0], /provide a valid --bundles\/-b selection or --no-bundle/);
});

test("Linux config override recognizes Tauri config and bundle attached forms", async t => {
  t.mock.method(console, "error", () => {});
  for (const config of [
    ["--config", "override.json"], ["--config=override.json"],
    ["-c", "override.json"], ["-coverride.json"], ["-c=override.json"],
  ]) {
    assert.equal(await main(config, "linux", () => assert.fail("config-only build must fail before spawn")), 1);
  }

  for (const bundle of [
    ["--bundles", "deb"], ["--bundles=deb"], ["-b", "deb"], ["-bdeb"], ["-b=deb"],
    ["--bundles=rpm", "deb"], ["-brpm", "deb"], ["-b=rpm", "deb"],
  ]) {
    let inspected = false;
    const status = await main(["-c", "override.json", ...bundle], "linux",
      () => ({ status: 0 }), async () => { inspected = true; },
      (_directory, extension) => [`/fixture/MixRelay_0.1.0_amd64${extension}`], undefined, async () => {});
    assert.equal(status, 0, bundle.join(" "));
    assert.equal(inspected, true, bundle.join(" "));
  }
});

test("Linux config overrides can explicitly select no bundles, but never an RPM without its DEB", async t => {
  const errors = [];
  t.mock.method(console, "error", message => errors.push(message));
  const failIfCalled = () => assert.fail("no package may be built or inspected");
  assert.equal(await main(["--config=override.json", "-brpm"], "linux",
    failIfCalled, failIfCalled, failIfCalled, undefined, failIfCalled), 1);
  assert.match(errors[0], /checked against the DEB from the same build/);
  assert.equal(await main(["-c=override.json", "--no-bundle"], "linux",
    () => ({ status: 0 }), failIfCalled, failIfCalled, undefined, failIfCalled), 0);
});

test("Linux product builds reject bundle formats without coordinated package removal", async t => {
  const errors = [];
  t.mock.method(console, "error", message => errors.push(message));
  for (const format of ["appimage", "tar", "updater", "all"]) {
    assert.equal(await main(["--bundles", format], "linux",
      () => assert.fail("unsupported package must not build")), 1);
  }
  assert.equal(errors.length, 4);
  assert.ok(errors.every(message => message.includes("must use deb or rpm")));
});

test("config-like arguments after the separator are forwarded, not treated as wrapper overrides", async () => {
  let builds = 0;
  let inspected = false;
  const status = await main(["--", "--config", "runner-value"], "linux", (_command, args) => {
    builds += 1;
    assert.deepEqual(args.slice(-3), ["--", "--config", "runner-value"]);
    return { status: 0 };
  }, async () => { inspected = true; }, (_directory, extension) => [`/fixture/default${extension}`],
  undefined, async () => {});
  assert.equal(status, 0);
  assert.equal(builds, 1);
  assert.equal(inspected, true);
});

test("relative target directory is shared by Tauri, engine hook and Debian inspection", async t => {
  const original = process.env.CARGO_TARGET_DIR;
  t.after(() => {
    if (original === undefined) delete process.env.CARGO_TARGET_DIR;
    else process.env.CARGO_TARGET_DIR = original;
  });
  process.env.CARGO_TARGET_DIR = "target/custom";
  const expectedRoot = fileURLToPath(new URL("../target/custom", import.meta.url));
  let buildEnvironment;
  assert.equal(await main([], "linux", (_command, _args, options) => {
    buildEnvironment = options.env;
    assert.equal(options.env.CARGO_TARGET_DIR, expectedRoot);
    return { status: 0 };
  }, async () => {}, (directory, extension) => {
    assert.equal(directory, `${expectedRoot}/release/bundle/${extension.slice(1)}`);
    return [`/fixture/MixRelay_0.1.0_arm64${extension}`];
  }, undefined, async () => {}), 0);
  assert.equal(process.env.CARGO_TARGET_DIR, "target/custom");
  const { buildPlan } = await import("./prepare-desktop-engine.mjs");
  const plan = buildPlan({
    profile: "release", target: "aarch64-unknown-linux-gnu", platform: "linux", env: buildEnvironment,
  });
  assert.equal(plan.output, `${expectedRoot}/aarch64-unknown-linux-gnu/release/mixrelay-service`);
});

test("package discovery ignores other bundles and rejects non-regular artifacts", () => {
  const entries = [
    { name: "MixRelay.deb", isFile: () => true },
    { name: "MixRelay.rpm", isFile: () => true },
  ];
  assert.deepEqual(listPackages("/fixture/bundle/deb", ".deb", () => entries), ["/fixture/bundle/deb/MixRelay.deb"]);
  assert.deepEqual(listPackages("/fixture/bundle/rpm", ".rpm", () => entries), ["/fixture/bundle/rpm/MixRelay.rpm"]);
  assert.throws(() => listPackages("/fixture/bundle/deb", ".deb", () => [
    { name: "link.deb", isFile: () => false },
  ]), /not a regular file/);
  assert.deepEqual(listPackages("/fixture/missing", ".deb", () => {
    const error = new Error("missing");
    error.code = "ENOENT";
    throw error;
  }), []);
});

test("successful Linux build inspects its DEB, then checks the RPM against that DEB", async () => {
  const calls = [];
  const inspected = [];
  const status = await main([], "linux", (command, args, options) => {
    calls.push({ command, args, options });
    return { status: 0 };
  }, async path => { inspected.push(path); }, (directory, extension) => {
    assert.equal(directory, linuxBundleDirectory([], {}, extension.slice(1)));
    return [`/fixture/MixRelay_0.1.0_amd64${extension}`];
  }, undefined, async (rpm, deb) => { inspected.push([rpm, deb]); });
  assert.equal(status, 0);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].options.cwd, fileURLToPath(new URL("../apps/desktop-shell", import.meta.url)));
  assert.deepEqual(inspected, ["/fixture/MixRelay_0.1.0_amd64.deb",
    ["/fixture/MixRelay_0.1.0_amd64.rpm", "/fixture/MixRelay_0.1.0_amd64.deb"]]);
});

test("a failed RPM comparison fails the Linux build", async t => {
  t.mock.method(console, "error", () => {});
  assert.equal(await main([], "linux", () => ({ status: 0 }), async () => {},
    (_directory, extension) => [`/fixture/MixRelay${extension}`], undefined, async () => {
      throw new Error("RPM payload differs from the DEB");
    }), 1);
});

test("missing or multiple Debian artifacts fail without inspecting a guessed package", async t => {
  t.mock.method(console, "error", () => {});
  for (const packages of [[], ["/fixture/one.deb", "/fixture/two.deb"]]) {
    let inspected = false;
    const status = await main(["--bundles", "deb"], "linux", () => ({ status: 0 }), () => {
      inspected = true;
    }, () => packages);
    assert.equal(status, 1);
    assert.equal(inspected, false);
  }
});

test("invalid Debian package inspection prevents a successful Linux build result", async t => {
  t.mock.method(console, "error", () => {});
  const status = await main(["--bundles=deb"], "linux", () => ({ status: 0 }), async () => {
    throw new Error("ELF architecture mismatch");
  }, () => ["/fixture/wrong-architecture.deb"]);
  assert.equal(status, 1);
});

test("nonzero Tauri builds do not invoke package inspection", async t => {
  t.mock.method(console, "error", () => {});
  const failIfCalled = () => assert.fail("package path should not be inspected");
  assert.equal(await main(["--bundles", "deb"], "linux", () => ({ status: 9 }), failIfCalled, failIfCalled,
    undefined, failIfCalled), 9);
});

test("spawn failures, signals, and missing exit status fail the build", async t => {
  t.mock.method(console, "error", () => {});
  for (const result of [{ error: new Error("missing tool") }, { signal: "SIGTERM", status: null }, { status: null }]) {
    assert.equal(await main([], "darwin", () => result), 1);
  }
  assert.equal(await main(["--debug"], "darwin", () => assert.fail("must not spawn")), 1);
});
