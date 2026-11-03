export const DESKTOP_CODE_CHECK_TAURI_CONFIG = '{"bundle":{"externalBin":[],"resources":[]}}';

export function desktopCodeCheckEnvironment(env) {
  if (env.TAURI_CONFIG !== undefined && env.TAURI_CONFIG !== DESKTOP_CODE_CHECK_TAURI_CONFIG) {
    throw new Error("TAURI_CONFIG conflicts with the canonical desktop code-check configuration");
  }
  return {
    ...env,
    TAURI_CONFIG: DESKTOP_CODE_CHECK_TAURI_CONFIG,
    MIXRELAY_DESKTOP_CODE_CHECK: "1",
  };
}

export function assertDesktopPackagingAllowed(env) {
  if (env.MIXRELAY_DESKTOP_CODE_CHECK !== undefined
    || env.TAURI_CONFIG === DESKTOP_CODE_CHECK_TAURI_CONFIG) {
    throw new Error("Desktop packaging is disabled in the code-check environment");
  }
}
