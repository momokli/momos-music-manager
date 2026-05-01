/**
 * auto-categorize.js — AI-powered tag categorization wizard.
 *
 * Layout:
 *   Progress bar
 *   ┌── TAG CARD ──────────────────────────────┐
 *   │  Name  #id                   X of Y   %   │
 *   │  [Spotify] [SoundCloud] [YouTube]         │
 *   └────────────────────────────────────────────┘
 *   ┌── CATEGORIES ─────────────────────────────┐
 *   │   ┌──────┐  ┌──────┐  ┌──────┐           │
 *   │   │ Vibe │  │ Mood │  │Mkml  │           │
 *   │   │ [1]  │  │ [2]  │  │ [3]  │           │
 *   │   └──────┘  └──────┘  └──────┘           │
 *   └────────────────────────────────────────────┘
 *   ┌── DEFAULT ────────────────────────────────┐
 *   │  Setlist                         [SPACE]   │
 *   └────────────────────────────────────────────┘
 *   [Skip]  (S)
 *
 * Keyboard shortcuts:
 *   1-3     Select category by grid position (Vibe=1, Mood=2, Merkmal=3)
 *   Space   Select default/setlist category
 *   Enter   Select AI-recommended category
 *   S       Skip tag
 *
 * Notes:
 *   - Phase is technical/prefilled — hidden from the wizard
 *   - AI recommends among Vibe, Mood, Merkmal only
 *
 * Speculative Prefetch ("Fork"):
 *   After showing a tag with its AI recommendation, the wizard
 *   pre-emptively does two things in the background:
 *     1. Optimistic PUT — categorizes the current tag with the
 *        recommendation, assuming the user will press Enter.
 *     2. Next-tag prefetch — fetches the suggestion for the next
 *        tag so it renders instantly.
 *   If the user picks a different category, a second PUT overwrites
 *   the optimistic one ("throw away the fork"). The prefetch is always
 *   correct because the queue order is linear.
 */
import { renderLoading, renderErrorBlock, renderBadge } from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

const S = (v) => ` style="${v}"`;
const qsa = (s, c) => c.querySelectorAll(s);

/* ── Color helpers ─────────────────────────────────────────────── */

