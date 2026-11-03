import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const executable = "/Applications/UTM.app/Contents/MacOS/utmctl";

export function vmArguments(action, uuid) {
  if (action === "list" && uuid === undefined) return ["list"];
  if (!["start", "status", "shutdown"].includes(action)) {
    throw new Error("Usage: node scripts/lab/vm.mjs list | start|status|shutdown <UUID>");
  }
  if (!/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(uuid ?? "")) {
    throw new Error("Use the exact VM UUID from lab:vm list; names and option strings are not accepted.");
  }
  // utmctl's default stop is a forced power-off. Always request a guest shutdown,
  // never kill the VM or discard disk writes. Guest refusal remains visible.
  return action === "shutdown" ? ["stop", "--request", uuid] : [action, uuid];
}

function main() {
  const args = vmArguments(process.argv[2], process.argv[3]);
  if (process.argv.length > 4) throw new Error("Unexpected arguments");
  if (process.platform !== "darwin") throw new Error("This VM controller runs on the macOS host.");
  if (!existsSync(executable)) throw new Error("UTM is not installed. Install UTM before starting the lab.");
  const result = spawnSync(executable, args, { stdio: "inherit", timeout: 60_000 });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
