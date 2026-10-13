import { runCatwalk } from './wasi-runner.js';

let modulePromise;
let busy = false;

async function loadBytes(compressed) {
  const response = await fetch(new URL(compressed ? './catwalk.wasm.gz' : './catwalk.wasm', import.meta.url), {
    credentials: 'omit',
  });
  if (compressed && response.status === 404) return loadBytes(false);
  if (!response.ok) throw new Error(`Could not load the renderer (${response.status}).`);
  const bytes = await response.arrayBuffer();
  // A host may already decode Content-Encoding: gzip before exposing the body.
  const magic = new Uint8Array(bytes, 0, Math.min(2, bytes.byteLength));
  if (magic[0] === 0x1f && magic[1] === 0x8b) {
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
    return new Response(stream).arrayBuffer();
  }
  return bytes;
}

async function loadModule() {
  if (!modulePromise) {
    modulePromise = loadBytes(typeof DecompressionStream === 'function')
      .then(bytes => WebAssembly.compile(bytes));
  }
  try {
    return await modulePromise;
  } catch (error) {
    modulePromise = undefined;
    throw error;
  }
}

self.addEventListener('message', async event => {
  const job = event.data;
  if (job?.type !== 'render') return;
  const { id } = job;
  if (busy) {
    self.postMessage({ type: 'error', id, message: 'Another thumbnail is already rendering.' });
    return;
  }
  busy = true;
  try {
    self.postMessage({ type: 'progress', id, stage: 'loading', message: 'Loading the renderer…' });
    const module = await loadModule();
    self.postMessage({ type: 'progress', id, stage: 'rendering', message: 'Rendering your thumbnail…' });
    const result = await runCatwalk(module, job.files, job.input, job.options);
    self.postMessage({ type: 'result', id, png: result.png.buffer }, [result.png.buffer]);
  } catch (error) {
    self.postMessage({
      type: 'error', id, message: error.message ?? String(error),
      exitCode: error.exitCode, stderr: error.stderr ?? '',
    });
  } finally {
    busy = false;
  }
});
