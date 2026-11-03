import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, statSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const job = fileURLToPath(new URL("./guest-job.sh", import.meta.url));
const buildJob = fileURLToPath(new URL("./guest-build-job.sh", import.meta.url));
const pipewireCheck = fileURLToPath(new URL("./guest-pipewire-check.sh", import.meta.url));
const nativeTestRoot = fileURLToPath(new URL("../../target/native-tests/", import.meta.url));
function fixture(t) {
  mkdirSync(nativeTestRoot, { recursive: true });
  const directory = mkdtempSync(join(nativeTestRoot, "mixrelay-guest-test-"));
  t.after(() => rmSync(directory, { recursive: true })); // Only this test's mkdtemp directory.
  return directory;
}
test("systemd build job selects the lab Rust toolchain without inheriting root HOME", () => {
  const source = readFileSync(buildJob, "utf8");
  assert.match(source, /\[\[ \$\(uname -s\) == Linux && \$\(id -u\) == 0 \]\] \|\| exit 2/);
  assert.match(source, /\[\[ \$\(hostname\) == mixrelay-debian-engine \]\] \|\| exit 2/);
  assert.match(source, /\[\[ "\$workspace" == \/home\/lab\/\.lab\/mixrelay-integration-\* \]\] \|\| exit 2/);
  const command = source.slice(source.indexOf("exec systemd-run")).replace(/^exec /, "");
  // Replay only argv construction, not the privileged wrapper or a fake toolchain.
  const result = spawnSync("bash", ["-c", `
    set -euo pipefail
    systemd-run() { printf '%s\\n' "$@"; }
    job_id=toolchain-regression
    workspace=/home/lab/.lab/mixrelay-integration-current
    set -- cargo clippy --locked
    ${command}
  `], { env: { ...process.env, HOME: "/root" }, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const args = result.stdout.trim().split("\n");
  assert.ok(args.includes("--uid=lab"));
  assert.ok(args.includes("--setenv=HOME=/home/lab"));
  assert.ok(args.includes("--setenv=PATH=/home/lab/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"));
  assert.ok(args.includes("--property=KillMode=control-group"));
  assert.ok(!args.some(arg => arg.includes("/root")));
  assert.deepEqual(args.slice(-6), ["/bin/bash", "/home/lab/.lab/mixrelay-integration-current/scripts/lab/guest-job.sh", "toolchain-regression", "cargo", "clippy", "--locked"]);
});

test("official Linux preparation includes source-sync and inspection tools", () => {
  const source = readFileSync(new URL("../../infra/lab/install-linux-deps.sh", import.meta.url), "utf8");
  for (const branch of source.split(/\n\s+;;/).slice(0, 2)) {
    assert.match(branch, /\brsync\b/);
    assert.match(branch, /\bripgrep\b/);
  }
});
test("guest job records completion and preserves command exit status", t => {
  const directory = fixture(t);
  for (const code of [0, 7]) {
    const id = `result-${code}`;
    const result = spawnSync("bash", [job, id, "bash", "-c", `echo job-output; exit ${code}`], {
      env: { ...process.env, MIXRELAY_LAB_JOB_ROOT: directory }, encoding: "utf8",
    });
    assert.equal(result.status, code);
    const path = join(directory, `mixrelay-lab-${id}`);
    assert.equal(readFileSync(join(path, "exit-code"), "utf8"), `${code}\n`);
    assert.equal(readFileSync(join(path, "output.log"), "utf8"), "job-output\n");
    assert.equal(statSync(path).mode & 0o777, 0o700);
  }
});
test("guest job rejects unsafe IDs and never overwrites previous evidence", t => {
  const directory = fixture(t);
  const run = id => spawnSync("bash", [job, id, "true"], {
    env: { ...process.env, MIXRELAY_LAB_JOB_ROOT: directory }, encoding: "utf8",
  });
  for (const id of ["../escape", "a/b", "has space"]) assert.equal(run(id).status, 2);
  assert.equal(run("once").status, 0);
  assert.notEqual(run("once").status, 0);
  assert.equal(readFileSync(join(directory, "mixrelay-lab-once", "exit-code"), "utf8"), "0\n");
});
test("guest job preserves command stdin while owning the child", t => {
  const directory = fixture(t);
  const result = spawnSync("bash", [job, "stdin", "bash", "-c", 'IFS= read -r value; printf "%s\\n" "$value"'], {
    env: { ...process.env, MIXRELAY_LAB_JOB_ROOT: directory }, input: "owned-input\n", encoding: "utf8",
  });
  assert.equal(result.status, 0);
  assert.equal(readFileSync(join(directory, "mixrelay-lab-stdin", "output.log"), "utf8"), "owned-input\n");
});
for (const [signal, expected, ignoresTerm] of [["SIGINT", 130, false], ["SIGTERM", 143, false], ["SIGTERM", 143, true]]) {
  test(`guest job records ${signal} and releases its exact child${ignoresTerm ? " ignoring TERM" : ""}`, async t => {
    const directory = fixture(t);
    const id = `signal-${signal}`;
    const childCommand = ignoresTerm ? 'trap "" TERM; echo child_pid=$$; exec sleep 10' : 'echo child_pid=$$; exec sleep 10';
    const running = spawn("bash", [job, id, "bash", "-c", childCommand], {
      env: { ...process.env, MIXRELAY_LAB_JOB_ROOT: directory }, stdio: "ignore",
    });
    const closed = new Promise((resolve, reject) => {
      running.once("error", reject);
      running.once("close", (code, exitSignal) => resolve({ code, exitSignal }));
    });
    t.after(async () => { if (running.exitCode === null && running.signalCode === null) running.kill("SIGTERM"); await closed; });
    const path = join(directory, `mixrelay-lab-${id}`);
    let childPid;
    for (let attempt = 0; attempt < 100; attempt++) {
      try { childPid = Number(readFileSync(join(path, "output.log"), "utf8").match(/child_pid=(\d+)/)?.[1]); } catch {}
      if (childPid) break;
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    assert.ok(childPid, "Owned child did not start");
    const started = Date.now();
    running.kill(signal);
    const result = await closed;
    assert.deepEqual(result, { code: expected, exitSignal: null });
    assert.equal(readFileSync(join(path, "exit-code"), "utf8"), `${expected}\n`);
    assert.throws(() => process.kill(childPid, 0), { code: "ESRCH" });
    assert.ok(Date.now() - started < 5000, "Signal cleanup was not bounded");
  });
}
for (const scenario of ["occupied", "start-failed", "probe-failed", "success"]) {
  test(`PipeWire probe lifecycle: ${scenario}`, t => {
    const directory = fixture(t);
    const bin = join(directory, "bin");
    mkdirSync(bin);
    const mock = (name, body) => writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
    mock("uname", "echo Linux");
    mock("id", "echo 1000");
    mock("sha256sum", "exit 0");
    mock("pw-cat", "exit 0");
    mock("engine", "exit 0");
    // A 404 response is still an occupied control socket. The first probe must not
    // use curl --fail, which would incorrectly allow replacing that service.
    mock("curl", `
      [ "$1" = --unix-socket ] && [ "$2" = "$LAB_MOCK_ROOT/ipc/engine.sock" ] || exit 99
      [ "$3" = --header ] && [ "$4" = 'Host: mixrelay-engine' ] || exit 99
      case " $* " in *" --fail "*) fail=1 ;; *) fail=0 ;; esac
      if [ ! -f "$LAB_MOCK_ROOT/queried" ]; then
        touch "$LAB_MOCK_ROOT/queried"
        [ "$LAB_SCENARIO" = occupied ] && [ "$fail" = 0 ] && exit 0
        exit 7
      fi
      exit 0`);
    mock("systemd-run", `
      case " $* " in *" --setenv=MIXRELAY_IPC_DIR=$LAB_MOCK_ROOT/ipc "*) ;; *) exit 99 ;; esac
      echo start >> "$LAB_MOCK_ROOT/lifecycle"
      [ "$LAB_SCENARIO" != start-failed ]`);
    mock("systemctl", `echo "$*" >> "$LAB_MOCK_ROOT/lifecycle"`);
    mock("node", `
      [ "$MIXRELAY_IPC_DIR" = "$LAB_MOCK_ROOT/ipc" ] || exit 99
      echo probe >> "$LAB_MOCK_ROOT/lifecycle"
      [ "$LAB_SCENARIO" != probe-failed ]`);
    const result = spawnSync("bash", [pipewireCheck, join(bin, "engine"), join(bin, "node"), "/unused-probe"], {
      encoding: "utf8", timeout: 5000,
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, LAB_MOCK_ROOT: directory, LAB_SCENARIO: scenario, MIXRELAY_IPC_DIR: join(directory, "ipc") },
    });
    assert.equal(result.error, undefined, `${result.stderr}\n${result.stdout}`);
    assert.equal(result.status, { occupied: 2, "start-failed": 1, "probe-failed": 1, success: 0 }[scenario], result.stderr);
    if (scenario === "occupied") {
      assert.match(result.stderr, /Refusing to test over an existing engine/);
      assert.throws(() => readFileSync(join(directory, "lifecycle")), { code: "ENOENT" });
    } else {
      const lifecycle = readFileSync(join(directory, "lifecycle"), "utf8").trim().split("\n");
      assert.deepEqual(lifecycle, scenario === "start-failed" ? ["start"]
        : Array.from({ length: 3 }, () => ["start", "probe", "--user stop mixrelay-lab-engine"]).flat());
    }
  });
}


test("guest smoke startup resolves its actual inline module imports", () => {
  const source = readFileSync(new URL("../e2e/guest-session.sh", import.meta.url), "utf8");
  const smoke = source.split("<<'JS'\n")[1]?.split("\nJS")[0];
  assert.ok(smoke, "guest smoke program exists");
  const imports = smoke.split("\n").filter(line => line.startsWith("import ")).join("\n");
  assert.ok(imports, "guest smoke program has module imports");
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", imports], {
    cwd: fileURLToPath(new URL("../../", import.meta.url)), encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
});
