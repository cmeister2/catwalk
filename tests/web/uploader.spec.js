import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { MAX_FILES } from '../../web/config.js';

const fixture = (relative) => fileURLToPath(new URL(`../../${relative}`, import.meta.url));
const gear = fixture('examples/models/gear.obj');
const pngSignature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const observations = new WeakMap();

test.beforeEach(async ({ page, context }) => {
  const requests = [];
  const errors = [];
  observations.set(page, { requests, errors, expectRenderer: true });
  context.on('request', request => requests.push({ url: request.url(), method: request.method() }));
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('./');
  await expect(page.locator('#file-limits')).toContainText('64.0 MiB');
  await expect(page.locator('#generate')).toBeDisabled();
});

test.afterEach(async ({ page, baseURL }) => {
  const { requests, errors, expectRenderer } = observations.get(page);
  const origin = new URL(baseURL).origin;
  expect(errors, 'the page has no uncaught errors').toEqual([]);
  expect(requests.some(request => /\/web\/catwalk\.wasm(?:\.gz)?$/.test(new URL(request.url).pathname)),
    'the renderer is requested only after file selection passes preflight').toBe(expectRenderer);
  for (const request of requests) {
    expect(new URL(request.url).origin, `request remains local: ${request.url}`).toBe(origin);
    expect(request.method, `selected files are never uploaded: ${request.url}`).toBe('GET');
  }
});

async function selectFile(page, input, companions = []) {
  await page.locator('#primary-file').setInputFiles(input);
  await page.locator('#companion-files').setInputFiles(companions);
  await page.locator('#thumbnail-size').selectOption('256');
  await expect(page.locator('#generate')).toBeEnabled();
}

async function render(page) {
  await page.locator('#generate').click();
  await expect(page.locator('#preview-image')).toBeVisible();
  await expect(page.locator('#download')).toBeVisible();
  await expect(page.locator('#status')).toContainText('ready');
  await expect(page.locator('#error-notice')).toBeHidden();
  await expect(page.locator('#generate')).toBeEnabled();
}

async function pixels(page) {
  return page.locator('#preview-image').evaluate(image => {
    const canvas = document.createElement('canvas');
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
    let transparent = 0;
    let visible = 0;
    let red = 0;
    let green = 0;
    let blue = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (data[i + 3] === 0) transparent += 1;
      if (data[i + 3] > 0) {
        visible += 1;
        if (data[i] > 1.3 * data[i + 1] && data[i] > 1.3 * data[i + 2]) red += 1;
        if (data[i + 1] > 1.3 * data[i] && data[i + 1] > 1.3 * data[i + 2]) green += 1;
        if (data[i + 2] > 1.3 * data[i] && data[i + 2] > 1.3 * data[i + 1]) blue += 1;
      }
    }
    return { width: canvas.width, height: canvas.height, cornerAlpha: data[3], transparent, visible, red, green, blue };
  });
}

async function assertDownload(page, size, expectedName) {
  const pending = page.waitForEvent('download');
  await page.locator('#download').click();
  const download = await pending;
  expect(download.suggestedFilename()).toBe(expectedName);
  const bytes = await readFile(await download.path());
  expect(bytes.length).toBeGreaterThan(33);
  expect(bytes.subarray(0, 8)).toEqual(pngSignature);
  expect(bytes.subarray(12, 16).toString('ascii')).toBe('IHDR');
  expect(bytes.readUInt32BE(16)).toBe(size);
  expect(bytes.readUInt32BE(20)).toBe(size);
}

test('renders the gear locally and downloads a PNG on desktop and mobile', async ({ page }, testInfo) => {
  await selectFile(page, gear);
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([256, 256]);
  expect(image.visible).toBeGreaterThan(100);
  expect(image.transparent).toBeGreaterThan(100);
  expect(image.cornerAlpha).toBe(0);
  await assertDownload(page, 256, 'gear-256.png');

  await page.evaluate(() => { window.scrollTo(0, 0); document.activeElement?.blur(); });
  const desktop = testInfo.outputPath('gear-desktop.png');
  await page.screenshot({ path: desktop, fullPage: true });
  await testInfo.attach('gear-desktop', { path: desktop, contentType: 'image/png' });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.evaluate(() => window.scrollTo(0, 0));
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const mobile = testInfo.outputPath('gear-mobile.png');
  await page.screenshot({ path: mobile, fullPage: true });
  await testInfo.attach('gear-mobile', { path: mobile, contentType: 'image/png' });
});

