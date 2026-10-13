import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { dirname, join, relative, sep } from 'node:path';
import { MAX_FILE_BYTES, MAX_FILES, MAX_OUTPUT_BYTES, MAX_TOTAL_BYTES } from '../web/config.js';
import { PROJECT_ROOT, createRunnerHarness, loadCatwalkModule } from './web/runner-support.js';

const corpusRoot = join(PROJECT_ROOT, 'tests/fixtures/formats');
const reportPath = join(PROJECT_ROOT, 'target/web-test-results/wasm-corpus.json');
const size = 32;
const results = [];
let harness;
let module;
let cases;
let advertised;
let nativeGeometry;

function compareText(left, right) { return left < right ? -1 : left > right ? 1 : 0; }
function extensions(source, name) {
  const declaration = source.match(new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`));
  assert.ok(declaration, `missing Rust extension registry ${name}`);
  return [...declaration[1].matchAll(/"([a-z0-9]+)"/g)].map(match => match[1]);
}
function assertCorpusPath(path) {
  assert.ok(typeof path === 'string' && !path.includes('\\')
    && path.split('/').every(part => part !== '' && part !== '.' && part !== '..'),
  `fixture path must stay relative to the corpus: ${path}`);
}

before(async () => {
  const [manifest, formats, previews] = await Promise.all([
    readFile(join(corpusRoot, 'manifest.json'), 'utf8'),
    readFile(join(PROJECT_ROOT, 'src/formats/mod.rs'), 'utf8'),
    readFile(join(PROJECT_ROOT, 'src/saved_preview.rs'), 'utf8'),
  ]);
  advertised = [...new Set([
    ...extensions(formats, 'MODEL_EXTENSIONS'), ...extensions(formats, 'IMAGE_EXTENSIONS'),
    ...extensions(previews, 'EXTENSIONS'),
  ])].sort(compareText);
  nativeGeometry = extensions(formats, 'NATIVE_GEOMETRY_EXTENSIONS').sort(compareText);
  cases = await Promise.all(JSON.parse(manifest).cases.map(async entry => {
    assertCorpusPath(entry.input);
    const metadata = await stat(join(corpusRoot, entry.input));
    assert.ok(metadata.isFile(), `fixture is not a file: ${entry.input}`);
    return { ...entry, sourceBytes: metadata.size };
  }));
  assert.deepEqual([...new Set(cases.map(entry => entry.extension))].sort(compareText), advertised,
    'the positive corpus must cover exactly the advertised extensions');
  assert.deepEqual([...new Set(cases.filter(entry => entry.kind === 'native-geometry')
    .map(entry => entry.extension))].sort(compareText), nativeGeometry,
  'each native geometry alias needs a positive fresh-render case');
  harness = await createRunnerHarness();
  module = await loadCatwalkModule();
});

after(async () => {
  await harness?.close();
  await mkdir(dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify({
    dimensions: [size, size], advertisedExtensions: advertised, nativeGeometryExtensions: nativeGeometry,
    passed: results.filter(result => result.status === 'passed').length,
    total: results.length, cases: results,
  }, null, 2)}\n`);
});

function smallestCase(extension, mode) {
  const candidates = cases.filter(entry => entry.extension === extension
    && (mode !== 'geometry' || entry.kind === 'native-geometry'));
  assert.ok(candidates.length, `no positive ${mode} fixture for .${extension}`);
  return candidates.sort((left, right) => left.sourceBytes - right.sourceBytes
    || compareText(left.input, right.input) || compareText(left.id, right.id))[0];
}

// Preserve each input's relative directory and every named companion in that fixture directory.
async function companionFiles(entry) {
  const paths = [];
  async function collect(directory) {
    const children = await readdir(directory, { withFileTypes: true });
    for (const child of children.sort((left, right) => compareText(left.name, right.name))) {
      const path = join(directory, child.name);
      if (child.isDirectory()) await collect(path);
      else {
        assert.ok(child.isFile(), `nonregular fixture companion: ${relative(corpusRoot, path)}`);
        const metadata = await stat(path);
        assert.ok(metadata.size <= MAX_FILE_BYTES, `companion exceeds per-file browser cap: ${path}`);
        paths.push({ path, size: metadata.size });
      }
    }
  }
  await collect(dirname(join(corpusRoot, entry.input)));
  assert.ok(paths.length <= MAX_FILES, `companions exceed browser file-count cap for ${entry.id}`);
  const total = paths.reduce((sum, path) => sum + path.size, 0);
  assert.ok(total <= MAX_TOTAL_BYTES, `companions exceed total browser byte cap for ${entry.id}`);
  const files = await Promise.all(paths.map(async ({ path, size: expectedSize }) => {
    const bytes = await readFile(path);
    assert.equal(bytes.byteLength, expectedSize, `fixture changed during the test: ${path}`);
    return { name: relative(corpusRoot, path).split(sep).join('/'),
      bytes: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) };
  }));
  return { files, total };
}

async function renderCase(entry, mode) {
  const record = { extension: entry.extension, id: entry.id, input: entry.input, mode,
    sourceBytes: entry.sourceBytes };
  try {
    const { files, total } = await companionFiles(entry);
    record.fileCount = files.length;
    record.totalInputBytes = total;
    const result = await harness.runner.runCatwalk(module, files, entry.input,
      { size, transparent: true, mode });
    assert.ok(result.png.byteLength > 24 && result.png.byteLength <= MAX_OUTPUT_BYTES);
    assert.deepEqual([...result.png.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
    assert.equal(Buffer.from(result.png.subarray(12, 16)).toString('ascii'), 'IHDR');
    const header = new DataView(result.png.buffer, result.png.byteOffset, result.png.byteLength);
    assert.deepEqual([header.getUint32(16), header.getUint32(20)], [size, size]);
    record.pngBytes = result.png.byteLength;
    record.status = 'passed';
  } catch (error) {
    record.status = 'failed';
    record.error = error.message;
    record.stderr = error.stderr;
    record.exitCode = error.exitCode;
    throw new Error(`WASI ${mode} .${entry.extension}: ${entry.id} (${entry.input}): ${error.message}`,
      { cause: error });
  } finally {
    results.push(record);
  }
}

test('actual WASI auto thumbnails cover every advertised extension', async context => {
  for (const extension of advertised) {
    const entry = smallestCase(extension, 'auto');
    await context.test(`.${extension}: ${entry.id}`, () => renderCase(entry, 'auto'));
  }
});

test('actual WASI fresh geometry covers every native geometry alias', async context => {
  for (const extension of nativeGeometry) {
    const entry = smallestCase(extension, 'geometry');
    await context.test(`.${extension}: ${entry.id}`, () => renderCase(entry, 'geometry'));
  }
});
