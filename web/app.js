import { MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES, MAX_OUTPUT_BYTES, JOB_TIMEOUT_MS } from './config.js';

const element = (id) => document.getElementById(id);
const form = element('upload-form');
const controls = element('input-controls');
const primaryPicker = element('primary-file');
const companionPicker = element('companion-files');
const dropTarget = element('drop-target');
const generateButton = element('generate');
const cancelButton = element('cancel');
const status = element('status');
const errorNotice = element('error-notice');
const errorMessage = element('error-message');
const previewImage = element('preview-image');
const download = element('download');
const canRender = typeof Worker !== 'undefined' && typeof WebAssembly !== 'undefined';
let primary = null;
let companions = [];
let activeJob = null;
let nextJobId = 0;
let previewUrl = null;
let dragDepth = 0;

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function clearError() {
  errorNotice.hidden = true;
  errorMessage.textContent = '';
}

function showError(message) {
  errorMessage.textContent = message;
  errorNotice.hidden = false;
}

function clearPreview() {
  previewImage.onload = null;
  previewImage.onerror = null;
  previewImage.removeAttribute('src');
  previewImage.alt = '';
  previewImage.hidden = true;
  download.hidden = true;
  download.removeAttribute('href');
  download.removeAttribute('download');
  element('preview-size').hidden = true;
  element('preview-placeholder').hidden = false;
  element('preview-caption').textContent = 'The first preview may take a moment to prepare.';
  if (previewUrl !== null) URL.revokeObjectURL(previewUrl);
  previewUrl = null;
}

function setBusy(busy) {
  controls.disabled = busy;
  generateButton.disabled = busy || primary === null;
  cancelButton.hidden = !busy;
  element('progress-spinner').hidden = !busy;
  element('preview-card').setAttribute('aria-busy', String(busy));
  dropTarget.setAttribute('aria-disabled', String(busy));
  dropTarget.classList.remove('is-dragging');
  dragDepth = 0;
}

function finishJob(job) {
  if (activeJob !== job) return false;
  clearTimeout(job.timer);
  job.worker?.terminate();
  job.worker = null;
  activeJob = null;
  setBusy(false);
  return true;
}

function failJob(job, message) {
  if (!finishJob(job)) return;
  clearPreview();
  status.textContent = 'No thumbnail created. You can adjust the settings or choose another file.';
  showError(message);
}

function refreshFiles() {
  element('primary-summary').hidden = primary === null;
  element('primary-summary').textContent = primary ? `${primary.name} · ${formatBytes(primary.size)}` : '';
  element('companion-details').hidden = companions.length === 0;
  element('companion-summary').textContent = `${companions.length} companion ${companions.length === 1 ? 'file' : 'files'} · ${formatBytes(companions.reduce((total, file) => total + file.size, 0))}`;
  const list = element('companion-list');
  list.replaceChildren();
  for (const file of companions) {
    const item = document.createElement('li');
    item.textContent = `${file.name} · ${formatBytes(file.size)}`;
    list.append(item);
  }
  generateButton.disabled = activeJob !== null || primary === null;
}

function selectionChanged() {
  clearError();
  clearPreview();
  refreshFiles();
  status.textContent = primary ? 'Ready to create your thumbnail.' : 'Choose a file to get started.';
}

function validateFiles(files) {
  if (files.length > MAX_FILES) throw new Error(`Too many files selected. Choose up to ${MAX_FILES} files, including the main file and its companions.`);
  const names = new Set();
  let total = 0;
  for (const file of files) {
    if (names.has(file.name)) throw new Error(`More than one selected file is named “${file.name}”. Choose one copy of each file.`);
    names.add(file.name);
    if (file.size === 0) throw new Error(`“${file.name}” is empty. Choose a file that contains a model or image.`);
    if (file.size > MAX_FILE_BYTES) throw new Error(`“${file.name}” is too large. Each file must be ${formatBytes(MAX_FILE_BYTES)} or smaller.`);
    total += file.size;
    if (total > MAX_TOTAL_BYTES) throw new Error(`The selected files are too large together. Choose files totaling ${formatBytes(MAX_TOTAL_BYTES)} or less.`);
  }
}

function displayResult(job, png) {
  if (!(png instanceof ArrayBuffer) || png.byteLength === 0 || png.byteLength > MAX_OUTPUT_BYTES) {
    failJob(job, 'The preview could not be saved as a PNG. Try again with a smaller thumbnail.');
    return;
  }
  if (!finishJob(job)) return;
  const url = URL.createObjectURL(new Blob([png], { type: 'image/png' }));
  previewUrl = url;
  previewImage.onload = () => {
    if (previewUrl !== url) return;
    element('preview-placeholder').hidden = true;
    previewImage.hidden = false;
    element('preview-size').textContent = `${job.options.size} × ${job.options.size}`;
    element('preview-size').hidden = false;
    element('preview-caption').textContent = job.input;
    download.href = url;
    download.download = `${job.input.replace(/\.[^.]+$/, '') || 'thumbnail'}-${job.options.size}.png`;
    download.hidden = false;
    status.textContent = 'Your thumbnail is ready. Download the PNG to save it.';
  };
  previewImage.onerror = () => {
    if (previewUrl !== url) return;
    clearPreview();
    status.textContent = 'No thumbnail created. Please try again.';
    showError('The generated preview could not be displayed. Try again with a smaller thumbnail.');
  };
  previewImage.alt = `Thumbnail of ${job.input}`;
  previewImage.src = url;
  status.textContent = 'Opening your thumbnail…';
}