test('applies geometry mode, size and an opaque background', async ({ page }) => {
  await selectFile(page, gear);
  await page.locator('#thumbnail-size').selectOption('512');
  await page.locator('#preview-mode').selectOption('geometry');
  await page.locator('#transparent').uncheck();
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([512, 512]);
  expect(image.visible).toBe(512 * 512);
  expect(image.transparent).toBe(0);
  expect(image.cornerAlpha).toBe(255);
  await assertDownload(page, 512, 'gear-512.png');
});

test('reports unsupported and malformed files, then renders the next valid file', async ({ page }) => {
  await selectFile(page, { name: 'unsupported.catwalk-test', mimeType: 'application/octet-stream', buffer: Buffer.from('not a supported document') });
  await page.locator('#generate').click();
  await expect(page.locator('#error-notice')).toBeVisible();
  await expect(page.locator('#error-message')).toContainText(/reader|extension|unsupported/i);
  await expect(page.locator('#download')).toBeHidden();
  await expect(page.locator('#generate')).toBeEnabled();

  await selectFile(page, { name: 'broken.obj', mimeType: 'text/plain', buffer: Buffer.from('v nope 0 0\n') });
  await expect(page.locator('#error-notice')).toBeHidden();
  await page.locator('#generate').click();
  await expect(page.locator('#error-notice')).toBeVisible();
  await expect(page.locator('#error-message')).toContainText(/failed|invalid|obj|geometry/i);
  await expect(page.locator('#download')).toBeHidden();
  await expect(page.locator('#generate')).toBeEnabled();

  await selectFile(page, gear);
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([256, 256]);
  expect(image.visible).toBeGreaterThan(100);
});

test('extracts a real MAX saved preview through bounded compound-file and clock readers', async ({ page }) => {
  await selectFile(page, fixture('tests/fixtures/formats/max_saved/Combine Test.max'));
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([256, 256]);
  expect(image.visible).toBeGreaterThan(100);
  await assertDownload(page, 256, 'Combine Test-256.png');
});

test('resolves uploaded OBJ materials and texture companions locally', async ({ page }) => {
  await selectFile(page, fixture('tests/fixtures/formats/obj_appearance/obj-texture-transform-border.obj'), [
    fixture('tests/fixtures/formats/obj_appearance/maps.mtl'),
    fixture('tests/fixtures/formats/obj_appearance/white.png'),
    fixture('tests/fixtures/formats/obj_appearance/quadrants.png'),
  ]);
  await expect(page.locator('#companion-summary')).toContainText('3 companion files');
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([256, 256]);
  // The authored red/green/blue quadrant map distinguishes decoded textures
  // from the plain diffuse fallback without fixing floating-point PNG bytes.
  expect(image.red).toBeGreaterThan(10);
  expect(image.green).toBeGreaterThan(10);
  expect(image.blue).toBeGreaterThan(10);
});

test('cancels while the real renderer loads and successfully starts a new job', async ({ page }) => {
  let heldRoute;
  let requested;
  const wasmRequested = new Promise(resolve => { requested = resolve; });
  const wasmPattern = /\/web\/catwalk\.wasm(?:\.gz)?$/;
  await page.route(wasmPattern, route => { heldRoute = route; requested(); });
  await selectFile(page, gear);
  await page.locator('#generate').click();
  await wasmRequested;
  await expect(page.locator('#cancel')).toBeVisible();
  await expect(page.locator('#primary-file')).toBeDisabled();
  await page.locator('#cancel').click();
  await expect(page.locator('#status')).toContainText('cancelled');
  await expect(page.locator('#primary-file')).toBeEnabled();
  await expect(page.locator('#download')).toBeHidden();
  await heldRoute.abort().catch(() => {});
  await page.unroute(wasmPattern);
  await render(page);
  const image = await pixels(page);
  expect([image.width, image.height]).toEqual([256, 256]);
  expect(image.visible).toBeGreaterThan(100);
});

test('rejects excessive file counts before empty companions or renderer loading', async ({ page }) => {
  observations.get(page).expectRenderer = false;
  const companions = Array.from({ length: MAX_FILES }, (_, index) => ({
    name: `companion-${index}.mtl`, mimeType: 'text/plain', buffer: Buffer.alloc(0),
  }));
  await selectFile(page, gear, companions);
  expect(await page.locator('#companion-files').evaluate(input => input.files.length)).toBe(MAX_FILES);
  await page.locator('#generate').click();
  await expect(page.locator('#error-notice')).toBeVisible();
  await expect(page.locator('#error-message')).toContainText(`Choose up to ${MAX_FILES} files`);
  await expect(page.locator('#error-message')).not.toContainText('empty');
  await expect(page.locator('#cancel')).toBeHidden();
  await expect(page.locator('#download')).toBeHidden();
  await expect(page.locator('#generate')).toBeEnabled();
});
