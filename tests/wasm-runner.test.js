import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { gzipSync } from 'node:zlib';
import { PROJECT_ROOT as root, createRunnerHarness, loadCatwalkModule as actualModule } from './web/runner-support.js';

const encoder = new TextEncoder();
const png = new Uint8Array(Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64',
));
let temporary;
let runner;
let shim;
let harness;

before(async () => {
  harness = await createRunnerHarness({ withWorker: true });
  ({ directory: temporary, runner, shim } = harness);
});
after(async () => { await harness?.close(); });

function file(name, bytes = new Uint8Array([1])) {
  return { name, bytes: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) };
}
function textFile(name, text) { return file(name, encoder.encode(text)); }

function unsigned(value) {
  const bytes = [];
  do {
    let byte = value & 127;
    value = Math.floor(value / 128);
    if (value) byte |= 128;
    bytes.push(byte);
  } while (value);
  return bytes;
}
function signed(value) {
  const bytes = [];
  while (true) {
    const byte = value & 127;
    value >>= 7;
    const done = (value === 0 && !(byte & 64)) || (value === -1 && (byte & 64));
    bytes.push(byte | (done ? 0 : 128));
    if (done) return bytes;
  }
}
function string(value) { const bytes = [...encoder.encode(value)]; return [...unsigned(bytes.length), ...bytes]; }
function section(id, bytes) { return [id, ...unsigned(bytes.length), ...bytes]; }
function i32(value) { return [0x41, ...signed(value)]; }

/** A real WASI guest exercises byte capture, process exits, traps and memory isolation. */
function guestBytes({ output = png, outputLength = output.length, error = '', exit = 0, trap = false, mutate = false, argumentBytes } = {}) {
  const stderr = encoder.encode(error);
  const outputOffset = 128;
  const errorOffset = outputOffset + Math.max(output.length, outputLength) + 8;
  const pages = Math.ceil((errorOffset + stderr.length + 1) / 65536);
  const descriptor = new Uint8Array(24);
  const view = new DataView(descriptor.buffer);
  view.setUint32(0, outputOffset, true);
  view.setUint32(4, outputLength, true);
  view.setUint32(16, errorOffset, true);
  view.setUint32(20, stderr.length, true);
  const instructions = [];
  if (argumentBytes !== undefined) {
    instructions.push(...i32(64), ...i32(68), 0x10, 2, 0x1a,
      ...i32(68), 0x28, 2, 0, ...i32(argumentBytes), 0x47, 0x04, 0x40, 0x00, 0x0b);
  }
  function write(fd, pointer) {
    instructions.push(...i32(fd), ...i32(pointer), ...i32(1), ...i32(32), 0x10, 0, 0x1a);
  }
  if (outputLength) write(1, 0);
  if (stderr.length) write(2, 16);
  if (mutate) instructions.push(...i32(outputOffset), ...i32(0), 0x3a, 0, 0);
  if (trap) instructions.push(0x00);
  else instructions.push(...i32(exit), 0x10, 1);
  const body = [0, ...instructions, 0x0b];
  function data(offset, bytes) { return [0, ...i32(offset), 0x0b, ...unsigned(bytes.length), ...bytes]; }
  const bytes = [0, 97, 115, 109, 1, 0, 0, 0,
    ...section(1, [4, 0x60, 4, 0x7f, 0x7f, 0x7f, 0x7f, 1, 0x7f, 0x60, 1, 0x7f, 0, 0x60, 0, 0,
      0x60, 2, 0x7f, 0x7f, 1, 0x7f]),
    ...section(2, [3, ...string('wasi_snapshot_preview1'), ...string('fd_write'), 0, 0,
      ...string('wasi_snapshot_preview1'), ...string('proc_exit'), 0, 1,
      ...string('wasi_snapshot_preview1'), ...string('args_sizes_get'), 0, 3]),
    ...section(3, [1, 2]), ...section(5, [1, 0, ...unsigned(pages)]),
    ...section(7, [2, ...string('memory'), 2, 0, ...string('_start'), 0, 3]),
    ...section(10, [1, ...unsigned(body.length), ...body]),
    ...section(11, [3, ...data(0, descriptor), ...data(outputOffset, output), ...data(errorOffset, stderr)]),
  ];
  return new Uint8Array(bytes);
}
function guest(options) { return new WebAssembly.Module(guestBytes(options)); }

