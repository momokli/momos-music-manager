const API_BASE = "http://localhost:3000/api";

let state = {
  categories: [], // [{id, name, icon, isDefault}]
  results: [], // [{name, status, tagId, categoryId, categoryName, currentCategoryId, currentCategoryName}]
  isProcessing: false,
};

const $ = (id) => document.getElementById(id);
const dom = {
  categoriesContainer: $("categories-container"),
  importBtn: $("import-btn"),
  resultsContainer: $("results-container"),
  loading: $("loading-state"),
  error: $("error-message"),
};

function showError(msg) {
  dom.error.textContent = msg;
  dom.error.style.display = "block";
  setTimeout(() => {
    dom.error.style.display = "none";
  }, 6000);
}

async function fetchJson(url, options = {}) {
  const res = await fetch(url, {
    headers: { "Content-Type": "application/json", ...options.headers },
    ...options,
  });
  if (!res.ok) {
    const errBody = await res.json().catch(() => ({}));
    throw new Error(errBody.error || `HTTP ${res.status}`);
  }
  return res.json();
}

async function loadCategories() {
  const data = await fetchJson(`${API_BASE}/tag-categories`);
  state.categories = data.data;
}

function renderCategoryBoxes() {
  dom.categoriesContainer.innerHTML = "";

  for (const cat of state.categories) {
    const card = document.createElement("div");
    card.className = "category-card";
    card.innerHTML = `
            <div class="category-header">
                ${cat.icon ? `<i class="fas fa-${cat.icon.toLowerCase()}"></i>` : '<i class="fas fa-tag"></i>'}
                <span class="category-name">${cat.name}</span>
                ${cat.isDefault ? '<span class="default-badge">default</span>' : ""}
            </div>
            <textarea
                class="tag-textarea"
                data-category-id="${cat.id}"
                data-category-name="${cat.name}"
                placeholder="Paste tags here, one per line...
        ${cat.isDefault ? "e.g. my-setlist-name" : "e.g. nostalgic\ndark\nsexy"}"
                rows="6"
            ></textarea>
            <div class="tag-count" id="count-${cat.id}">0 tags</div>
        `;
    dom.categoriesContainer.appendChild(card);
  }

  // Add live tag count
  document.querySelectorAll(".tag-textarea").forEach((ta) => {
    ta.addEventListener("input", () => {
      const count = ta.value.split("\n").filter((l) => l.trim() !== "").length;
      const countEl = document.getElementById(`count-${ta.dataset.categoryId}`);
      if (countEl) countEl.textContent = `${count} tags`;
    });
  });
}

function collectEntries() {
  const entries = [];
  document.querySelectorAll(".tag-textarea").forEach((ta) => {
    const categoryId = parseInt(ta.dataset.categoryId);
    const lines = ta.value
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l !== "" && !l.startsWith("//") && !l.startsWith("#"));
    for (const line of lines) {
      entries.push({ name: line, categoryId });
    }
  });
  return entries;
}

function renderResults(results) {
  const container = dom.resultsContainer;
  container.innerHTML = `<div class="results-header"><i class="fas fa-clipboard-list"></i> Import Results</div>`;
  container.style.display = "block";

  // Stats
  const matched = results.filter((r) => r.status === "matched").length;
  const conflicts = results.filter((r) => r.status === "conflict").length;
  const notFound = results.filter((r) => r.status === "not_found").length;

  const statsDiv = document.createElement("div");
  statsDiv.className = "results-stats";
  statsDiv.innerHTML = `
        <div class="stat stat-matched">
            <i class="fas fa-check-circle"></i>
            <span class="stat-count">${matched}</span> already good
        </div>
        <div class="stat stat-conflict">
            <i class="fas fa-exclamation-triangle"></i>
            <span class="stat-count">${conflicts}</span> conflicts
        </div>
        <div class="stat stat-new">
            <i class="fas fa-plus-circle"></i>
            <span class="stat-count">${notFound}</span> new tags
        </div>
    `;
  container.appendChild(statsDiv);

  // Group by category
  const byCategory = {};
  for (const r of results) {
    const catName = r.categoryName || "Unknown";
    if (!byCategory[catName]) byCategory[catName] = [];
    byCategory[catName].push(r);
  }

  for (const [catName, entries] of Object.entries(byCategory)) {
    // Separator per status
    for (const status of ["matched", "conflict", "not_found"]) {
      const filtered = entries.filter((e) => e.status === status);
      if (filtered.length === 0) continue;

      const section = document.createElement("div");
      section.className = `result-section result-${status}`;

      const headerText =
        status === "matched"
          ? "✅ Already good"
          : status === "conflict"
            ? "↩️ Already exists (different category)"
            : "⚠️ Not found — create?";

      section.innerHTML = `<div class="result-section-header">${headerText} — ${catName}</div>`;

      const list = document.createElement("div");
      list.className = "result-list";

      for (const entry of filtered) {
        const item = document.createElement("div");
        item.className = `result-item result-item-${entry.status}`;
        item.dataset.name = entry.name;
        item.dataset.categoryId = entry.categoryId;

        if (entry.status === "matched") {
          item.innerHTML = `
                        <span class="result-name">${escapeHtml(entry.name)}</span>
                        <span class="result-badge badge-matched">✅ ${escapeHtml(catName)}</span>
                    `;
        } else if (entry.status === "conflict") {
          const currentCat = entry.currentCategoryName || "Unknown";
          item.innerHTML = `
                        <span class="result-name">${escapeHtml(entry.name)}</span>
                        <span class="result-badge badge-conflict">Currently: ${escapeHtml(currentCat)}</span>
                        <div class="result-actions">
                            <button class="btn-action btn-keep" data-action="keep">Keep as ${escapeHtml(currentCat)}</button>
                            <button class="btn-action btn-move" data-action="move" data-target-cat="${entry.categoryId}">Move to ${escapeHtml(catName)}</button>
                            <button class="btn-action btn-setlist" data-action="move-setlist">Move to Setlist</button>
                        </div>
                    `;
        } else {
          item.innerHTML = `
                        <span class="result-name">${escapeHtml(entry.name)}</span>
                        <span class="result-badge badge-new">New</span>
                        <div class="result-actions">
                            <button class="btn-action btn-create" data-action="create">Create as ${escapeHtml(catName)}</button>
                            <button class="btn-action btn-setlist" data-action="create-setlist">Create as Setlist</button>
                        </div>
                    `;
        }

        list.appendChild(item);
      }

      section.appendChild(list);
      container.appendChild(section);
    }
  }

  // Attach action handlers
  container.querySelectorAll("[data-action]").forEach((btn) => {
    btn.addEventListener("click", handleAction);
  });
}

