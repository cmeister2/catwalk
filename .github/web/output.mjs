import { readdir } from 'node:fs/promises';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';

const requiredFiles = [
  'index.html', 'app.js', 'config.js', 'style.css', 'worker.js', 'wasi-runner.js',
  'vendor/wasi.js', 'catwalk.wasm', 'catwalk.wasm.gz', 'licenses.txt', 'build.json',
];
const noticeFiles = [
  'THIRD_PARTY_LICENSES.html', 'RUST-LICENSES.html', 'MUSL-COPYRIGHT', 'WASI-LIBC-LICENSES.txt',
];
const allowedFiles = new Set([...requiredFiles, ...noticeFiles, 'vendor/wasi.js.LEGAL.txt']);

/** Cargo writes host build dependencies into <target-dir>/web for this profile. */
export function assertSeparateDirectories(output, targetDir) {
  const contains = (parent, child) => {
    const path = relative(resolve(parent), resolve(child));
    return path === '' || (!isAbsolute(path) && path !== '..' && !path.startsWith(`..${sep}`));
  };
  if (contains(output, targetDir) || contains(targetDir, output)) {
    throw new Error('The page output and CARGO_TARGET_DIR must be separate directories. Use target/web-build for Cargo and target/web for the page.');
  }
}

/** Refuse to publish compiler artifacts, stale files or incomplete runtime assets. */
export async function verifyWebOutput(directory, { requireNotices = false } = {}) {
  const found = new Set();
  async function visit(path, prefix = '') {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const name = prefix + entry.name;
      if (entry.isDirectory() && name === 'vendor') {
        await visit(join(path, entry.name), 'vendor/');
      } else if (entry.isFile() && allowedFiles.has(name)) {
        found.add(name);
      } else {
        throw new Error(`Unexpected file in browser output: ${name}`);
      }
    }
  }
  await visit(directory);
  for (const name of [...requiredFiles, ...(requireNotices ? noticeFiles : [])]) {
    if (!found.has(name)) throw new Error(`Missing browser runtime asset: ${name}`);
  }
  return [...found].sort();
}