test('worker loads gzip, HTTP-decoded WASM and raw fallbacks without extra network access', async () => {
  const previous = { self: globalThis.self, fetch: globalThis.fetch, decompress: globalThis.DecompressionStream };
  const bytes = guestBytes();
  try {
    for (const mode of ['gzip', 'http-decoded', 'raw', 'gzip-404']) {
      const messages = [];
      const requests = [];
      let handler;
      globalThis.self = {
        addEventListener(type, callback) { assert.equal(type, 'message'); handler = callback; },
        postMessage(message, transfer) {
          if (message.type === 'result') assert.deepEqual(transfer, [message.png]);
          messages.push(message);
        },
      };
      globalThis.DecompressionStream = mode === 'raw' ? undefined : previous.decompress;
      globalThis.fetch = async (url, options) => {
        assert.equal(options.credentials, 'omit');
        const name = new URL(url).pathname.split('/').at(-1);
        requests.push(name);
        assert.ok(['catwalk.wasm.gz', 'catwalk.wasm'].includes(name));
        if (mode === 'gzip-404' && name.endsWith('.gz')) return new Response(null, { status: 404 });
        return new Response(mode === 'gzip' ? gzipSync(bytes) : bytes);
      };
      await import(`${pathToFileURL(join(temporary, 'worker.js')).href}?loader=${mode}`);
      await handler({ data: { type: 'render', id: mode, files: [file('a.obj')], input: 'a.obj', options: { size: 32 } } });
      assert.deepEqual(messages.map(message => message.type), ['progress', 'progress', 'result']);
      assert.ok(messages.every(message => message.id === mode));
      assert.deepEqual(new Uint8Array(messages.at(-1).png), png);
      assert.deepEqual(requests, mode === 'gzip-404' ? ['catwalk.wasm.gz', 'catwalk.wasm']
        : [mode === 'raw' ? 'catwalk.wasm' : 'catwalk.wasm.gz']);
    }
  } finally {
    if (previous.self === undefined) delete globalThis.self;
    else globalThis.self = previous.self;
    globalThis.fetch = previous.fetch;
    globalThis.DecompressionStream = previous.decompress;
  }
});

test('runner captures binary stdout and creates a fresh instance for each render', async () => {
  const module = guest({ mutate: true });
  const first = await runner.runCatwalk(module, [file('model.obj')], 'model.obj');
  const second = await runner.runCatwalk(module, [file('model.obj')], 'model.obj');
  assert.deepEqual(first.png, png);
  assert.deepEqual(second.png, png);
  assert.notEqual(first.png.buffer, second.png.buffer);
});

test('runner advertises UTF-8 argument sizes and forwards the selected render options', async () => {
  const input = 'modèle-模型.obj';
  const args = ['catwalk', `/input/${input}`, '--output', '-', '--size', '64', '--transparent', '--render-geometry'];
  const argumentBytes = args.reduce((total, arg) => total + encoder.encode(arg).length + 1, 0);
  const result = await runner.runCatwalk(guest({ argumentBytes }), [file(input)], input,
    { size: 64, transparent: true, mode: 'geometry' });
  assert.deepEqual(result.png, png);
});

test('runner reports CLI exits, absent previews and traps without returning partial PNGs', async () => {
  for (const exitCode of [1, 3]) {
    await assert.rejects(runner.runCatwalk(guest({ exit: exitCode, error: 'diagnostic\n' }), [file('a.skp')], 'a.skp'), error => {
      assert.equal(error.exitCode, exitCode);
      assert.equal(error.stderr, 'diagnostic\n');
      return true;
    });
  }
  await assert.rejects(runner.runCatwalk(guest({ trap: true }), [file('a.obj')], 'a.obj'), /Rendering stopped/);
  await assert.rejects(runner.runCatwalk(guest({ output: new Uint8Array([1, 2, 3]) }), [file('a.obj')], 'a.obj'), /did not return a PNG/);
});

test('runner bounds output and diagnostic capture', async () => {
  await assert.rejects(runner.runCatwalk(guest({ outputLength: 64 * 1024 * 1024 + 1 }), [file('a.obj')], 'a.obj'), /output limit/);
  await assert.rejects(runner.runCatwalk(guest({ exit: 1, error: 'x'.repeat(70 * 1024) }), [file('a.obj')], 'a.obj'), error => {
    assert.equal(error.stderr.length, 64 * 1024 + '\n[stderr truncated]'.length);
    return true;
  });
});

test('virtual input paths reject traversal, aliases, duplicates and file-directory collisions', async () => {
  const module = guest();
  for (const name of ['', '/a.obj', '../a.obj', 'x/../a.obj', './a.obj', 'x//a.obj', 'x/', 'x\\a.obj', 'C:/a.obj', 'a\0.obj']) {
    await assert.rejects(runner.runCatwalk(module, [file(name)], name), /relative|paths|filename/);
  }
  await assert.rejects(runner.runCatwalk(module, [file('a.obj'), file('a.obj')], 'a.obj'), /Duplicate/);
  for (const names of [['a.obj', 'a.obj/b'], ['a.obj/b', 'a.obj']]) {
    await assert.rejects(runner.runCatwalk(module, names.map(name => file(name)), names[0]), /collides/);
  }
  await assert.rejects(runner.runCatwalk(module, [file('a.obj')], 'missing.obj'), /missing/);
  for (const options of [{ size: 0 }, { size: 4097 }, { size: 3.5 }, { mode: 'extract' }, { transparent: 'yes' }]) {
    await assert.rejects(runner.runCatwalk(module, [file('a.obj')], 'a.obj', options), /size|options/);
  }
});

