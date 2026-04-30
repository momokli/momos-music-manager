/**
 * comment-writer.js — Shared comment write options panel
 *
 * Renders a box with filter options for the "write comment diffs" task:
 *   - Only linked files (checkbox)
 *   - Tag picker (like the filter-by-tag on the files page)
 *   - Only non-default categories (checkbox)
 *   - Execute button
 *
 * Exports:
 *   renderCommentWriter(state) → HTML string
 *   wireCommentWriter(container, signal, onExecute) → state object (mutable)
 *
 * The onExecute callback receives (linkedOnly, tagNames, nonDefaultOnly).
 */

import { fetchJSON } from "./api.js";

/**
 * Render the comment-writer options panel.
 * @param {{ linkedOnly: boolean, tagNames: string[], nonDefaultOnly: boolean }} state
 * @returns {string} HTML
 */
export function renderCommentWriter(state) {
  const chipsHtml = (state.tagNames || [])
    .map(
      (t) =>
        `<span class="tag-chip" data-cw-tag="${t}">${t} <i class="fas fa-times tag-chip-x"></i></span>`,
    )
    .join("");

  return `
    <div class="comment-writer-panel" id="cw-panel">
      <div class="cw-options">
        <label class="checkbox-label cw-checkbox">
          <input type="checkbox" id="cw-linked-only" ${state.linkedOnly ? "checked" : ""}>
          <span>Only linked files</span>
        </label>
        <label class="checkbox-label cw-checkbox">
          <input type="checkbox" id="cw-non-default" ${state.nonDefaultOnly ? "checked" : ""}>
          <span>ignore files with only default tags</span>
        </label>
      </div>
      <div class="cw-tags">
        <div class="tag-search-wrap cw-tag-search">
          <i class="fas fa-tag"></i>
          <input type="text" class="input-text input-search" id="cw-tag-search"
                 placeholder="filter by TAG" autocomplete="off">
          <div class="tag-dropdown" id="cw-tag-dropdown"></div>
        </div>
        <div class="tag-chips" id="cw-tag-chips">${chipsHtml}</div>
      </div>
      <button class="btn btn-sm btn-green" id="cw-execute" style="width:100%;">
        <i class="fa-solid fa-cloud-arrow-up"></i> Write Comments
      </button>
    </div>`;
}

/**
 * Wire events for the comment-writer panel.
 * Returns a mutable state object that updates as the user interacts.
 *
 * @param {HTMLElement} container - The element containing the panel
 * @param {AbortSignal} signal - Abort signal for cleanup
 * @param {(linkedOnly: boolean, tagNames: string[], nonDefaultOnly: boolean) => void} onExecute
 * @returns {{ linkedOnly: boolean, tagNames: string[], nonDefaultOnly: boolean }}
 */