function escapeHtml(str) {
  const div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}

function getSetlistCategoryId() {
  const setlist = state.categories.find((c) => c.isDefault);
  return setlist ? setlist.id : null;
}

async function handleAction(e) {
  if (state.isProcessing) return;
  state.isProcessing = true;

  const btn = e.currentTarget;
  const item = btn.closest(".result-item");
  const name = item.dataset.name;
  const action = btn.dataset.action;
  const categoryId = parseInt(item.dataset.categoryId);
  const setlistId = getSetlistCategoryId();

  let resolveAction, targetCategoryId;

  switch (action) {
    case "keep":
      // Do nothing, just visually remove
      item.style.opacity = "0.4";
      item.querySelector(".result-actions").innerHTML =
        '<span class="kept-badge">Kept</span>';
      state.isProcessing = false;
      return;
    case "move":
      resolveAction = "move";
      targetCategoryId = categoryId;
      break;
    case "move-setlist":
      resolveAction = "move";
      targetCategoryId = setlistId;
      break;
    case "create":
      resolveAction = "create";
      targetCategoryId = categoryId;
      break;
    case "create-setlist":
      resolveAction = "create";
      targetCategoryId = setlistId;
      break;
    default:
      state.isProcessing = false;
      return;
  }

  try {
    const data = await fetchJson(`${API_BASE}/tags/bulk-resolve`, {
      method: "POST",
      body: JSON.stringify({
        entries: [{ name, categoryId: targetCategoryId, action: resolveAction }],
      }),
    });

    const result = data.data[0];
    if (result.status === "error") {
      showError(`Failed: ${result.error}`);
    } else {
      item.style.opacity = "0.4";
      const statusIcon =
        result.status === "created"
          ? "✨ Created"
          : result.status === "moved"
            ? "↩️ Moved"
            : "✅ Reviewed";
      item.querySelector(".result-actions").innerHTML =
        `<span class="kept-badge">${statusIcon}</span>`;
    }
  } catch (err) {
    showError(`Error: ${err.message}`);
  } finally {
    state.isProcessing = false;
  }
}

async function handleImport() {
  if (state.isProcessing) return;

  const entries = collectEntries();
  if (entries.length === 0) {
    showError("No tags to import — paste some tags first!");
    return;
  }

  state.isProcessing = true;
  dom.importBtn.disabled = true;
  dom.importBtn.textContent = "⏳ Checking...";

  try {
    const data = await fetchJson(`${API_BASE}/tags/bulk-import`, {
      method: "POST",
      body: JSON.stringify({ entries }),
    });
    state.results = data.data;
    renderResults(data.data);
  } catch (err) {
    showError(`Import failed: ${err.message}`);
  } finally {
    state.isProcessing = false;
    dom.importBtn.disabled = false;
    dom.importBtn.textContent = "🚀 Import All";
  }
}

async function init() {
  try {
    dom.loading.style.display = "block";
    await loadCategories();
    renderCategoryBoxes();
    dom.loading.style.display = "none";
    dom.importBtn.addEventListener("click", handleImport);
  } catch (err) {
    showError(`Init failed: ${err.message}`);
    dom.loading.innerHTML = `<div style="color: #fca5a5;">❌ ${err.message}</div>`;
  }
}

document.addEventListener("DOMContentLoaded", init);
