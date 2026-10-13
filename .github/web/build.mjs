import { spawnSync } from 'node:child_process';
import { cp, copyFile, mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { gzip } from 'node:zlib';
import { build } from 'esbuild';
import { generateWasiLibcNotices } from './notices.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const args = process.argv.slice(2);
const outputIndex = args.indexOf('--output');
const output = resolve(root, outputIndex < 0 ? 'target/web' : args[outputIndex + 1]);
const skipCompile = args.includes('--skip-compile');
const noticesIndex = args.indexOf('--notices');
const notices = noticesIndex < 0 ? null : resolve(root, args[noticesIndex + 1]);
const targetDir = resolve(root, process.env.CARGO_TARGET_DIR || 'target');
const wasm = join(targetDir, 'wasm32-wasip1/web/catwalk.wasm');

if (!skipCompile) {
  const result = spawnSync('cargo', [
    'rustc', '--profile', 'web', '--locked', '--target', 'wasm32-wasip1', '--bin', 'catwalk',
    '--', '-C', 'link-arg=--max-memory=536870912',
  ], {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      // Zstd supplies its own wasm stdlib/string shim. Use Clang's freestanding
      // integer/limits headers rather than accidentally selecting host libc.
      CFLAGS_wasm32_wasip1: `${process.env.CFLAGS_wasm32_wasip1 || ''} -ffreestanding`.trim(),
    },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status || 1);
}

await mkdir(output, { recursive: true });
await cp(join(root, 'web'), output, { recursive: true });
await mkdir(join(output, 'vendor'), { recursive: true });
await build({
  stdin: { contents: "export * from '@bjorn3/browser_wasi_shim';", resolveDir: root, loader: 'js' },
  outfile: join(output, 'vendor/wasi.js'),
  bundle: true,
  format: 'esm',
  platform: 'browser',
  target: 'es2022',
  minify: true,
  legalComments: 'linked',
});
await copyFile(wasm, join(output, 'catwalk.wasm'));
const wasmBytes = (await stat(wasm)).size;
if (wasmBytes > 90 * 1024 * 1024) throw new Error('Browser module exceeds the Pages file budget');
const compressed = await promisify(gzip)(await readFile(wasm), { level: 9 });
await writeFile(join(output, 'catwalk.wasm.gz'), compressed);

const shimLicense = await readFile(join(root, 'node_modules/@bjorn3/browser_wasi_shim/LICENSE-MIT'), 'utf8');
const catwalkLicense = await readFile(join(root, 'LICENSE'), 'utf8');
await writeFile(join(output, 'licenses.txt'), `Catwalk\n\n${catwalkLicense}\n\n@bjorn3/browser_wasi_shim 0.4.2\n\n${shimLicense}`);
if (notices) {
  for (const name of ['THIRD_PARTY_LICENSES.html', 'RUST-LICENSES.html', 'MUSL-COPYRIGHT']) {
    await copyFile(join(notices, name), join(output, name));
  }
  const runtime = await generateWasiLibcNotices(output, notices);
  const escape = value => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  const runtimeText = await readFile(join(output, runtime.filename), 'utf8');
  const muslText = await readFile(join(output, 'MUSL-COPYRIGHT'), 'utf8');
  const thirdPartyPath = join(output, 'THIRD_PARTY_LICENSES.html');
  const thirdParty = await readFile(thirdPartyPath, 'utf8');
  const section = `<section id="wasi-runtime"><h2>WASI C runtime</h2>\n` +
    `<p><a href="./${runtime.filename}">Original runtime notices</a> and ` +
    '<a href="./MUSL-COPYRIGHT">musl copyright</a></p>\n' +
    `<pre>${escape(runtimeText)}</pre>\n<pre>${escape(muslText)}</pre>\n</section>\n`;
  await writeFile(thirdPartyPath, thirdParty.replace(/<\/html>\s*$/, section + '</html>\n'));
}
await writeFile(join(output, 'build.json'), JSON.stringify({ wasmBytes, gzipBytes: compressed.length, target: 'wasm32-wasip1' }) + '\n');
console.log(`Browser uploader: ${output} (${(compressed.length / 1024 / 1024).toFixed(2)} MiB download; ${(wasmBytes / 1024 / 1024).toFixed(2)} MiB WASM)`);
