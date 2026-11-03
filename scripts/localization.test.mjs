import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";

const languages = JSON.parse(readFileSync(new URL("../packaging/localization/languages.json", import.meta.url), "utf8"));
const removal = JSON.parse(readFileSync(new URL("../packaging/localization/linux-removal.json", import.meta.url), "utf8"));

test("localization metadata shares one product notice without translated agreements", () => {
  assert.deepEqual(languages.map(entry => entry.language).sort(), ["de", "en", "es", "fr", "it", "ja", "ko", "pt", "ru", "zh-Hans", "zh-Hant"]);
  assert.deepEqual(readdirSync(new URL("../", import.meta.url)).filter(name => /^LICENSE.*\.txt$/.test(name)), ["LICENSE.txt"]);
  for (const entry of languages) {
    for (const key of ["language", "uiLocale", "name", "guideTitle", "beforeHeading", "afterHeading"]) assert.ok(entry[key], `${entry.language}: ${key}`);
    for (const key of ["appTerms", "driverTerms", "agreementTitle", "businessHeading"]) assert.ok(!(key in entry), `${entry.language}: no obsolete ${key}`);
  }
});

test("English product text uses British spelling", () => {
  const american = /\b(?:[Ll]icenses?\b(?! a\b)|[Bb]ehaviors?\b|[Cc]olors?\b|[Cc]atalogs?\b|[Aa]nalyz(?:e|es|ed|ing)\b|[Cc]enters?\b|[Tt]oward\b|(?:[Ff]avo|[Hh]ono|[Ll]abo|[Nn]eighbo|[Hh]umo|[Rr]umo|[Ff]lavo|[Ee]ndeavo)r(?:s|ed|ing|able|ably|ite|ites|hood)?\b)/;
  const read = path => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
  const texts = [
    ["apps/control-ui/src/core/i18n/locales/en.ts", [...read("apps/control-ui/src/core/i18n/locales/en.ts").matchAll(/'((?:[^'\\\n]|\\.)*)'/g)].map(match => match[1]).join("\n")],
    ["packaging/localization/languages.json", JSON.stringify(languages.find(entry => entry.language === "en"))],
    ["packaging/localization/linux-lifecycle.json", JSON.parse(read("packaging/localization/linux-lifecycle.json")).en.join("\n")],
  ];
  for (const [path, text] of texts) {
    const match = text.match(american);
    assert.equal(match, null, `${path}: American spelling "${match?.[0]}"`);
  }
});

test("macOS declares all UI languages and bundles localized permission descriptions", () => {
  const config = JSON.parse(readFileSync(new URL("../apps/desktop/src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  const plist = readFileSync(new URL("../apps/desktop/src-tauri/Info.plist", import.meta.url), "utf8");
  const localizations = plist.match(/<key>CFBundleLocalizations<\/key>\s*<array>([\s\S]*?)<\/array>/)?.[1];
  assert.ok(localizations);
  assert.deepEqual([...localizations.matchAll(/<string>(.*?)<\/string>/g)].map(match => match[1]).sort(), languages.map(entry => entry.language).sort());
  for (const {language} of languages) {
    const filename = `packaging/macos/localization/${language}.lproj/InfoPlist.strings`;
    const source = readFileSync(new URL(`../${filename}`, import.meta.url), "utf8");
    assert.match(source, /NSAudioCaptureUsageDescription = ".+";/);
    assert.match(source, /NSMicrophoneUsageDescription = ".+";/);
    assert.equal(config.bundle.resources[`../../../${filename}`], `${language}.lproj/InfoPlist.strings`);
  }
});

test("Linux removal selects actual translated messages and Chinese scripts", () => {
  const script = readFileSync(new URL("../packaging/linux/uninstall.sh", import.meta.url), "utf8");
  const start = script.indexOf('case "${LC_ALL:');
  const end = script.indexOf("\nesac", start) + 6;
  assert.ok(start >= 0 && end > start);
  for (const [locale, resolved] of [
    ["es_MX.UTF-8", "es"], ["pt_BR.UTF-8", "pt"], ["pt_PT.UTF-8", "pt"], ["it_IT.UTF-8", "it"], ["ru_RU.UTF-8", "ru"], ["ja_JP.UTF-8", "ja"], ["fr_CA.UTF-8", "fr"], ["de_DE.UTF-8", "de"],
    ["zh_CN.UTF-8", "zh-Hans"], ["zh_SG.UTF-8", "zh-Hans"],
    ["zh_TW.UTF-8", "zh-Hant"], ["zh_HK.UTF-8", "zh-Hant"],
    ["zh_Hans_TW", "zh-Hans"], ["zh-Hant-CN", "zh-Hant"],
  ]) {
    const expected = removal[resolved].app;
    const command = `${script.slice(start, end)}\nprintf '%s\\000' ${Object.keys(expected).map(key => `"$${key}"`).join(" ")}`;
    const result = spawnSync("/bin/sh", ["-c", command], { env: { PATH: "/usr/bin:/bin", LC_ALL: locale }, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.split("\0").slice(0, -1), Object.values(expected), `uninstall.sh: ${locale}`);
  }
  for (const locale of ["ko_KR.UTF-8", "en_US.UTF-8", "nl_NL.UTF-8"]) {
    const result = spawnSync("/bin/sh", ["-c", `${script.slice(start, end)}\nprintf '%s' "$title"`], { env: { LC_ALL: locale }, encoding: "utf8" });
    assert.equal(result.status, 0);
    assert.ok(result.stdout.includes("MixRelay"));
    assert.ok(locale.startsWith("ko") ? result.stdout.includes("제거") : result.stdout.includes("Uninstall"));
  }
});

test("Linux install/update/removal prerequisites use every supported language", () => {
  const messages = JSON.parse(readFileSync(new URL("../packaging/localization/linux-lifecycle.json", import.meta.url), "utf8"));
  for (const [index, filename] of ["driver-before-change.sh", "session-start.sh"].entries()) {
    const source = readFileSync(new URL(`../packaging/linux/${filename}`, import.meta.url), "utf8");
    const start = source.indexOf('case "${LC_ALL:');
    const end = source.indexOf("\nesac", start) + 6;
    for (const {language} of languages) {
      const result = spawnSync("/bin/sh", ["-c", `${source.slice(start, end)}\nprintf '%s' "$instruction"`], { env: { LC_ALL: language }, encoding: "utf8" });
      assert.equal(result.status, 0);
      assert.equal(result.stdout, messages[language][index]);
    }
    assert.match(source.slice(end), /printf '%s\\n' "\$instruction"/);
  }
});
