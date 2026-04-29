import { fetchJSON } from "./shared/api.js";
import {
  useErrorBanner,
  renderLoading,
  renderEmpty,
  renderErrorBlock,
  renderTable,
  td,
  initSearchBar,
} from "./shared/components.js";
import { formatDateTime } from "./shared/format.js";
import { renderNav } from "./shared/nav.js";

renderNav("folders");

// ── State ──────────────────────────────────────

let currentFolderId = null;
let foldersData = [];

const ALL_EXTENSIONS = [
  "mp3",
  "flac",
  "m4a",
  "wav",
  "opus",
  "stem.m4a",
  "aiff",
  "aac",
  "ogg",
  "wma",
  "ac3",
  "dsd",
  "dsf",
];

const foldersContent = document.getElementById("folders-content");
const totalFolders = document.getElementById("total-folders");
const errorBanner = useErrorBanner(document.getElementById("error-message"));
const searchInput = document.getElementById("search-input");
const searchBtn = document.getElementById("search-btn");
const addFolderBtn = document.getElementById("add-folder-btn");
const folderModal = document.getElementById("folder-modal");
const modalTitle = document.getElementById("modal-title");
const folderPath = document.getElementById("folder-path");
const scanRecursive = document.getElementById("scan-recursive");
const fixedExtensions = document.getElementById("fixed-extensions");
const extensionsContainer = document.getElementById("extensions-container");
const selectAllExtBtn = document.getElementById("select-all-ext-btn");
const deselectAllExtBtn = document.getElementById("deselect-all-ext-btn");
const saveFolderBtn = document.getElementById("save-folder-btn");
const cancelFolderBtn = document.getElementById("cancel-folder-btn");

// ── Extension helpers ──────────────────────────

function renderExtensionsCheckboxes() {
  let html = '<div class="extensions-grid">';
  for (const ext of ALL_EXTENSIONS) {
    html += `<label class="ext-checkbox">
      <input type="checkbox" value="${ext}" /> ${ext}
    </label>`;
  }
  html += "</div>";
  extensionsContainer.innerHTML = html;
}

function getSelectedExtensions() {
  const checked = extensionsContainer.querySelectorAll("input:checked");
  return Array.from(checked)
    .map((cb) => cb.value)
    .join(",");
}

function setSelectedExtensions(val) {
  const selected = val ? val.split(",").map((s) => s.trim()) : [];
  const checkboxes = extensionsContainer.querySelectorAll("input[type=checkbox]");
  for (const cb of checkboxes) {
    cb.checked = selected.includes(cb.value);
  }
}

function selectAllExtensions() {
  const checkboxes = extensionsContainer.querySelectorAll("input[type=checkbox]");
  for (const cb of checkboxes) {
    cb.checked = true;
  }
}

function deselectAllExtensions() {
  const checkboxes = extensionsContainer.querySelectorAll("input[type=checkbox]");
  for (const cb of checkboxes) {
    cb.checked = false;
  }
}

// ── Load & Render ──────────────────────────────

async function loadFolders() {
  foldersContent.innerHTML = renderLoading("Loading folders...");
  try {
    const data = await fetchJSON("/folders");
    foldersData = data.data || data;
    renderFolders(foldersData);
  } catch (err) {
    foldersContent.innerHTML = renderErrorBlock({
      title: "Failed to load folders",
      detail: err.message,
      retryFn: "loadFolders()",
    });
  }
}

function renderFolders(folders) {
  if (!folders || folders.length === 0) {
    totalFolders.textContent = "0";
    foldersContent.innerHTML = renderEmpty({
      icon: "folder-open",
      title: "No folders",
      message: "Add a music folder to start scanning local files.",
      actionHtml: `<button class="btn btn-primary" onclick="window.openAddFolderModal()"><i class="fas fa-plus"></i> Add Folder</button>`,
    });
    return;
  }

  totalFolders.textContent = folders.length;

  const rows = folders.map((f) => {
    const active = f.watchEnabled;
    const statusBadge = active
      ? `<span class="badge" style="background:rgba(16,185,129,0.15);color:#6ee7b7;border:1px solid rgba(16,185,129,0.3);">Active</span>`
      : `<span class="badge" style="background:rgba(100,116,139,0.15);color:#94a3b8;border:1px solid rgba(100,116,139,0.3);">Inactive</span>`;

    const configParts = [];
    if (f.scanRecursive) configParts.push("recursive");
    if (f.fixedExtensions && f.fileExtensions)
      configParts.push(`ext: ${f.fileExtensions}`);
    else if (f.fixedExtensions) configParts.push("fixed (none)");
    else configParts.push("all extensions");
    if (f.maxDepth > 0) configParts.push(`depth: ${f.maxDepth}`);
    const configSummary = configParts.join(", ") || "—";

    const lastScanned = f.lastScanned
      ? formatDateTime(new Date(f.lastScanned * 1000))
      : "Never";

    const actions = `
      <div class="flex gap-2" style="flex-wrap: nowrap;">
        <button class="btn btn-sm" onclick="window.scanFolder(${f.id})" title="Scan"><i class="fas fa-search"></i></button>
        <button class="btn btn-sm ${active ? "btn-yellow" : "btn-green"}" onclick="window.toggleWatch(${f.id})" title="${active ? "Pause" : "Resume"}">
          <i class="fas ${active ? "fa-pause" : "fa-play"}"></i>
        </button>
        <button class="btn btn-sm" onclick="window.openEditFolder(${f.id})" title="Edit"><i class="fas fa-edit"></i></button>
        <button class="btn btn-sm btn-red" onclick="window.deleteFolder(${f.id})" title="Delete"><i class="fas fa-trash"></i></button>
      </div>
    `;

    const pathCell = `<code style="font-size:0.82rem;">${f.path}</code>`;

    return (
      td(pathCell, { style: "max-width:360px;overflow:hidden;text-overflow:ellipsis;" }) +
      td(statusBadge) +
      td(`<strong>${f.fileCount}</strong>`) +
      td(configSummary) +
      td(lastScanned, { style: "white-space:nowrap;" }) +
      td(actions, { style: "white-space:nowrap;" })
    );
  });

  foldersContent.innerHTML = renderTable(
    ["Path", "Status", "Files", "Config", "Last Scanned", "Actions"],
    rows.join(""),
  );
}