export function wireCommentWriter(container, signal, onExecute) {
  const state = {
    linkedOnly: false,
    tagNames: [],
    nonDefaultOnly: false,
  };

  // Read initial checkbox states
  const linkedCb = container.querySelector("#cw-linked-only");
  const nonDefCb = container.querySelector("#cw-non-default");
  if (linkedCb) state.linkedOnly = linkedCb.checked;
  if (nonDefCb) state.nonDefaultOnly = nonDefCb.checked;

  // ── Checkbox changes ──
  if (linkedCb) {
    linkedCb.addEventListener(
      "change",
      () => {
        state.linkedOnly = linkedCb.checked;
      },
      { signal },
    );
  }
  if (nonDefCb) {
    nonDefCb.addEventListener(
      "change",
      () => {
        state.nonDefaultOnly = nonDefCb.checked;
      },
      { signal },
    );
  }

  // ── Tag search with dropdown ──
  const tagSearch = container.querySelector("#cw-tag-search");
  const tagDropdown = container.querySelector("#cw-tag-dropdown");
  let timer;
  let selectedIndex = -1;

  function updateSelection() {
    const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
    items.forEach((item, i) => {
      item.classList.toggle("selected", i === selectedIndex);
    });
    const selected = items[selectedIndex];
    if (selected) {
      selected.scrollIntoView({ block: "nearest" });
    }
  }

  function addSelectedTag() {
    const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
    const selected = items[selectedIndex];
    if (!selected) return;
    const tag = selected.dataset.tag;
    if (!tag) return;
    if (!state.tagNames.includes(tag)) {
      state.tagNames.push(tag);
    }
    tagSearch.value = "";
    tagDropdown.classList.remove("open");
    tagDropdown.innerHTML = "";
    selectedIndex = -1;
    renderChips();
  }

  function renderChips() {
    const chipsContainer = container.querySelector("#cw-tag-chips");
    if (!chipsContainer) return;
    chipsContainer.innerHTML = state.tagNames
      .map(
        (t) =>
          `<span class="tag-chip" data-cw-tag="${t}">${t} <i class="fas fa-times tag-chip-x"></i></span>`,
      )
      .join("");
  }

  if (tagSearch && tagDropdown) {
    tagSearch.addEventListener(
      "input",
      () => {
        clearTimeout(timer);
        selectedIndex = -1;
        const q = tagSearch.value.trim();
        if (!q) {
          tagDropdown.classList.remove("open");
          tagDropdown.innerHTML = "";
          return;
        }
        timer = setTimeout(async () => {
          try {
            const resp = await fetchJSON(`/api/tags?search=${encodeURIComponent(q)}`);
            const tags = resp.data || [];
            if (tags.length === 0) {
              tagDropdown.innerHTML = `<div class="tag-dropdown-empty">No tags found</div>`;
              selectedIndex = -1;
            } else {
              tagDropdown.innerHTML = tags
                .map(
                  (t, i) =>
                    `<div class="tag-dropdown-item${i === 0 ? " selected" : ""}" data-tag="${t.name}">
                    <span class="tag-dropdown-name">${t.name}</span>
                    ${t.category ? `<span class="tag-dropdown-cat">${t.category}</span>` : ""}
                  </div>`,
                )
                .join("");
              selectedIndex = 0;
            }
            tagDropdown.classList.add("open");
          } catch {
            // ignore
          }
        }, 150);
      },
      { signal },
    );

    // Click on dropdown item → add tag chip
    tagDropdown.addEventListener(
      "click",
      (e) => {
        const item = e.target.closest(".tag-dropdown-item");
        if (!item) return;
        const tag = item.dataset.tag;
        if (!tag) return;
        if (!state.tagNames.includes(tag)) {
          state.tagNames.push(tag);
        }
        tagSearch.value = "";
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        selectedIndex = -1;
        renderChips();
      },
      { signal },
    );

    // Keyboard navigation
    tagSearch.addEventListener(
      "keydown",
      (e) => {
        if (!tagDropdown.classList.contains("open")) return;
        const items = tagDropdown.querySelectorAll(".tag-dropdown-item");
        switch (e.key) {
          case "ArrowDown":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.min(selectedIndex + 1, items.length - 1);
            updateSelection();
            break;
          case "ArrowUp":
            e.preventDefault();
            if (items.length === 0) return;
            selectedIndex = Math.max(selectedIndex - 1, 0);
            updateSelection();
            break;
          case "Enter":
            e.preventDefault();
            addSelectedTag();
            break;
          case "Escape":
            tagDropdown.classList.remove("open");
            tagDropdown.innerHTML = "";
            selectedIndex = -1;
            tagSearch.blur();
            break;
        }
      },
      { signal },
    );
  }

  // ── Tag chip removal (delegated) ──
  const chipsContainer = container.querySelector("#cw-tag-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener(
      "click",
      (e) => {
        const x = e.target.closest(".tag-chip-x");
        if (!x) return;
        const chip = x.closest(".tag-chip");
        if (!chip) return;
        const tag = chip.dataset.cwTag;
        state.tagNames = state.tagNames.filter((t) => t !== tag);
        chip.remove();
      },
      { signal },
    );
  }

  // ── Close dropdown on outside click ──
  document.addEventListener(
    "click",
    (e) => {
      const wrap = container.querySelector(".cw-tag-search");
      if (!wrap || wrap.contains(e.target)) return;
      if (tagDropdown) {
        tagDropdown.classList.remove("open");
        tagDropdown.innerHTML = "";
        selectedIndex = -1;
      }
    },
    { signal },
  );

  // ── Execute button ──
  const execBtn = container.querySelector("#cw-execute");
  if (execBtn) {
    execBtn.addEventListener(
      "click",
      () => {
        if (typeof onExecute === "function") {
          onExecute(state.linkedOnly, state.tagNames, state.nonDefaultOnly);
        }
      },
      { signal },
    );
  }

  return state;
}
