// popup.js - Self-contained popup logic for Crates Packer
// No dependency on background/content scripts.

// ---------- State ----------
let currentCrate = null;
let savedCrates = [];
let currentPage = 0;
let lastSaved = 'Never';
let settings = {
  autoSave: false,
  saveFormat: 'toml'
};

// ---------- DOM ----------
const page1 = document.getElementById('page1');
const page2 = document.getElementById('page2');
const prevBtn = document.getElementById('prevBtn');
const nextBtn = document.getElementById('nextBtn');
const dots = document.querySelectorAll('.page-dots .dot');

const crateInfo = document.getElementById('crateInfo');
const crateDetails = document.getElementById('crateDetails');
const savedBadge = document.getElementById('savedBadge');
const saveBtn = document.getElementById('saveBtn');

const autoSaveToggle = document.getElementById('autoSave');
const exportFormat = document.getElementById('exportFormat');
const exportBtn = document.getElementById('exportBtn');
const clearBtn = document.getElementById('clearBtn');
const downloadBtn = document.getElementById('downloadBtn');

const savedCount = document.getElementById('savedCount');
const lastSavedEl = document.getElementById('lastSaved');
const statusEl = document.getElementById('status');
const status2El = document.getElementById('status2');

const previewContent = document.getElementById('previewContent');
const previewFilename = document.getElementById('previewFilename');
const previewLines = document.getElementById('previewLines');
const previewSubtitle = document.getElementById('previewSubtitle');

// ---------- Storage ----------
async function loadState() {
  try {
    const data = await chrome.storage.local.get(['savedCrates', 'settings', 'lastSaved']);
    if (Array.isArray(data.savedCrates)) savedCrates = data.savedCrates;
    if (data.settings) settings = { ...settings, ...data.settings };
    if (data.lastSaved) lastSaved = data.lastSaved;
  } catch (e) {
    console.error('Failed to load state:', e);
  }
}

async function saveState() {
  try {
    await chrome.storage.local.set({ savedCrates, settings, lastSaved });
  } catch (e) {
    console.error('Failed to save state:', e);
  }
}