// ── Modal functions ────────────────────────────

window.openAddFolderModal = function () {
  currentFolderId = null;
  modalTitle.textContent = "Add Folder";
  folderPath.value = "";
  scanRecursive.checked = true;
  fixedExtensions.checked = false;
  setSelectedExtensions("");
  folderModal.classList.add("open");
};

window.openEditFolder = function (id) {
  const folder = foldersData.find((f) => f.id === id);
  if (!folder) {
    errorBanner.showError("Folder not found");
    return;
  }
  currentFolderId = id;
  modalTitle.textContent = "Edit Folder";
  folderPath.value = folder.path || "";
  scanRecursive.checked = folder.scanRecursive;
  fixedExtensions.checked = folder.fixedExtensions;
  setSelectedExtensions(folder.fileExtensions || "");
  folderModal.classList.add("open");
};

function closeFolderModal() {
  folderModal.classList.remove("open");
  currentFolderId = null;
}

// ── Save / Delete / Scan / Toggle ─────────────

async function saveFolder() {
  const path = folderPath.value.trim();
  if (!path) {
    errorBanner.showError("Folder path is required");
    return;
  }

  saveFolderBtn.disabled = true;
  saveFolderBtn.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Saving...';

  try {
    if (currentFolderId) {
      const updateBody = {
        path: path,
        watchEnabled: true,
        scanRecursive: scanRecursive.checked,
        fixedExtensions: fixedExtensions.checked,
        fileExtensions: fixedExtensions.checked ? getSelectedExtensions() : "",
        maxDepth: 0,
      };
      await fetchJSON(`/folders/${currentFolderId}`, {
        method: "PUT",
        body: JSON.stringify(updateBody),
      });
    } else {
      const body = {
        path: path,
        watchEnabled: true,
        scanRecursive: scanRecursive.checked,
        fixedExtensions: fixedExtensions.checked,
        fileExtensions: fixedExtensions.checked ? getSelectedExtensions() : "",
        maxDepth: 0,
      };
      await fetchJSON("/folders", {
        method: "POST",
        body: JSON.stringify(body),
      });
    }
    closeFolderModal();
    await loadFolders();
  } catch (err) {
    errorBanner.showError(err.message);
  } finally {
    saveFolderBtn.disabled = false;
    saveFolderBtn.innerHTML = '<i class="fas fa-save"></i> Save';
  }
}

window.deleteFolder = async function (id) {
  const folder = foldersData.find((f) => f.id === id);
  const path = folder ? folder.path : id;
  if (!confirm(`Delete folder "${path}"? This will not delete any files on disk.`))
    return;

  try {
    await fetchJSON(`/folders/${id}`, { method: "DELETE" });
    await loadFolders();
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

window.scanFolder = async function (id) {
  try {
    await fetchJSON(`/folders/${id}/scan`, { method: "POST" });
    errorBanner.showError("Scan started");
    setTimeout(() => loadFolders(), 2000);
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

window.toggleWatch = async function (id) {
  try {
    await fetchJSON(`/folders/${id}/watch`, { method: "POST" });
    await loadFolders();
  } catch (err) {
    errorBanner.showError(err.message);
  }
};

// ── Filtering ──────────────────────────────────

// Search bar (shared component handles ENTER, button click, Cmd+F, Escape)
const searchBar = initSearchBar({
  onSearch: (term) => {
    const query = term.toLowerCase();
    if (!query) {
      renderFolders(foldersData);
      return;
    }
    const filtered = foldersData.filter((f) => f.path.toLowerCase().includes(query));
    renderFolders(filtered);
  },
});

// ── Init ───────────────────────────────────────

document.addEventListener("DOMContentLoaded", () => {
  // Render extension checkboxes once
  renderExtensionsCheckboxes();

  addFolderBtn?.addEventListener("click", window.openAddFolderModal);
  saveFolderBtn?.addEventListener("click", saveFolder);

  cancelFolderBtn?.addEventListener("click", closeFolderModal);

  selectAllExtBtn?.addEventListener("click", selectAllExtensions);
  deselectAllExtBtn?.addEventListener("click", deselectAllExtensions);

  // Close modal on backdrop click
  folderModal?.addEventListener("click", (e) => {
    if (e.target === folderModal) closeFolderModal();
  });

  // Expose loadFolders for retry
  window.loadFolders = loadFolders;

  // Initial load
  loadFolders();
});
