// auto-categorize.js — Wizard für semantische Tag-Kategorisierung
// Queue-Management (Option B): Skip rotiert innerhalb der lokalen Queue,
// kein API-Call nötig. Kategorisieren entfernt den Tag aus der Queue + persistiert.

const API_BASE = "http://localhost:3000/api";

// ─── State ────────────────────────────────────────────────────────────────────

let state = {
  queue: [], // Array von {id, name} — die aktuelle Queue
  currentIndex: 0, // Zeiger auf aktuelles Element in queue[]
  totalUnreviewed: 0, // Initialer Gesamtwert (für Progress)
  allCategories: [], // [{id, name, icon, isDefault}]
  isProcessing: false, // Verhindert Doppelklicks
};

// ─── DOM References ───────────────────────────────────────────────────────────

const $ = (id) => document.getElementById(id);
const dom = {
  error: $("error-message"),
  loading: $("loading-state"),
  done: $("done-state"),
  doneTotal: $("done-total"),
  wizard: $("wizard-card"),
  progressLabel: $("progress-label"),
  progressPct: $("progress-pct"),
  progressFill: $("progress-fill"),
  tagName: $("current-tag-name"),
  tagId: $("current-tag-id"),
  aiSection: $("ai-section"),
  aiName: $("ai-category-name"),
  aiConfidence: $("ai-confidence"),
  aiBtn: $("btn-ai"),
  categoryGrid: $("category-grid"),
  skipBtn: $("btn-skip"),
  resetBtn: $("btn-reset"),
};

// ─── Utilities ────────────────────────────────────────────────────────────────

function showError(msg) {
  dom.error.textContent = msg;
  dom.error.style.display = "block";
  setTimeout(() => {
    dom.error.style.display = "none";
  }, 6000);
}

function updateProgress(current, total) {
  const pct = total > 0 ? Math.round((current / total) * 100) : 0;
  dom.progressLabel.textContent = `Tag ${current} / ${total}`;
  dom.progressPct.textContent = `${pct}%`;
  dom.progressFill.style.width = `${pct}%`;
}

// Map: Category-Name → CSS-Klasse
const CATEGORY_CSS = {
  Phase: "phase",
  Mood: "mood",
  Vibe: "vibe",
  Merkmal: "merkmal",
  Setlist: "setlist",
};

function cssClassForCategory(name) {
  return CATEGORY_CSS[name] || "setlist";
}

// ─── API Calls ────────────────────────────────────────────────────────────────

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

async function loadQueue() {
  const data = await fetchJson(`${API_BASE}/tags/unreviewed`);
  // data = { data: { totalUnreviewed, totalReviewed, queue: [{id, name}] } }
  const q = data.data;
  state.queue = q.queue;
  state.totalUnreviewed = q.totalUnreviewed;
}

async function loadCategories() {
  const data = await fetchJson(`${API_BASE}/tag-categories`);
  state.allCategories = data.data;
}

async function loadSuggestion(tagId) {
  const data = await fetchJson(`${API_BASE}/tags/${tagId}/suggest`);
  return data.data; // { suggestedCategoryId, suggestedCategoryName, confidence, allCategories }
}

async function categorizeTag(tagId, categoryId) {
  await fetchJson(`${API_BASE}/tags/${tagId}/categorize`, {
    method: "PUT",
    body: JSON.stringify({ categoryId: categoryId }),
  });
}

async function resetAllReviewed() {
  // TODO: Backend-Endpoint für globales Reset aller reviewed_at
  // Aktuell: wir machen das direkt per POST auf einen neuen Endpoint
  // Fallback: manuelles SQL (für POC reicht Page-Reload)
  showError("Reset not yet implemented — restart backend after clearing DB");
}

// ─── Rendering ────────────────────────────────────────────────────────────────

function renderCategoryButtons(suggestion) {
  const cats = state.allCategories;
  dom.categoryGrid.innerHTML = "";

  for (const cat of cats) {
    const btn = document.createElement("button");
    const label = cat.icon
      ? `<i class="fas fa-${cat.icon.toLowerCase()}"></i> ${cat.name}`
      : cat.name;
    btn.innerHTML = label;
    btn.className = `btn-category ${cssClassForCategory(cat.name)}`;
    btn.dataset.categoryId = cat.id;
    btn.addEventListener("click", () => handleCategorize(cat.id));
    dom.categoryGrid.appendChild(btn);
  }
}