form.addEventListener('submit', async (event) => {
  event.preventDefault();
  if (!canRender || activeJob !== null || primary === null) return;
  clearError();
  clearPreview();
  const selected = [primary, ...companions];
  try {
    validateFiles(selected);
  } catch (error) {
    showError(error.message);
    status.textContent = 'Check your selected files before creating a thumbnail.';
    return;
  }
  const job = {
    id: ++nextJobId,
    input: primary.name,
    options: {
      size: Number(element('thumbnail-size').value),
      transparent: element('transparent').checked,
      mode: element('preview-mode').value,
    },
    worker: null,
    timer: null,
  };
  activeJob = job;
  setBusy(true);
  status.textContent = 'Reading your files…';
  job.timer = setTimeout(() => failJob(job, 'This preview took too long. Try a smaller file or a smaller thumbnail.'), JOB_TIMEOUT_MS);
  try {
    const files = [];
    for (const file of selected) {
      const bytes = await file.arrayBuffer();
      if (activeJob !== job) return;
      files.push({ name: file.name, bytes });
    }
    if (activeJob !== job) return;
    status.textContent = 'Preparing your preview…';
    job.worker = new Worker('./worker.js', { type: 'module' });
    job.worker.onmessage = ({ data }) => {
      if (activeJob !== job || data?.id !== job.id) return;
      if (data.type === 'progress') {
        if (typeof data.message === 'string') status.textContent = data.message;
      } else if (data.type === 'result') {
        displayResult(job, data.png);
      } else if (data.type === 'error') {
        failJob(job, typeof data.message === 'string' && data.message ? data.message : 'We couldn’t create a preview from this file. Check its format and try again.');
      } else {
        failJob(job, 'The preview stopped unexpectedly. Please try again.');
      }
    };
    job.worker.onerror = (event) => {
      event.preventDefault();
      failJob(job, 'The preview stopped unexpectedly. Try a smaller file or another preview mode.');
    };
    job.worker.onmessageerror = () => failJob(job, 'The preview could not be opened. Please try again.');
    job.worker.postMessage({ type: 'render', id: job.id, input: job.input, files, options: job.options }, files.map(({ bytes }) => bytes));
  } catch {
    failJob(job, 'Your file could not be read or the preview could not start. Choose the file again and retry.');
  }
});

primaryPicker.addEventListener('change', () => {
  primary = primaryPicker.files[0] ?? null;
  selectionChanged();
});

companionPicker.addEventListener('change', () => {
  companions = Array.from(companionPicker.files);
  selectionChanged();
});

element('clear-companions').addEventListener('click', () => {
  companions = [];
  companionPicker.value = '';
  selectionChanged();
});

for (const id of ['thumbnail-size', 'preview-mode', 'transparent']) {
  element(id).addEventListener('change', selectionChanged);
}

cancelButton.addEventListener('click', () => {
  if (activeJob === null) return;
  finishJob(activeJob);
  clearPreview();
  clearError();
  status.textContent = 'Preview cancelled. Your files are still selected.';
});

element('reset-files').addEventListener('click', () => {
  if (activeJob !== null) finishJob(activeJob);
  form.reset();
  primary = null;
  companions = [];
  selectionChanged();
});

element('dismiss-error').addEventListener('click', clearError);

for (const type of ['dragenter', 'dragover', 'dragleave', 'drop']) {
  dropTarget.addEventListener(type, (event) => {
    event.preventDefault();
    if (!canRender || activeJob !== null) return;
    if (type === 'dragenter') {
      dragDepth += 1;
      dropTarget.classList.add('is-dragging');
    } else if (type === 'dragover') {
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
    } else if (type === 'dragleave') {
      dragDepth = Math.max(0, dragDepth - 1);
      if (dragDepth === 0) dropTarget.classList.remove('is-dragging');
    } else {
      dragDepth = 0;
      dropTarget.classList.remove('is-dragging');
      const files = Array.from(event.dataTransfer?.files ?? []);
      if (files.length !== 1) {
        showError('Drop one main file here. Add any materials, textures or parts using Companion files.');
        return;
      }
      primaryPicker.value = '';
      primary = files[0];
      selectionChanged();
    }
  });
}

// Prevent a dropped file elsewhere on the page from replacing the page.
for (const type of ['dragover', 'drop']) {
  window.addEventListener(type, (event) => event.preventDefault());
}

window.addEventListener('pagehide', () => {
  if (activeJob !== null) finishJob(activeJob);
  clearPreview();
});

element('file-limits').textContent = `Up to ${MAX_FILES} files, ${formatBytes(MAX_FILE_BYTES)} per file and ${formatBytes(MAX_TOTAL_BYTES)} in total.`;
if (!canRender) {
  controls.disabled = true;
  showError('This browser cannot create these previews. Try a current version of Chrome, Firefox, Edge or Safari.');
  status.textContent = 'Preview creation is unavailable in this browser.';
  dropTarget.setAttribute('aria-disabled', 'true');
} else {
  refreshFiles();
}
