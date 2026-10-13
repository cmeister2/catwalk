import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { buildSync } from 'esbuild';

export const PROJECT_ROOT = fileURLToPath(new URL('../../', import.meta.url));
let modulePromise;

/** Exercise the current source runner against the same pinned shim used by the site. */
export async function createRunnerHarness({ withWorker = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'catwalk-wasi-tests-'));
  const close = () => rm(directory, { recursive: true, force: true });
  try {
    await mkdir(join(directory, 'vendor'));
    await writeFile(join(directory, 'package.json'), '{"type":"module"}\n');
    for (const name of ['wasi-runner.js', 'config.js', ...(withWorker ? ['worker.js'] : [])]) {
      await copyFile(join(PROJECT_ROOT, 'web', name), join(directory, name));
    }
    buildSync({
      entryPoints: [fileURLToPath(import.meta.resolve('@bjorn3/browser_wasi_shim'))],
      outfile: join(directory, 'vendor/wasi.js'), bundle: true, format: 'esm', platform: 'browser',
    });
    const runner = await import(pathToFileURL(join(directory, 'wasi-runner.js')).href);
    const shim = await import(pathToFileURL(join(directory, 'vendor/wasi.js')).href);
    return { directory, runner, shim, close };
  } catch (error) {
    await close();
    throw error;
  }
}

/** Use the actual build artifact; a missing build must fail rather than skip coverage. */
export function loadCatwalkModule() {
  modulePromise ??= readFile(process.env.CATWALK_WASM_PATH ?? join(PROJECT_ROOT, 'target/web/catwalk.wasm'))
    .then(bytes => WebAssembly.compile(bytes));
  return modulePromise;
}
