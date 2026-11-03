import { readFileSync } from "node:fs";

const header = readFileSync(new URL("../native/macos/installer/MRInstallContract.h", import.meta.url), "utf8");
function expand(name, depth = 0) {
  if (depth > 8) throw new Error("Recursive macOS installation contract macro");
  let expression = header.split("\n").find(line => line.startsWith(`#define ${name} `))?.slice(`#define ${name} `.length);
  if (expression === undefined) throw new Error(`Missing macOS installation contract macro: ${name}`);
  let value = "";
  while (expression.trim()) {
    const token = expression.trimStart().match(/^(?:"([^"]*)"|([A-Z_]+))/);
    if (!token) throw new Error(`Invalid macOS installation contract macro: ${name}`);
    value += token[1] ?? expand(token[2], depth + 1);
    expression = expression.trimStart().slice(token[0].length);
  }
  return value;
}

export const PRODUCT_DIRECTORY = expand("MR_PRODUCT_DIRECTORY");
export const APP_BUNDLE_NAME = expand("MR_APP_BUNDLE_NAME");
export const UNINSTALLER_BUNDLE_NAME = expand("MR_UNINSTALLER_BUNDLE_NAME");
export const APP_PATH = expand("MR_APP_PATH");
export const UNINSTALLER_PATH = expand("MR_UNINSTALLER_PATH");
export const REMOVAL_HELPER_PATH = expand("MR_REMOVAL_HELPER_PATH");
