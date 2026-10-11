import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { Writable } from 'node:stream';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import config from '../release.config.js';

const cwd = fileURLToPath(new URL('..', import.meta.url));
const require = createRequire(import.meta.url);
const coreDirectory = dirname(require.resolve('semantic-release'));
// Use the installed core's loader: direct imports miss nested-plugin resolution bugs.
const { loadPlugin } = await import(
  pathToFileURL(join(coreDirectory, 'lib/plugins/utils.js')).href
);

async function configuredPlugin(moduleName) {
  const entry = config.plugins.find(
    (plugin) => Array.isArray(plugin)
      && (plugin[0] === moduleName || plugin[0] === require.resolve(moduleName))
  );
  assert.ok(entry, `Missing configured plugin: ${moduleName}`);
  const [name, options] = entry;
  return [await loadPlugin({ cwd }, name, {}), options];
}

for (const [message, expected] of [
  ['fix: preserve model normals', 'patch'],
  ['feat: preview another model format', 'minor'],
  ['feat!: change the scene API', 'major'],
  ['docs: explain thumbnail options', null],
]) {
  test(`configured analyzer classifies ${message}`, async () => {
    const [plugin, options] = await configuredPlugin('@semantic-release/commit-analyzer');
    const result = await plugin.analyzeCommits(options, {
      cwd,
      commits: [{ hash: 'a'.repeat(40), message }],
      logger: { log() {} },
    });
    assert.equal(result, expected);
  });
}

test('configured notes generator renders the Conventional Commits preset', async () => {
  const [plugin, options] = await configuredPlugin('@semantic-release/release-notes-generator');
  const notes = await plugin.generateNotes(options, {
    cwd,
    options: config,
    commits: [{ hash: 'a'.repeat(40), message: 'fix: preserve model normals' }],
    lastRelease: { gitTag: '1.0.0', gitHead: 'b'.repeat(40) },
    nextRelease: { version: '1.0.1', gitTag: '1.0.1', gitHead: 'a'.repeat(40) },
  });
  assert.match(notes, /1\.0\.1/);
  assert.match(notes, /preserve model normals/);
  assert.ok(notes.includes('https://github.com/cmeister2/catwalk3d/compare/1.0.0...1.0.1'));
});

test('configured asset guard stops preparation before an incomplete release can be tagged', async () => {
  const [plugin, options] = await configuredPlugin('@semantic-release/exec');
  const guardIndex = config.plugins.findIndex((entry) => entry[0] === '@semantic-release/exec');
  const cargoIndex = config.plugins.indexOf('@semantic-release-cargo/semantic-release-cargo');
  assert.ok(guardIndex >= 0 && guardIndex < cargoIndex);
  assert.equal(options.publishCmd, undefined);
  const quiet = () => new Writable({ write(_chunk, _encoding, done) { done(); } });
  await assert.rejects(plugin.prepare(options, {
    cwd,
    env: process.env,
    nextRelease: { version: '9999.0.0', gitHead: 'c'.repeat(40) },
    logger: { log() {} },
    stdout: quiet(),
    stderr: quiet(),
  }), (error) => /ENOENT[^\n]*release-assets|Release assets must contain exactly every configured target/.test(error.stderr ?? ''));
});
