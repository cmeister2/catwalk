/** Native release targets, kept in one place for CI, packaging and publication. */
export const TARGETS = Object.freeze([
  { target: 'x86_64-unknown-linux-gnu', runner: 'ubuntu-24.04', binary: 'catwalk', musl: false },
  { target: 'aarch64-unknown-linux-gnu', runner: 'ubuntu-24.04-arm', binary: 'catwalk', musl: false },
  { target: 'x86_64-unknown-linux-musl', runner: 'ubuntu-24.04', binary: 'catwalk', musl: true },
  { target: 'aarch64-unknown-linux-musl', runner: 'ubuntu-24.04-arm', binary: 'catwalk', musl: true },
  { target: 'x86_64-apple-darwin', runner: 'macos-15-intel', binary: 'catwalk', musl: false },
  { target: 'aarch64-apple-darwin', runner: 'macos-15', binary: 'catwalk', musl: false },
  { target: 'x86_64-pc-windows-msvc', runner: 'windows-2025', binary: 'catwalk.exe', musl: false },
].map(Object.freeze));

/** Admit only canonical SemVer values that can safely appear in filenames. */
export function validateVersion(version) {
  const valid = typeof version === 'string' && version.length <= 128 && version.trim() === version
    && /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\dA-Za-z-]+(?:\.[\dA-Za-z-]+)*))?(?:\+[\dA-Za-z-]+(?:\.[\dA-Za-z-]+)*)?$/.test(version);
  const prerelease = valid && version.split('+', 1)[0].split('-').slice(1).join('-');
  if (!valid || (prerelease && prerelease.split('.').some(
    (part) => /^\d+$/.test(part) && part.length > 1 && part.startsWith('0')
  ))) {
    throw new Error(`Invalid release version: ${version}`);
  }
  return version;
}

/** Require the full lowercase Git commit recorded by the producing checkout. */
export function validateCommit(commit) {
  if (typeof commit !== 'string' || commit.length !== 40 || !/^[\da-f]{40}$/.test(commit)) {
    throw new Error('Expected a full lowercase Git commit SHA.');
  }
  return commit;
}

/** Resolve an explicitly supported target instead of accepting arbitrary paths. */
export function targetDefinition(target) {
  const definition = TARGETS.find((entry) => entry.target === target);
  if (!definition) throw new Error(`Unsupported release target: ${target}`);
  return definition;
}

/** Produce the canonical archive name used in checksums and GitHub assets. */
export function archiveName(version, target) {
  validateVersion(version);
  targetDefinition(target);
  return `catwalk3d-${version}-${target}.zip`;
}