test('runner refuses oversized inputs before constructing the guest filesystem', async () => {
  const oversized = { name: 'a.obj', bytes: new ArrayBuffer(64 * 1024 * 1024 + 1) };
  await assert.rejects(runner.runCatwalk(guest(), [oversized], 'a.obj'), /64 MiB/);
  const bytes = new ArrayBuffer(44 * 1024 * 1024);
  const files = ['a.obj', 'b', 'c'].map(name => ({ name, bytes }));
  await assert.rejects(runner.runCatwalk(guest(), files, 'a.obj'), /128 MiB/);
});

test('memory readlink has non-symlink semantics on root and opened nested directories', () => {
  const root = runner.createInputDirectory([file('scene/a.obj')]);
  assert.equal(root.path_readlink('input/scene/a.obj').ret, shim.wasi.ERRNO_INVAL);
  assert.equal(root.path_readlink('input/scene').ret, shim.wasi.ERRNO_INVAL);
  assert.equal(root.path_readlink('input/missing').ret, shim.wasi.ERRNO_NOENT);
  assert.equal(root.path_readlink('../outside').ret, shim.wasi.ERRNO_NOTCAPABLE);
  const opened = root.path_open(0, 'input/scene', 0, 0n, 0n, 0).fd_obj;
  assert.equal(opened.path_readlink('a.obj').ret, shim.wasi.ERRNO_INVAL);
  assert.equal(opened.path_readlink('missing').ret, shim.wasi.ERRNO_NOENT);
  const outside = root.path_open(0, 'etc/passwd', 0, 0n, 0n, 0);
  assert.equal(outside.ret, shim.wasi.ERRNO_NOENT);
  const readonly = root.path_open(0, 'input/scene/a.obj', 0, BigInt(shim.wasi.RIGHTS_FD_WRITE), 0n, 0);
  assert.equal(readonly.ret, shim.wasi.ERRNO_PERM);
});

function dimensions(bytes) {
  assert.deepEqual([...bytes.subarray(0, 8)], [...png.subarray(0, 8)]);
  return [new DataView(bytes.buffer, bytes.byteOffset).getUint32(16), new DataView(bytes.buffer, bytes.byteOffset).getUint32(20)];
}

test('actual WASI CLI renders file-backed OBJ with canonicalized material and texture companions', async () => {
  const files = [
    textFile('scene/model.obj', 'mtllib material.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl painted\nf 1/1 2/2 3/3\n'),
    textFile('scene/material.mtl', 'newmtl painted\nKd 1 1 1\nmap_Kd texture.png\n'),
    file('scene/texture.png', new Uint8Array(await readFile(join(root, 'tests/fixtures/formats/images_points/png-pattern.png')))),
  ];
  const result = await runner.runCatwalk(await actualModule(), files, 'scene/model.obj', { size: 64, transparent: true, mode: 'geometry' });
  assert.deepEqual(dimensions(result.png), [64, 64]);
  const again = await runner.runCatwalk(await actualModule(), files, 'scene/model.obj', { size: 64, transparent: true, mode: 'geometry' });
  assert.deepEqual(again.png, result.png);
});

test('actual WASI CLI reads external glTF buffers and reports missing companions', async () => {
  const vertices = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
  const json = {
    asset: { version: '2.0' }, buffers: [{ uri: 'vertices.bin', byteLength: vertices.byteLength }],
    bufferViews: [{ buffer: 0, byteLength: vertices.byteLength }],
    accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: 'VEC3', min: [0, 0, 0], max: [1, 1, 0] }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }], nodes: [{ mesh: 0 }], scenes: [{ nodes: [0] }], scene: 0,
  };
  const input = textFile('scene/model.gltf', JSON.stringify(json));
  const result = await runner.runCatwalk(await actualModule(), [input, file('scene/vertices.bin', new Uint8Array(vertices.buffer))], input.name, { size: 80 });
  assert.deepEqual(dimensions(result.png), [80, 80]);
  await assert.rejects(runner.runCatwalk(await actualModule(), [input], input.name), error => {
    assert.equal(error.exitCode, 1);
    assert.match(error.stderr, /vertices|No such|not found/i);
    return true;
  });
});

test('actual WASI CLI extracts a native saved preview and rejects malformed geometry', async () => {
  const path = join(root, 'tests/fixtures/formats/solidworks/solidworks-about.sldprt');
  const result = await runner.runCatwalk(await actualModule(), [file('part.sldprt', new Uint8Array(await readFile(path)))], 'part.sldprt', { size: 48 });
  assert.deepEqual(dimensions(result.png), [48, 48]);
  await assert.rejects(runner.runCatwalk(await actualModule(), [textFile('broken.obj', 'f 99 99 99\n')], 'broken.obj'), error => {
    assert.equal(error.exitCode, 1);
    return true;
  });
});