function stringToColor(str, sat = "55%", light = "55%") {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = str.charCodeAt(i) + ((hash << 5) - hash);
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue}, ${sat}, ${light})`;
}

/* ── Rendering ─────────────────────────────────────────────────── */

function renderPage(
  tag,
  totalTags,
  currentIndex,
  categories,
  aiRecommendation,
  serviceConnections,
) {
  const pct = totalTags > 0 ? Math.round((currentIndex / totalTags) * 100) : 0;

  // Separate default category from the rest
  const defaultCat = categories.find((c) => c.isDefault);
  const gridCats = defaultCat ? categories.filter((c) => !c.isDefault) : categories;

  // Which category is AI-recommended?
  const aiCatId = aiRecommendation ? String(aiRecommendation.categoryId) : null;

  function isAi(cat) {
    return aiCatId && String(cat.id) === aiCatId;
  }

  // ── Tag card ──
  const services = serviceConnections || {};
  const serviceIcons = [
    { key: "spotify", icon: "fab fa-spotify", label: "Spotify" },
    { key: "soundcloud", icon: "fab fa-soundcloud", label: "SoundCloud" },
    { key: "youtube", icon: "fab fa-youtube", label: "YouTube" },
  ];
  const serviceHtml = serviceIcons
    .map((s) =>
      services[s.key]
        ? `<span class="tag-service active" title="${s.label}"><i class="${s.icon}"></i></span>`
        : `<span class="tag-service inactive" title="${s.label} not connected"><i class="${s.icon}" style="opacity:0.25"></i></span>`,
    )
    .join("");

  const tagCardHtml = `<div class="tag-card">
    <div class="tag-card-main">
      <div class="tag-card-left">
        <span class="tag-card-name">${tag.name}</span>
        <span class="tag-card-id">#${tag.id}</span>
      </div>
      <div class="tag-card-meta">
        <span class="tag-card-pos">${currentIndex + 1} of ${totalTags}</span>
        <span class="tag-card-pct">${pct}%</span>
      </div>
    </div>
    <div class="tag-card-services">
      ${serviceHtml}
    </div>
  </div>`;

  // ── Category grid ──
  let gridNum = 0;
  const gridCardsHtml = gridCats
    .map((cat) => {
      gridNum++;
      const ai = isAi(cat);
      const aiClass = ai ? " cat-btn-ai" : "";
      const aiStrip = ai
        ? `<div class="cat-btn-ai-strip">
        <span class="ai-pct">AI ${aiRecommendation.confidence}%</span>
        <span class="ai-enter">Enter</span>
      </div>`
        : `<div class="cat-btn-ai-strip cat-btn-ai-strip--empty"></div>`;
      return `<button class="cat-btn${aiClass}" data-category="${cat.id}"${S(`border-color:${cat.color}`)}>
      <div class="cat-btn-top">
        <div class="cat-btn-header">
          <div class="cat-btn-icon"${S(`background:${cat.color}20;color:${cat.color}`)}><i class="${cat.icon}"></i></div>
          <span class="cat-btn-label">${cat.label}</span>
        </div>
        <div class="cat-btn-footer">
          <kbd>${gridNum}</kbd>
        </div>
      </div>
      ${aiStrip}
    </button>`;
    })
    .join("");

  const gridHtml = gridCardsHtml
    ? `<div class="cat-grid-row">${gridCardsHtml}</div>`
    : "";

  // ── Default row ──
  const defaultHtml = defaultCat
    ? `<div class="cat-default-row">
        <button class="cat-btn cat-btn-default${isAi(defaultCat) ? " cat-btn-ai" : ""}" data-category="${defaultCat.id}"${S(`border-color:${defaultCat.color}`)}>
          <div class="cat-btn-header">
            <div class="cat-btn-icon"${S(`background:${defaultCat.color}20;color:${defaultCat.color}`)}><i class="${defaultCat.icon}"></i></div>
            <span class="cat-btn-label">${defaultCat.label}</span>
            ${isAi(defaultCat) ? renderBadge(`${aiRecommendation.confidence}%`, "var(--purple)") : ""}
          </div>
          <div class="cat-btn-footer">
            <kbd>SPACE</kbd>
          </div>
        </button>
      </div>`
    : "";

  // ── Assemble ──
  return `<div class="auto-categorize-page">
  <div class="auto-categorize-inner">
    <div class="auto-cat-canvas">
      ${tagCardHtml}
      ${gridHtml}
      ${defaultHtml}
      <div class="auto-cat-footer">
        <button class="btn btn-sm skip-btn"><i class="fas fa-forward"></i> Skip <span class="text-xs text-muted">(S)</span></button>
        <div class="shortcuts-hint">
          <span><kbd>Space</kbd> Default</span>
          <span><kbd>1-${Math.min(gridCats.length, 9)}</kbd> Cat</span>
          <span><kbd>S</kbd> Skip</span>
          <span><kbd>Enter</kbd> AI</span>
        </div>
      </div>
    </div>
  </div>
