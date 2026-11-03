import assert from "node:assert/strict";
import test from "node:test";
import { vmArguments } from "./vm.mjs";

const uuid = "00000000-1111-2222-3333-444444444444";
test("VMs are addressed by exact UUID", () => {
  assert.deepEqual(vmArguments("list"), ["list"]);
  assert.deepEqual(vmArguments("start", uuid), ["start", uuid]);
  assert.deepEqual(vmArguments("status", uuid), ["status", uuid]);
});
test("shutdown never uses UTM's forced-stop default", () => {
  assert.deepEqual(vmArguments("shutdown", uuid), ["stop", "--request", uuid]);
});
test("destructive commands, names, and option injection are rejected", () => {
  for (const action of ["delete", "kill", "stop", "reset"]) assert.throws(() => vmArguments(action, uuid));
  for (const id of [undefined, "Linux", "--disposable", `${uuid}; echo no`]) {
    assert.throws(() => vmArguments("start", id));
  }
});
