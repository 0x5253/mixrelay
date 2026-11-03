import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import test from "node:test";

const source = fileURLToPath(new URL("../native/macos/common/MRDriverRuntimeLauncher.c", import.meta.url));

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "mixrelay-launcher-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const wrapper = join(directory, "fixture.c");
  const executable = join(directory, "launcher");
  const events = join(directory, "events");
  writeFileSync(wrapper, `
#define execv fixture_execv
#include ${JSON.stringify(source)}
#undef execv
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
static volatile sig_atomic_t received;
static void fixture_stop(int signal_number) { (void)signal_number; received=1; }
int fixture_execv(const char *path, char *const arguments[]) {
  if(strcmp(path,"/Applications/MixRelay/MixRelay.app/Contents/MacOS/mixrelay-desktop") ||
     strcmp(arguments[1],"--driver-runtime") || arguments[2]) _exit(99);
  if(getenv("MR_TEST_EXIT")) _exit(23);
  struct sigaction action={0};action.sa_handler=fixture_stop;sigemptyset(&action.sa_mask);
  sigset_t watched,previous;sigemptyset(&watched);sigaddset(&watched,SIGTERM);
  sigprocmask(SIG_BLOCK,&watched,&previous);
  sigaction(SIGTERM,&action,NULL);
  int fd=open(getenv("MR_TEST_EVENTS"),O_WRONLY|O_CREAT|O_APPEND,0600);
  if(fd<0) _exit(98);
  dprintf(fd,"ready %d\\n",getpid());
  while(!received) sigsuspend(&previous);
  dprintf(fd,"stopped\\n");close(fd);_exit(0);
}
`);
  const build = spawnSync("cc", ["-Wall", "-Wextra", "-Werror", wrapper, "-o", executable], { encoding: "utf8" });
  assert.equal(build.status, 0, build.stderr);
  return { executable, events };
}

async function start(t, paths, environment = {}) {
  const child = spawn(paths.executable, [], {
    detached: true,
    stdio: "ignore",
    env: { ...process.env, MR_TEST_EVENTS: paths.events, ...environment },
  });
  const exited = once(child, "exit");
  t.after(() => {
    if (child.exitCode === null && child.signalCode === null) {
      try { process.kill(-child.pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") throw error; }
    }
  });
  return { child, exited };
}

for (const signal of ["SIGTERM", "SIGHUP"]) {
  test(`${signal} forwards graceful stop and waits for the runtime`, { timeout: 8000 }, async (t) => {
    const paths = fixture(t);
    const { child, exited } = await start(t, paths);
    const deadline = Date.now() + 4000;
    while (!(existsSync(paths.events) && readFileSync(paths.events, "utf8").includes("ready"))) {
      assert.ok(Date.now() < deadline, "runtime did not become ready");
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    process.kill(child.pid, signal);
    assert.deepEqual(await exited, [0, null]);
    assert.match(readFileSync(paths.events, "utf8"), /ready \d+\nstopped\n/);
  });
}

test("runtime exit is reaped and its failure reaches the editor", { timeout: 8000 }, async (t) => {
  const paths = fixture(t);
  const { exited } = await start(t, paths, { MR_TEST_EXIT: "1" });
  assert.deepEqual(await exited, [23, null]);
});

test("unexpected arguments cannot launch a runtime", (t) => {
  const paths = fixture(t);
  const result = spawnSync(paths.executable, ["unexpected"], { env: { ...process.env, MR_TEST_EVENTS: paths.events } });
  assert.equal(result.status, 64);
  assert.equal(existsSync(paths.events), false);
});