</div>`;
}

/* ── Speculative Prefetch ("Fork") ────────────────────────────── */

/**
 * After rendering a tag, fires off background work:
 *
 *   1. Optimistic PUT — pre-categorizes the current tag with the AI
 *      recommendation (assumes the user will press Enter).
 *   2. Next-tag prefetch — fetches the suggestion for the next tag
 *      so it can be rendered instantly when the user moves on.
 *
 * If the user picks a different category, the optimistic PUT is
 * overridden by the real choice ("throw away the fork").
 *
 * @param {object} state
 * @param {Array} tagQueue
 * @param {number} [nextIndex] — if provided, prefetch that specific index
 */
function startSpeculativePrefetch(state, tagQueue, nextIndex) {
  // ── Fork 1: Optimistic pre-categorize ──
  // Only fire once per tag (skip if we already sent it for this tag)
  const currentTag = state.currentTag;
  if (currentTag && state.aiRecommendation && state.aiRecommendation.categoryId) {
    if (state.optimisticTagId !== currentTag.id) {
      state.optimisticTagId = currentTag.id;
      state.optimisticCatId = state.aiRecommendation.categoryId;

      // Fire-and-forget: put the tag with the recommended category.
      // If the user picks differently, a second PUT will overwrite it.
      fetchJSON(`/api/tags/${currentTag.id}/categorize`, {
        method: "PUT",
        body: JSON.stringify({ categoryId: state.optimisticCatId }),
        signal: state.signal,
      }).catch((err) => {
        if (err.name === "AbortError") return;
        // Non-critical — user will send a real PUT on interaction
        console.warn("Optimistic pre-categorize failed:", err);
      });
    }
  }

  // ── Fork 2: Prefetch next tag's suggestion ──
  const idx = nextIndex != null ? nextIndex : state.currentIndex + 1;
  if (idx < tagQueue.length) {
    const nextTag = tagQueue[idx];
    // Avoid duplicate in-flight requests
    if (!state.prefetchInFlight[nextTag.id] && !state.prefetchCache[nextTag.id]) {
      state.prefetchInFlight[nextTag.id] = true;

      fetchJSON(`/api/tags/${nextTag.id}/suggest`, { signal: state.signal })
        .then((resp) => {
          state.prefetchCache[nextTag.id] = resp;
          delete state.prefetchInFlight[nextTag.id];
        })
        .catch((err) => {
          delete state.prefetchInFlight[nextTag.id];
          if (err.name === "AbortError") return;
          console.warn("Prefetch failed for tag", nextTag.id, ":", err);
        });
    }
  }
}

/* ── Category helpers ──────────────────────────────────────────── */

function resetAll(container) {
  qsa(".cat-btn.selected", container).forEach((b) => b.classList.remove("selected"));
  qsa(".cat-btn .fa-check", container).forEach((c) => c.remove());
}

function selectBtn(btn) {
  btn.classList.add("selected");
  const check = document.createElement("i");
  check.className = "fas fa-check";
  check.style.cssText = "color:var(--pink);font-size:0.75rem";
  btn.appendChild(check);
}

async function selectCategory(container, state, tagQueue, categories, btn) {
  if (!btn || btn.disabled) return;
  resetAll(container);
  selectBtn(btn);

  const catId = parseInt(btn.dataset.category);
  if (isNaN(catId) || !state.currentTag) return;

  const tagId = state.currentTag.id;

  // Send the authoritative PUT on every explicit user interaction.
  // The optimistic PUT is just a background bonus — if it succeeded,
  // this second PUT is idempotent and cheap (simple DB write).
  // If it failed, this ensures correctness ("throw away the fork").
  try {
    await fetchJSON(`/api/tags/${tagId}/categorize`, {
      method: "PUT",
      body: JSON.stringify({ categoryId: catId }),
      signal: state.signal,
    });
  } catch (err) {
    if (err.name === "AbortError") return;
    console.error("Categorize failed:", err);
    return; // Don't advance if the real PUT failed
  }

  state.currentIndex++;
  if (state.signal.aborted) return;
  await loadNextTag(container, state, tagQueue, categories);
}

async function skipTag(container, state, tagQueue, categories) {
  state.currentIndex++;
  if (state.signal.aborted) return;
  await loadNextTag(container, state, tagQueue, categories);
}

/* ── Event wiring ─────────────────────────────────────────────── */

function setupEvents(container, state, tagQueue, categories) {
  container.addEventListener("click", async (e) => {
    const btn = e.target.closest("[data-category]");
    if (btn) {
      e.preventDefault();
      await selectCategory(container, state, tagQueue, categories, btn);
      return;
    }
    if (e.target.closest(".skip-btn")) {
      await skipTag(container, state, tagQueue, categories);
      return;
    }
    if (e.target.closest(".reset-btn")) {
      resetAll(container);
    }
  });

  container.addEventListener("keydown", async (e) => {
    const key = e.key;

    if (key === " " || key === "Spacebar") {
      e.preventDefault();
      const defaultBtn = container.querySelector(".cat-btn-default");
      if (defaultBtn) {
        await selectCategory(container, state, tagQueue, categories, defaultBtn);
      }
      return;
    }

    if (key >= "1" && key <= "9") {
      e.preventDefault();
      const idx = parseInt(key) - 1;
      const catBtns = container.querySelectorAll(".cat-grid-row .cat-btn");
      if (catBtns[idx]) {
        await selectCategory(container, state, tagQueue, categories, catBtns[idx]);
      }
      return;
    }

    if (key === "s" || key === "S") {
      e.preventDefault();
      await skipTag(container, state, tagQueue, categories);
      return;
    }

    if (key === "Enter") {
      e.preventDefault();
      const aiBtn = container.querySelector(".cat-btn-ai");
      if (aiBtn) {
        await selectCategory(container, state, tagQueue, categories, aiBtn);
      }
      return;
    }
  });

  container.addEventListener("click", () => container.focus());
}

/* ── Load next tag ─────────────────────────────────────────────── */

function mapCategories(apiCategories) {
  // Preferred order for grid shortcuts: Vibe(1), Mood(2), Merkmal(3)
  const preferredOrder = ["Vibe", "Mood", "Merkmal"];
  const mapped = apiCategories.map((c) => {
    const color = stringToColor(c.name || c.label || "");
    return {
      id: String(c.id),
      label: c.name,
      icon: c.icon || "fa-solid fa-tag",
      color,
      isDefault: !!c.isDefault,
      sortKey: preferredOrder.indexOf(c.name),
    };
  });
  // Sort non-default cats by preferred order, default stays in place
  return mapped.sort((a, b) => {
    if (a.isDefault) return 1;
    if (b.isDefault) return -1;
    return a.sortKey - b.sortKey;
  });
}

async function loadNextTag(container, state, tagQueue, fallbackCategories) {
  if (state.signal.aborted) return;

  if (state.currentIndex >= tagQueue.length) {
    container.innerHTML = `
    <div class="empty-state">
        <div class="empty-icon"><i class="fas fa-check-circle" style="color:var(--green);font-size:3rem"></i></div>
        <h3>All ${tagQueue.length} tags reviewed</h3>
        <p>You've categorized all unreviewed tags.</p>
        <a href="#tags" class="btn btn-primary"><i class="fas fa-tags"></i> View Tags</a>
      </div>`;
    return;
  }

  const currentTag = tagQueue[state.currentIndex];
  state.currentTag = currentTag;
  // Reset optimistic state for the new tag
  // (optimisticTagId stays set until startSpeculativePrefetch fires
  //  for a different tag, which prevents duplicate PUTs)

  let suggestResp;

  // ── Check prefetch cache first (the "fork" paid off) ──
  if (state.prefetchCache[currentTag.id]) {
    suggestResp = state.prefetchCache[currentTag.id];
    delete state.prefetchCache[currentTag.id];
    delete state.prefetchInFlight[currentTag.id];
  } else {
    // No prefetch (first tag, or prefetch failed) — do normal fetch
    try {
      suggestResp = await fetchJSON(`/api/tags/${currentTag.id}/suggest`, {
        signal: state.signal,
      });
    } catch (err) {
      if (err.name === "AbortError") return;
      // Fallback rendering without AI recommendation
      const displayCats = mapCategories(
        fallbackCategories.length > 0
          ? fallbackCategories
          : [{ id: 1, name: "Setlist", icon: "fa-solid fa-list", isDefault: true }],
      );
      state.aiRecommendation = null;
      container.innerHTML = renderPage(
        currentTag,
        tagQueue.length,
        state.currentIndex,
        displayCats,
        null,
        null,
      );
      container.focus();
      // Still try to prefetch the next tag even on error
      startSpeculativePrefetch(state, tagQueue);
      return;
    }
  }

  if (state.signal.aborted) return;

  // Build AI recommendation from response
  const aiRec = suggestResp.data
    ? {
        category: suggestResp.data.suggestedCategoryName,
        confidence: Math.round(suggestResp.data.confidence * 100),
        categoryId: suggestResp.data.suggestedCategoryId,
      }
    : null;

  // Store the recommendation so speculative prefetch can use it
  state.aiRecommendation = aiRec;

  const apiCategories = suggestResp.data?.allCategories || fallbackCategories;
  const services = suggestResp.data?.serviceConnections || null;
  const displayCats = mapCategories(apiCategories);

  container.innerHTML = renderPage(
    currentTag,
    tagQueue.length,
    state.currentIndex,
    displayCats,
    aiRec,
    services,
  );
  container.focus();

  // ── Kick off speculative prefetch for the NEXT tag ──
  startSpeculativePrefetch(state, tagQueue);
}

/* ── Init ──────────────────────────────────────────────────────── */

export async function init(container, signal) {
  container.tabIndex = -1;
  container.innerHTML = renderLoading("Loading auto-categorize...");
  if (signal.aborted) return;

  try {
    const resp = await fetchJSON("/api/tags/unreviewed", { signal });
    if (signal.aborted) return;

    const { queue, totalUnreviewed, totalReviewed } = resp.data;

    if (!queue || queue.length === 0) {
      container.innerHTML = `
        <div class="empty-state">
          <div class="empty-icon"><i class="fas fa-check-circle" style="color:var(--green);font-size:3rem"></i></div>
          <h3>All tags reviewed</h3>
          <p>${totalReviewed} tags have been reviewed. No unreviewed tags remain.</p>
          <a href="#tags" class="btn btn-primary"><i class="fas fa-tags"></i> View Tags</a>
        </div>`;
      return;
    }

    const state = {
      currentIndex: 0,
      currentTag: null,
      signal,
      // ── Speculative prefetch state ──
      prefetchCache: {}, // tagId → response data (resolved)
      prefetchInFlight: {}, // tagId → true (track which are in flight)
      optimisticTagId: null, // tagId we optimistically pre-categorized
      optimisticCatId: null, // categoryId we optimistically set
      aiRecommendation: null, // current tag's AI recommendation
    };

    await loadNextTag(container, state, queue, []);
    setupEvents(container, state, queue, []);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load Auto-Categorize",
      detail: err.message || "Unknown error",
      retryFn: "window.location.hash='#auto-categorize'",
    });
  }
}
