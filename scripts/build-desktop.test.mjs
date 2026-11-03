import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { desktopBuildCommand, linuxBuildIncludesDebian, linuxDebianBundleDirectory, listDebianPackages, main } from "./build-desktop.mjs";
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

test("Linux Debian output selection and target path follow Tauri build options", () => {
  assert.equal(linuxBuildIncludesDebian([]), true);
  assert.equal(linuxBuildIncludesDebian(["--bundles", "deb", "rpm"]), true);
  assert.equal(linuxBuildIncludesDebian(["--bundles=appimage,rpm"]), false);
  assert.equal(linuxBuildIncludesDebian(["-bdeb"]), true);
  assert.equal(linuxBuildIncludesDebian(["--bundles=rpm", "deb"]), true);
  assert.equal(linuxBuildIncludesDebian(["-brpm", "deb"]), true);
  assert.equal(linuxBuildIncludesDebian(["-b=appimage,rpm"]), false);
  assert.equal(linuxBuildIncludesDebian(["-brpm", "--", "deb"]), false);
  assert.equal(linuxBuildIncludesDebian(["-b", "rpm"]), false);
  assert.equal(linuxBuildIncludesDebian(["--no-bundle"]), false);
  assert.equal(linuxBuildIncludesDebian(["--", "--bundles", "rpm"]), true);

  const defaultDirectory = fileURLToPath(new URL("../target/release/bundle/deb", import.meta.url));
  assert.equal(linuxDebianBundleDirectory([]), defaultDirectory);
  assert.equal(linuxDebianBundleDirectory(["--debug", "--target", "aarch64-unknown-linux-gnu"]),
    fileURLToPath(new URL("../target/aarch64-unknown-linux-gnu/debug/bundle/deb", import.meta.url)));
  const targetedDebianDirectory = fileURLToPath(new URL(
    "../target/aarch64-unknown-linux-gnu/release/bundle/deb", import.meta.url,
  ));
  for (const args of [
    ["--target=aarch64-unknown-linux-gnu"], ["-t", "aarch64-unknown-linux-gnu"],
    ["-taarch64-unknown-linux-gnu"], ["-t=aarch64-unknown-linux-gnu"],
  ]) {
    assert.equal(linuxDebianBundleDirectory(args), targetedDebianDirectory, args.join(" "));
  }
  assert.equal(linuxDebianBundleDirectory(["--", "--debug", "-d", "-taarch64-unknown-linux-gnu"]),
    defaultDirectory);
  assert.equal(linuxDebianBundleDirectory([], { CARGO_TARGET_DIR: "/fixture/mixrelay-target" }),
    "/fixture/mixrelay-target/release/bundle/deb");
  assert.equal(linuxDebianBundleDirectory([], { CARGO_TARGET_DIR: "target/custom" }),
    fileURLToPath(new URL("../target/custom/release/bundle/deb", import.meta.url)));
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
      () => ["/fixture/MixRelay_0.1.0_amd64.deb"]);
    assert.equal(status, 0, bundle.join(" "));
    assert.equal(inspected, true, bundle.join(" "));
  }
});

test("Linux config overrides can explicitly select non-Debian or no bundles", async () => {
  const failIfCalled = () => assert.fail("non-Debian output must not be inspected as a Debian build");
  assert.equal(await main(["--config=override.json", "-brpm"], "linux",
    () => ({ status: 0 }), failIfCalled, failIfCalled), 0);
  assert.equal(await main(["-c=override.json", "--no-bundle"], "linux",
    () => ({ status: 0 }), failIfCalled, failIfCalled), 0);
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
  }, async () => { inspected = true; }, () => ["/fixture/default.deb"]);
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
  }, async () => {}, directory => {
    assert.equal(directory, `${expectedRoot}/release/bundle/deb`);
    return ["/fixture/MixRelay_0.1.0_arm64.deb"];
  }), 0);
  assert.equal(process.env.CARGO_TARGET_DIR, "target/custom");
  const { buildPlan } = await import("./prepare-desktop-engine.mjs");
  const plan = buildPlan({
    profile: "release", target: "aarch64-unknown-linux-gnu", platform: "linux", env: buildEnvironment,
  });
  assert.equal(plan.output, `${expectedRoot}/aarch64-unknown-linux-gnu/release/mixrelay-service`);
});

test("Debian package discovery ignores other bundles and rejects non-regular artifacts", () => {
  const entries = [
    { name: "MixRelay.deb", isFile: () => true },
    { name: "MixRelay.rpm", isFile: () => true },
  ];
  assert.deepEqual(listDebianPackages("/fixture/bundle/deb", () => entries), ["/fixture/bundle/deb/MixRelay.deb"]);
  assert.throws(() => listDebianPackages("/fixture/bundle/deb", () => [
    { name: "link.deb", isFile: () => false },
  ]), /not a regular file/);
  assert.deepEqual(listDebianPackages("/fixture/missing", () => {
    const error = new Error("missing");
    error.code = "ENOENT";
    throw error;
  }), []);
});

test("successful Linux Debian build inspects its single produced artifact before returning success", async () => {
  const calls = [];
  const inspected = [];
  const status = await main([], "linux", (command, args, options) => {
    calls.push({ command, args, options });
    return { status: 0 };
  }, async path => { inspected.push(path); }, directory => {
    assert.equal(directory, linuxDebianBundleDirectory([]));
    return ["/fixture/MixRelay_0.1.0_amd64.deb"];
  });
  assert.equal(status, 0);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].options.cwd, fileURLToPath(new URL("../apps/desktop-shell", import.meta.url)));
  assert.deepEqual(inspected, ["/fixture/MixRelay_0.1.0_amd64.deb"]);
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

test("non-Debian builds and nonzero Tauri builds do not invoke Debian inspection", async t => {
  t.mock.method(console, "error", () => {});
  const failIfCalled = () => assert.fail("Debian package path should not be inspected");
  assert.equal(await main(["--bundles", "rpm"], "linux", () => ({ status: 0 }), failIfCalled, failIfCalled), 0);
  assert.equal(await main(["--bundles", "deb"], "linux", () => ({ status: 9 }), failIfCalled, failIfCalled), 9);
});

test("spawn failures, signals, and missing exit status fail the build", async t => {
  t.mock.method(console, "error", () => {});
  for (const result of [{ error: new Error("missing tool") }, { signal: "SIGTERM", status: null }, { status: null }]) {
    assert.equal(await main([], "darwin", () => result), 1);
  }
  assert.equal(await main(["--debug"], "darwin", () => assert.fail("must not spawn")), 1);
});