// ---------- Current tab / crate detection ----------
async function checkCurrentTab() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab || !tab.url) { currentCrate = null; return; }

    const match = tab.url.match(/crates\.io\/crates\/([^\/?#]+)/);
    if (!match) { currentCrate = null; return; }

    const crateName = decodeURIComponent(match[1]);
    const response = await fetch(`https://crates.io/api/v1/crates/${crateName}`);
    if (!response.ok) throw new Error('Failed to fetch crate');
    const data = await response.json();
    const c = data.crate;

    currentCrate = {
      name: c.name,
      version: c.max_version || c.max_stable_version || 'unknown',
      description: c.description || '',
      url: `https://crates.io/crates/${c.name}`,
      repository: c.repository || null,
      documentation: c.documentation || null,
      license: c.license || null,
      downloads: c.downloads || null,
      categories: (c.categories || []).map(x => x.slug),
      keywords: c.keywords || [],
      rustVersion: c.rust_version || null,
      edition: c.edition || null
    };
  } catch (e) {
    console.error('Failed to get current crate:', e);
    currentCrate = null;
  }
}

function isCrateSaved(crate) {
  if (!crate) return false;
  return savedCrates.some(sc => sc.name === crate.name && sc.version === crate.version);
}

// ---------- Status ----------
function setStatus(el, message, type) {
  if (!el) return;
  el.textContent = message;
  el.className = 'status' + (type ? ' ' + type : '');
}

function showStatus(message, type) {
  setStatus(statusEl, message, type);
  clearTimeout(showStatus._t);
  showStatus._t = setTimeout(() => setStatus(statusEl, 'Ready', ''), 3500);
}

function showStatus2(message, type) {
  setStatus(status2El, message, type);
  clearTimeout(showStatus2._t);
  showStatus2._t = setTimeout(() => setStatus(status2El, 'Ready', ''), 3500);
}

// ---------- Rendering ----------
function render() {
  // Crate info
  if (currentCrate) {
    crateDetails.innerHTML =
      `<div><strong>Name:</strong> ${escapeHtml(currentCrate.name)}</div>` +
      `<div><strong>Version:</strong> ${escapeHtml(currentCrate.version)}</div>` +
      `<div><strong>Description:</strong> ${escapeHtml(currentCrate.description || 'N/A')}</div>` +
      `<div><strong>URL:</strong> ${escapeHtml(currentCrate.url)}</div>`;

    const saved = isCrateSaved(currentCrate);
    savedBadge.classList.toggle('visible', saved);
    crateInfo.classList.toggle('crate-saved', saved);
    saveBtn.disabled = saved;
    saveBtn.textContent = saved ? 'Already Saved' : `Save ${currentCrate.name}`;
  } else {
    crateDetails.textContent = 'Not on a crate page. Visit crates.io/crates/* to save.';
    savedBadge.classList.remove('visible');
    crateInfo.classList.remove('crate-saved');
    saveBtn.disabled = true;
    saveBtn.textContent = 'Save Current Crate';
  }

  // Settings
  autoSaveToggle.checked = !!settings.autoSave;
  exportFormat.value = settings.saveFormat;

  // Stats
  savedCount.textContent = savedCrates.length;
  lastSavedEl.textContent = lastSaved;

  // Preview (page 2)
  renderPreview();

  // Pagination
  renderPagination();
}

function renderPagination() {
  const isPage1 = currentPage === 0;
  page1.classList.toggle('active', isPage1);
  page2.classList.toggle('active', !isPage1);
  prevBtn.disabled = isPage1;
  nextBtn.disabled = !isPage1;
  dots.forEach((dot, i) => dot.classList.toggle('active', i === currentPage));
}

function renderPreview() {
  const content = generateExport();
  const filename = exportFilename();

  previewContent.textContent = content;
  previewFilename.textContent = filename;

  const lineCount = content.length ? content.split('\n').length : 0;
  previewLines.textContent = `${lineCount} line${lineCount === 1 ? '' : 's'}`;
  previewSubtitle.textContent = savedCrates.length
    ? `Exactly what will be downloaded (${savedCrates.length} crate${savedCrates.length === 1 ? '' : 's'}).`
    : 'This is exactly what will be downloaded.';
}

// ---------- Export generation ----------
function exportFilename() {
  const ext = settings.saveFormat === 'json' ? 'json' : settings.saveFormat === 'text' ? 'txt' : 'toml';
  const base = settings.saveFormat === 'toml' ? 'Cargo.toml' : `crates.${ext}`;
  return base;
}

function generateExport() {
  const format = settings.saveFormat;
  if (format === 'json') return generateJSON();
  if (format === 'text') return generateText();
  return generateTOML();
}

function generateTOML() {
  let toml = '# Cargo.toml - Generated by Crates Packer\n';
  toml += '# Source: crates.io\n';
  toml += `# Generated: ${new Date().toISOString()}\n`;
  toml += `# Crates: ${savedCrates.length}\n\n`;

  toml += '[package]\n';
  toml += 'name = "my-project"\n';
  toml += 'version = "0.1.0"\n';
  toml += 'edition = "2021"\n\n';

  if (savedCrates.length === 0) {
    toml += '# No crates saved yet.\n';
    return toml;
  }

  toml += '[dependencies]\n';
  for (const c of savedCrates) {
    toml += `# ${c.name} v${c.version}\n`;
    if (c.description) toml += `# ${c.description.replace(/\n/g, ' ')}\n`;
    if (c.repository) toml += `# Repository: ${c.repository}\n`;
    if (c.license) toml += `# License: ${c.license}\n`;
    toml += `${c.name} = "${c.version}"\n\n`;
  }
  return toml.trimEnd() + '\n';
}

function generateJSON() {
  return JSON.stringify({
    meta: {
      generated: new Date().toISOString(),
      count: savedCrates.length,
      source: 'Crates Packer'
    },
    packages: savedCrates
  }, null, 2) + '\n';
}

function generateText() {
  let text = 'Crates.io Saved Packages\n';
  text += `Generated: ${new Date().toISOString()}\n`;
  text += `Total: ${savedCrates.length} packages\n\n`;
  savedCrates.forEach((c, i) => {
    text += `${i + 1}. ${c.name} @ ${c.version}\n`;
    if (c.description) text += `   Description: ${c.description}\n`;
    if (c.url) text += `   URL: ${c.url}\n`;
    text += '\n';
  });
  if (savedCrates.length === 0) text += 'No crates saved yet.\n';
  return text;
}

// ---------- Actions ----------
async function saveCurrentCrate() {
  if (!currentCrate || saveBtn.disabled) return;

  if (isCrateSaved(currentCrate)) {
    showStatus('Crate already saved', 'error');
    return;
  }

  savedCrates.push({ ...currentCrate, timestamp: new Date().toISOString() });
  lastSaved = new Date().toLocaleTimeString();
  await saveState();
  render();

  let msg = `Saved ${currentCrate.name} v${currentCrate.version} — file updated (${savedCrates.length} crates)`;

  if (settings.autoSave) {
    const ok = await downloadExport();
    msg += ok ? ' · downloaded' : ' · download failed';
  }

  showStatus(msg, 'success');
}

async function exportAll() {
  if (savedCrates.length === 0) {
    showStatus('No crates to export', 'error');
    return;
  }
  const ok = await downloadExport();
  if (ok) {
    showStatus(`Exported ${savedCrates.length} crate${savedCrates.length === 1 ? '' : 's'} to ${exportFilename()}`, 'success');
    showStatus2(`Downloaded ${exportFilename()}`, 'success');
  } else {
    showStatus('Export failed', 'error');
    showStatus2('Download failed', 'error');
  }
}

async function clearAll() {
  if (savedCrates.length === 0) { showStatus('Nothing to clear', 'error'); return; }
  if (!confirm(`Clear all ${savedCrates.length} saved crates?`)) return;

  savedCrates = [];
  lastSaved = 'Never';
  await saveState();
  render();
  showStatus('All crates cleared', 'success');
}

// ---------- Download ----------
async function downloadExport() {
  const content = generateExport();
  const filename = exportFilename();
  const mime = settings.saveFormat === 'json' ? 'application/json' : 'text/plain';

  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);

  try {
    if (typeof chrome !== 'undefined' && chrome.downloads && chrome.downloads.download) {
      await new Promise((resolve) => {
        chrome.downloads.download({ url, filename, saveAs: false }, () => {
          const err = chrome.runtime && chrome.runtime.lastError;
          if (err) {
            console.error('chrome.downloads failed:', err);
            fallbackDownload(url, filename);
          }
          resolve(!err);
        });
      });
    } else {
      fallbackDownload(url, filename);
    }
    return true;
  } catch (e) {
    console.error('Download error:', e);
    try { fallbackDownload(url, filename); } catch (_) {}
    return false;
  } finally {
    // Give the download time to start before revoking.
    setTimeout(() => URL.revokeObjectURL(url), 60000);
  }
}

function fallbackDownload(url, filename) {
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
}

// ---------- Navigation ----------
function goToPage(page) {
  currentPage = page === 1 ? 1 : 0;
  renderPagination();
}

// ---------- Helpers ----------
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// ---------- Wiring ----------
function setupEventListeners() {
  saveBtn.addEventListener('click', saveCurrentCrate);
  exportBtn.addEventListener('click', exportAll);
  downloadBtn.addEventListener('click', exportAll);
  clearBtn.addEventListener('click', clearAll);

  autoSaveToggle.addEventListener('change', async (e) => {
    settings.autoSave = e.target.checked;
    await saveState();
  });

  exportFormat.addEventListener('change', async (e) => {
    settings.saveFormat = e.target.value;
    await saveState();
    renderPreview();
  });

  prevBtn.addEventListener('click', () => goToPage(currentPage - 1));
  nextBtn.addEventListener('click', () => goToPage(currentPage + 1));
  dots.forEach(dot => {
    dot.addEventListener('click', () => goToPage(parseInt(dot.dataset.page, 10)));
  });
}

document.addEventListener('DOMContentLoaded', async () => {
  await loadState();
  await checkCurrentTab();
  setupEventListeners();
  render();
});