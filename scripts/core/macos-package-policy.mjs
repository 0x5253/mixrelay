import { relative, resolve, sep } from "node:path";

const isWithin = (path, parent) => path === parent || path.startsWith(`${parent}${sep}`);

/**
 * Applies the fixed staging policy to every bundle discovered by pkgbuild --analyze.
 *
 * Installer bundle relocation can otherwise move a payload bundle onto a copy the user
 * previously moved elsewhere, bypassing the transaction's exact staging paths. Keep each
 * analyzer-provided path intact, require the expected manager and HAL roots, disable relocation
 * and version skipping, and require the analyzed bundle identity at its fixed staging path.
 */
export function fixedComponentBundlePolicy(componentPlist, packageRoot, requiredBundleRoots) {
  if (!Array.isArray(componentPlist)) {
    throw new Error("pkgbuild component analysis must be an array");
  }
  const allowedRoots = requiredBundleRoots.map(path => resolve(path));
  const seen = new Set();
  const bundlePaths = [];
  const visit = (bundle) => {
    if (!bundle || typeof bundle !== "object" || Array.isArray(bundle)) {
      throw new Error("pkgbuild component analysis contains an invalid bundle entry");
    }
    const relativeBundlePath = bundle.RootRelativeBundlePath;
    if (typeof relativeBundlePath !== "string" || relativeBundlePath.length === 0) {
      throw new Error("pkgbuild component analysis omitted a bundle's fixed staging path");
    }
    const bundlePath = resolve(packageRoot, relativeBundlePath);
    const canonicalRelativePath = relative(packageRoot, bundlePath).split(sep).join("/");
    if (canonicalRelativePath !== relativeBundlePath || !isWithin(bundlePath, resolve(packageRoot))) {
      throw new Error(`pkgbuild reported an unsafe bundle staging path: ${relativeBundlePath}`);
    }
    if (!allowedRoots.some(root => isWithin(bundlePath, root))) {
      throw new Error(`pkgbuild discovered a bundle outside the fixed manager/HAL payloads: ${relativeBundlePath}`);
    }
    if (seen.has(relativeBundlePath)) {
      throw new Error(`pkgbuild discovered a duplicate bundle path: ${relativeBundlePath}`);
    }
    seen.add(relativeBundlePath);
    bundlePaths.push(relativeBundlePath);

    bundle.BundleIsRelocatable = false;
    bundle.BundleIsVersionChecked = false;
    bundle.BundleHasStrictIdentifier = true;
    bundle.BundleOverwriteAction = "upgrade";
    if (bundle.ChildBundles !== undefined) {
      if (!Array.isArray(bundle.ChildBundles)) {
        throw new Error(`pkgbuild returned invalid child bundles for ${relativeBundlePath}`);
      }
      for (const child of bundle.ChildBundles) visit(child);
    }
  };
  for (const bundle of componentPlist) visit(bundle);

  for (const root of allowedRoots) {
    if (!seen.has(relative(packageRoot, root).split(sep).join("/"))) {
      throw new Error(`pkgbuild did not discover required release bundle: ${root}`);
    }
  }
  return { componentPlist, bundlePaths };
}

export function verifyPackageSignature(signature, signingIdentity, mode) {
  const signatureStatus = signature.match(/^[ \t]*Status:[ \t]*([^\r\n]+)\r?$/m)?.[1].trim();
  const trustedStatus = signatureStatus === "signed by a certificate trusted by macOS"
    || signatureStatus === "signed by a developer certificate issued by Apple for distribution";
  const signingLeaf = signature.match(/^[ \t]*Certificate Chain:[ \t]*\r?\n[ \t]*1\.[ \t]+([^\r\n]+)\r?$/m)?.[1].trim();
  const trustedTimestamp = /^[ \t]*Signed with a trusted timestamp on:[ \t]+\S[^\r\n]*\r?$/m.test(signature);
  if (!trustedStatus || signingLeaf !== signingIdentity || !trustedTimestamp) {
    throw new Error(`${mode} product signature is not trusted with the requested Developer ID Installer identity:\n${signature}`);
  }
}