function renderCurrentTag() {
  const tag = state.queue[state.currentIndex];
  if (!tag) {
    showDone();
    return;
  }
  dom.tagName.textContent = tag.name;
  dom.tagId.textContent = `ID: ${tag.id}`;

  const done = state.totalUnreviewed - state.queue.length + state.currentIndex;
  updateProgress(done, state.totalUnreviewed);

  // AI-Empfehlung laden
  loadSuggestion(tag.id)
    .then((suggestion) => {
      dom.aiName.textContent = suggestion.suggestedCategoryName;
      const pct = Math.round(suggestion.confidence * 100);
      dom.aiConfidence.textContent = `(${pct}%)`;

      // Update allCategories falls nötig
      if (state.allCategories.length === 0) {
        state.allCategories = suggestion.allCategories;
        renderCategoryButtons(suggestion);
      }

      // AI-Button klickbar machen
      dom.aiBtn.onclick = () => handleCategorize(suggestion.suggestedCategoryId);
      dom.aiSection.style.display = "block";
    })
    .catch((err) => {
      dom.aiName.textContent = "Keine Empfehlung";
      dom.aiConfidence.textContent = "";
      dom.aiBtn.onclick = null;
      dom.aiSection.style.display = "block";
    });
}

function showDone() {
  dom.wizard.style.display = "none";
  dom.done.style.display = "block";
  dom.doneTotal.textContent = state.totalUnreviewed;
}

function showWizard() {
  dom.loading.style.display = "none";
  dom.done.style.display = "none";
  dom.wizard.style.display = "block";
  dom.aiSection.style.display = "none"; // Wird nach Suggestion sichtbar
}

// ─── Actions ──────────────────────────────────────────────────────────────────

async function handleCategorize(categoryId) {
  if (state.isProcessing) return;
  state.isProcessing = true;

  const tag = state.queue[state.currentIndex];
  if (!tag || !tag.id) {
    state.isProcessing = false;
    return;
  }

  try {
    await categorizeTag(tag.id, categoryId);
    // Entferne aktuellen Tag aus Queue
    state.queue.splice(state.currentIndex, 1);
    // currentIndex bleibt, zeigt jetzt auf den nächsten Tag
    if (state.queue.length === 0) {
      showDone();
    } else {
      renderCurrentTag();
    }
  } catch (err) {
    showError(`Fehler beim Kategorisieren: ${err.message}`);
  } finally {
    state.isProcessing = false;
  }
}

function handleSkip() {
  if (state.queue.length <= 1) return; // nichts zu tun

  // Aktuelles Element ans Ende rotieren
  const skipped = state.queue.splice(state.currentIndex, 1)[0];
  state.queue.push(skipped);
  // currentIndex bleibt (zeigt jetzt auf das nächste Element)
  renderCurrentTag();
}

async function handleReset() {
  if (
    !confirm(
      "Wirklich alle reviewed_at-Status zurücksetzen?\n\n" +
        "Du musst danach ALLE Tags erneut kategorisieren.",
    )
  ) {
    return;
  }

  try {
    await fetchJson(`${API_BASE}/embeddings/reset-review`, { method: "POST" });
    window.location.reload();
  } catch (err) {
    showError(`Reset fehlgeschlagen: ${err.message}`);
  }
}

// ─── Init ─────────────────────────────────────────────────────────────────────

async function init() {
  try {
    // Queue und Kategorien parallel laden
    const [queueResult, catResult] = await Promise.all([loadQueue(), loadCategories()]);

    if (state.queue.length === 0) {
      showDone();
      return;
    }

    // Kategorie-Buttons rendern (statischer Teil)
    renderCategoryButtons(null);

    // Wizard anzeigen
    showWizard();

    // Ersten Tag rendern
    renderCurrentTag();

    // Event-Listener für Buttons
    dom.skipBtn.addEventListener("click", handleSkip);
    dom.resetBtn.addEventListener("click", handleReset);

    // Tastatur-Shortcuts
    document.addEventListener("keydown", (e) => {
      if (e.key === "s" || e.key === "S") {
        // Skip
        dom.skipBtn.click();
      } else if (e.key === "Enter" && dom.aiBtn.onclick) {
        // Enter drückt den AI-Button
        if (dom.aiBtn.style.display !== "none") {
          dom.aiBtn.click();
        }
      } else if (e.key >= "1" && e.key <= "9") {
        // Ziffern 1-9 für Kategorie-Buttons
        const idx = parseInt(e.key) - 1;
        const btns = dom.categoryGrid.querySelectorAll(".btn-category");
        if (idx < btns.length) {
          btns[idx].click();
        }
      }
    });
  } catch (err) {
    showError(`Init-Fehler: ${err.message}`);
    dom.loading.innerHTML = `
      <div style="color: #fca5a5; padding: 20px;">
        <i class="fas fa-exclamation-triangle"></i>
        <p>Fehler beim Laden: ${err.message}</p>
        <button onclick="window.location.reload()" style="
          padding: 8px 16px; background: #3b82f6; color: white;
          border: none; border-radius: 6px; cursor: pointer;
          margin-top: 12px;
        ">Nochmal versuchen</button>
      </div>
    `;
  }
}

// ─── Start ────────────────────────────────────────────────────────────────────

document.addEventListener("DOMContentLoaded", init);
