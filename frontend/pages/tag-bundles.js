/**
 * tag-bundles.js — Tag Bundles management page.
 *
 * A "bundle tag" aggregates multiple member tags into one.
 * Files with any member tag also get the bundle tag.
 *
 * Layout:
 *   Left panel: searchable list of bundle tags with member count
 *   Right panel: selected bundle detail (member chips, add/remove, file preview)
 */

import {
  escapeHtml,
  renderLoading,
  renderErrorBlock,
  renderEmpty,
  showToast,
  showModal,
} from "../shared/components.js";
import { fetchJSON } from "../shared/api.js";

/* ------------------------------------------------------------------ */
/*  Category Info                                                     */
/* ------------------------------------------------------------------ */

const CATEGORY_INFO = {
  Phase: { color: "var(--purple)", icon: "fa-activity", prefix: "P" },
  Mood: { color: "var(--pink)", icon: "fa-heart", prefix: "M" },
  Vibe: { color: "var(--yellow)", icon: "fa-sparkles", prefix: "V" },
  Merkmal: { color: "var(--green)", icon: "fa-hash", prefix: "E" },
  Setlist: { color: "var(--text-muted)", icon: "fa-list-music", prefix: "S" },
};

/* ------------------------------------------------------------------ */
/*  State                                                             */
/* ------------------------------------------------------------------ */

const state = {
  bundles: [],
  selectedBundleId: null,
  selectedBundle: null,
  members: [],
  search: "",
  memberSearch: "",
  typeaheadResults: [],
  typeaheadOpen: false,
  typeaheadIndex: -1,
  loading: false,
  saving: false,
};

/* ------------------------------------------------------------------ */
/*  Page Init                                                         */
/* ------------------------------------------------------------------ */

/**
 * Page init — called by the SPA router on #tag-bundles.
 * @param {HTMLElement} container
 * @param {AbortSignal} signal
 */
export async function init(container, signal, hashParams) {
  container.innerHTML = renderLoading("Loading tag bundles…");
  if (signal.aborted) return;

  try {
    const resp = await fetchJSON("/api/tags/bundles?limit=200", { signal });
    if (signal.aborted) return;
    state.bundles = resp.data || [];
    state.search = "";

    renderFullPage(container);
    wireEvents(container, signal);
  } catch (err) {
    if (err.name === "AbortError") return;
    container.innerHTML = renderErrorBlock({
      title: "Failed to load tag bundles",
      detail: err.message || "Unknown error",
      retryFn: "window.location.hash='#tag-bundles'",
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Render Functions                                                  */
/* ------------------------------------------------------------------ */

function renderFullPage(container) {
  const hasBundle = state.selectedBundleId != null;
  container.innerHTML = `
    <div class="page-header-row">
      <h1><i class="fas fa-layer-group"></i> Tag Bundles</h1>
      <button class="btn btn-sm btn-primary" id="tb-new-bundle-btn">
        <i class="fas fa-plus"></i> New Tag
      </button>
    </div>
    <div class="tag-bundles-layout">
      <div class="tag-bundles-list" id="tb-list-panel">
        <div style="margin-bottom:var(--space-3);">
          <div class="tag-search-wrap" style="position:relative;">
            <i class="fas fa-search" style="position:absolute;left:10px;top:50%;transform:translateY(-50%);color:var(--text-muted);font-size:0.8rem;z-index:1;"></i>
            <input type="text" class="input-text input-search" id="tb-search" placeholder="Filter bundles…" autocomplete="off">
          </div>
        </div>
        <div id="tb-bundle-list">
          ${renderBundleList(state.bundles, state.selectedBundleId)}
        </div>
      </div>
      <div class="tag-bundles-detail" id="tb-detail-panel">
        ${
          hasBundle
            ? renderBundleDetail(state.selectedBundle, state.members)
            : renderEmpty({
                icon: "layer-group",
                title: "Select a bundle",
                message: "Choose a bundle tag from the left panel to manage its members.",
              })
        }
      </div>
    </div>
  `;
}

function renderBundleList(bundles, selectedId) {
  if (bundles.length === 0) {
    return `<div style="padding:1rem 0;color:var(--text-muted);font-size:0.85rem;">No bundle tags yet. Create one to get started.</div>`;
  }

  const filtered = state.search
    ? bundles.filter((b) => b.name.toLowerCase().includes(state.search.toLowerCase()))
    : bundles;

  return filtered
    .map((b) => {
      const active = b.id === selectedId ? " active" : "";
      const catInfo = CATEGORY_INFO[b.categoryName] || CATEGORY_INFO.Setlist;
      return `
        <div class="bundle-list-item${active}" data-bundle-id="${b.id}">
          <div style="display:flex;align-items:center;gap:0.5rem;min-width:0;flex:1;">
            <i class="${catInfo.icon}" style="color:${catInfo.color};font-size:0.8rem;flex-shrink:0;"></i>
            <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:500;">${escapeHtml(b.name)}</span>
          </div>
          <span class="member-count">${b.memberCount ?? 0}</span>
        </div>
      `;
    })
    .join("");
}

function renderBundleDetail(bundle, members) {
  if (!bundle) return "";
  const catInfo = CATEGORY_INFO[bundle.categoryName] || CATEGORY_INFO.Setlist;

  const membersHtml =
    members.length > 0
      ? `<div style="display:flex;flex-wrap:wrap;gap:0.25rem;">${members.map(renderMemberChip).join("")}</div>`
      : '<span style="font-size:0.85rem;color:var(--text-subtle);">No member tags yet. Search and add tags below.</span>';

  return `
    <div style="margin-bottom:var(--space-4);">
      <div style="display:flex;align-items:center;gap:var(--space-3);margin-bottom:var(--space-2);">
        <span style="font-size:1.25rem;font-weight:700;word-break:break-word;">${escapeHtml(bundle.name)}</span>
      </div>
      <div style="display:flex;align-items:center;gap:var(--space-3);font-size:0.8rem;color:var(--text-muted);">
        <span style="color:${catInfo.color};display:inline-flex;align-items:center;gap:4px;">
          <i class="${catInfo.icon}"></i> ${escapeHtml(bundle.categoryName)}
        </span>
        <span><i class="fas fa-users"></i> ${members.length} member${members.length !== 1 ? "s" : ""}</span>
      </div>
    </div>

    <div style="background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-xl);padding:var(--space-5) var(--space-6);margin-bottom:var(--space-4);">
      <div style="display:flex;align-items:center;gap:var(--space-2);margin-bottom:var(--space-3);">
        <i class="fas fa-tags" style="color:var(--text-muted);font-size:0.85rem;"></i>
        <span style="font-weight:600;font-size:0.85rem;color:var(--text-secondary);">Members</span>
      </div>
      <div id="tb-member-chips" style="margin-bottom:var(--space-3);min-height:28px;">
        ${membersHtml}
      </div>
      <div style="display:flex;gap:6px;position:relative;">
        <div class="typeahead-wrap" style="flex:1;position:relative;">
          <input type="text" class="input-text w-full" id="tb-member-search" placeholder="Search tags to add…" autocomplete="off" style="font-size:0.85rem;">
          <div id="tb-member-dropdown" class="typeahead-dropdown" style="display:none;position:absolute;top:100%;left:0;right:0;max-height:220px;overflow-y:auto;background:var(--bg);border:1px solid var(--border);border-top:none;border-radius:0 0 var(--radius-md) var(--radius-md);z-index:100;box-shadow:0 8px 24px rgba(0,0,0,0.3);"></div>
        </div>
        <button class="btn btn-sm btn-primary" id="tb-add-btn" disabled style="white-space:nowrap;"><i class="fas fa-plus"></i> Add</button>
      </div>
    </div>

    <div id="tb-file-preview" style="background:var(--surface);border:1px solid var(--border);border-radius:var(--radius-xl);padding:var(--space-4) var(--space-6);">
      <div style="display:flex;align-items:center;gap:var(--space-2);margin-bottom:var(--space-3);">
        <i class="fas fa-file" style="color:var(--text-muted);font-size:0.85rem;"></i>
        <span style="font-weight:600;font-size:0.85rem;color:var(--text-secondary);">File Preview</span>
        <span style="font-size:0.75rem;color:var(--text-subtle);">(first 10 tracks)</span>
      </div>
      <div id="tb-file-preview-content">
        <div class="loading" style="padding:1rem 0;"><div class="spinner" style="width:20px;height:20px;"></div></div>
      </div>
    </div>
  `;
}

function renderMemberChip(member) {
  const catInfo = CATEGORY_INFO[member.categoryName] || CATEGORY_INFO.Setlist;
  return `
    <span class="tag-chip" data-member-id="${member.id}">
      <span style="color:${catInfo.color};font-size:0.7rem;font-weight:700;margin-right:2px;">${catInfo.prefix}</span>
      ${escapeHtml(member.name)}
      <span class="tag-chip-x" data-member-id="${member.id}" title="Remove member">×</span>
    </span>
  `;
}

function renderTypeaheadDropdown(results, selectedIndex) {
  if (results.length === 0) {
    return `<div style="padding:var(--space-3);color:var(--text-subtle);font-size:0.85rem;text-align:center;">No tags found</div>`;
  }
  return results
    .map((tag, idx) => {
      const catInfo = CATEGORY_INFO[tag.categoryName] || CATEGORY_INFO.Setlist;
      const highlighted = idx === selectedIndex ? " highlighted" : "";
      return `
        <div class="tag-dropdown-item${highlighted}" data-index="${idx}" data-tag-id="${tag.id}" data-tag-name="${escapeHtml(tag.name)}" data-category="${escapeHtml(tag.categoryName || "")}">
          <span style="display:flex;align-items:center;gap:0.5rem;">
            <span class="cat-badge" style="background:${catInfo.color};color:#fff;font-size:0.65rem;padding:0.1rem 0.35rem;border-radius:3px;font-weight:600;">${catInfo.prefix}</span>
            <span>${escapeHtml(tag.name)}</span>
          </span>
          <span style="font-size:0.75rem;color:var(--accent);white-space:nowrap;">→ Add</span>
        </div>
      `;
    })
    .join("");
}

function renderFilePreviewContainer() {
  return `<div class="loading" style="padding:1rem 0;"><div class="spinner" style="width:20px;height:20px;"></div></div>`;
}

/* ------------------------------------------------------------------ */
/*  Wire Events                                                       */
/* ------------------------------------------------------------------ */

function wireEvents(container, signal) {
  // ── Search bundles ──
  const searchInput = container.querySelector("#tb-search");
  if (searchInput) {
    let timer = null;
    searchInput.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        state.search = searchInput.value.trim();
        const list = container.querySelector("#tb-bundle-list");
        if (list) {
          list.innerHTML = renderBundleList(state.bundles, state.selectedBundleId);
        }
      }, 150);
    });
  }

  // ── Bundle list item click (delegated) ──
  const listPanel = container.querySelector("#tb-bundle-list");
  if (listPanel) {
    listPanel.addEventListener("click", (e) => {
      const item = e.target.closest(".bundle-list-item");
      if (!item) return;
      const id = parseInt(item.dataset.bundleId, 10);
      if (id !== state.selectedBundleId) {
        selectBundle(container, id);
      }
    });
  }

  // ── New Tag button ──
  const newBtn = container.querySelector("#tb-new-bundle-btn");
  if (newBtn) {
    newBtn.addEventListener("click", () => {
      showNewTagModal(container);
    });
  }

  // ── Member typeahead ──
  wireTypeahead(container, signal);

  // ── Member chip removal (delegated) ──
  const chipsContainer = container.querySelector("#tb-member-chips");
  if (chipsContainer) {
    chipsContainer.addEventListener("click", async (e) => {
      const xBtn = e.target.closest(".tag-chip-x");
      if (!xBtn) return;
      const memberId = parseInt(xBtn.dataset.memberId, 10);
      if (isNaN(memberId)) return;
      const member = state.members.find((m) => m.id === memberId);
      if (!member) return;
      await removeMember(container, member);
    });
  }

  // ── Keyboard shortcuts for typeahead ──
  const memberSearch = container.querySelector("#tb-member-search");
  if (memberSearch) {
    memberSearch.addEventListener("keydown", (e) => {
      const dropdown = container.querySelector("#tb-member-dropdown");
      if (!dropdown || dropdown.style.display === "none") return;
      const items = dropdown.querySelectorAll(".tag-dropdown-item");

      if (e.key === "ArrowDown") {
        e.preventDefault();
        state.typeaheadIndex = Math.min(state.typeaheadIndex + 1, items.length - 1);
        highlightTypeaheadItem(dropdown, state.typeaheadIndex);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        state.typeaheadIndex = Math.max(state.typeaheadIndex - 1, 0);
        highlightTypeaheadItem(dropdown, state.typeaheadIndex);
      } else if (e.key === "Enter") {
        e.preventDefault();
        const item = items[state.typeaheadIndex];
        if (item) {
          selectTypeaheadItem(container, item);
        }
      } else if (e.key === "Escape") {
        closeTypeahead(container);
        memberSearch.blur();
      }
    });
  }

  if (signal) {
    signal.addEventListener("abort", () => {
      // Cleanup handled by React-style re-init on page change
    });
  }
}

/* ------------------------------------------------------------------ */
/*  Typeahead                                                         */
/* ------------------------------------------------------------------ */

function wireTypeahead(container, signal) {
  const input = container.querySelector("#tb-member-search");
  const dropdown = container.querySelector("#tb-member-dropdown");
  const addBtn = container.querySelector("#tb-add-btn");
  if (!input || !dropdown) return;

  let debounceTimer = null;

  const doSearch = async () => {
    const q = input.value.trim();
    if (q.length < 1) {
      closeTypeahead(container);
      if (addBtn) addBtn.disabled = true;
      return;
    }

    // Fetch tags from the API for typeahead
    try {
      const resp = await fetchJSON(
        `/api/tags?search=${encodeURIComponent(q)}&page_size=20`,
      );
      const results = (resp.data || []).filter(
        (t) =>
          t.id !== state.selectedBundleId && !state.members.some((m) => m.id === t.id),
      );
      state.typeaheadResults = results;
      state.typeaheadIndex = results.length > 0 ? 0 : -1;
      dropdown.innerHTML = renderTypeaheadDropdown(results, state.typeaheadIndex);
      dropdown.style.display = results.length > 0 ? "block" : "none";
      if (addBtn) addBtn.disabled = results.length === 0;
    } catch (err) {
      if (err.name === "AbortError") return;
      // Silently fail — don't disrupt the user
    }
  };

  input.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    if (input.value.trim().length === 0) {
      closeTypeahead(container);
      if (addBtn) addBtn.disabled = true;
      return;
    }
    debounceTimer = setTimeout(doSearch, 200);
  });

  // Click on dropdown item
  dropdown.addEventListener("click", (e) => {
    const item = e.target.closest(".tag-dropdown-item");
    if (item) {
      selectTypeaheadItem(container, item);
    }
  });

  // Close on outside click
  document.addEventListener("click", (e) => {
    if (
      !e.target.closest("#tb-member-search") &&
      !e.target.closest("#tb-member-dropdown")
    ) {
      closeTypeahead(container);
    }
  });

  // Add button click — add the highlighted item
  if (addBtn) {
    addBtn.addEventListener("click", () => {
      const highlighted = dropdown.querySelector(".tag-dropdown-item.highlighted");
      if (highlighted) {
        selectTypeaheadItem(container, highlighted);
      }
    });
  }
}

function highlightTypeaheadItem(dropdown, idx) {
  const items = dropdown.querySelectorAll(".tag-dropdown-item");
  items.forEach((el, i) => {
    el.classList.toggle("highlighted", i === idx);
  });
}

function selectTypeaheadItem(container, item) {
  const id = parseInt(item.dataset.tagId, 10);
  const name = item.dataset.tagName;
  const categoryName = item.dataset.category;
  if (!id) return;

  addMember(container, { id, name, categoryName });
  const input = container.querySelector("#tb-member-search");
  if (input) {
    input.value = "";
  }
  closeTypeahead(container);
}

function closeTypeahead(container) {
  const dropdown = container.querySelector("#tb-member-dropdown");
  const addBtn = container.querySelector("#tb-add-btn");
  if (dropdown) dropdown.style.display = "none";
  if (addBtn) addBtn.disabled = true;
  state.typeaheadResults = [];
  state.typeaheadIndex = -1;
  state.typeaheadOpen = false;
}

/* ------------------------------------------------------------------ */
/*  Data Operations                                                   */
/* ------------------------------------------------------------------ */

async function selectBundle(container, bundleId) {
  state.selectedBundleId = bundleId;
  state.selectedBundle = state.bundles.find((b) => b.id === bundleId) || null;
  state.members = [];
  state.saving = false;

  // Update list active highlight
  const list = container.querySelector("#tb-bundle-list");
  if (list) {
    list.querySelectorAll(".bundle-list-item").forEach((el) => {
      el.classList.toggle("active", parseInt(el.dataset.bundleId, 10) === bundleId);
    });
  }

  // Show detail with loading
  const detail = container.querySelector("#tb-detail-panel");
  if (detail) {
    detail.innerHTML = renderLoading("Loading bundle details…");
  }

  try {
    const resp = await fetchJSON(`/api/tags/${bundleId}/bundle-members`);
    state.members = resp.data || [];

    // Re-render detail
    if (detail) {
      detail.innerHTML = renderBundleDetail(state.selectedBundle, state.members);
      wireTypeahead(container, null);
    }

    // Load file preview
    loadFilePreview(container, bundleId);
  } catch (err) {
    showToast(`Failed to load bundle members: ${err.message}`, "error");
    if (detail) {
      detail.innerHTML = renderErrorBlock({
        title: "Failed to load bundle",
        detail: err.message,
        retryFn: `window.location.hash='#tag-bundles'`,
      });
    }
  }
}

async function addMember(container, tag) {
  if (state.saving) return;
  state.saving = true;
  state.members.push({ id: tag.id, name: tag.name, categoryName: tag.categoryName });

  try {
    await saveMembers(container);
    showToast(`Added "${tag.name}"`, "success");

    // Refresh bundle list (member count changes)
    refreshBundleList(container);

    // Update member chips
    const chipsEl = container.querySelector("#tb-member-chips");
    if (chipsEl) {
      chipsEl.innerHTML = `<div style="display:flex;flex-wrap:wrap;gap:0.25rem;">${state.members.map(renderMemberChip).join("")}</div>`;
    }

    // Update file preview
    loadFilePreview(container, state.selectedBundleId);
  } catch (err) {
    // Rollback
    state.members = state.members.filter((m) => m.id !== tag.id);
    showToast(`Failed to add member: ${err.message}`, "error");
  } finally {
    state.saving = false;
  }
}

async function removeMember(container, member) {
  if (state.saving) return;
  state.saving = true;
  const oldMembers = [...state.members];
  state.members = state.members.filter((m) => m.id !== member.id);

  try {
    await saveMembers(container);
    showToast(`Removed "${member.name}"`, "info");

    // Refresh bundle list
    refreshBundleList(container);

    // Update member chips
    const chipsEl = container.querySelector("#tb-member-chips");
    if (chipsEl) {
      if (state.members.length > 0) {
        chipsEl.innerHTML = `<div style="display:flex;flex-wrap:wrap;gap:0.25rem;">${state.members.map(renderMemberChip).join("")}</div>`;
      } else {
        chipsEl.innerHTML =
          '<span style="font-size:0.85rem;color:var(--text-subtle);">No member tags yet. Search and add tags below.</span>';
      }
    }

    // Update file preview
    loadFilePreview(container, state.selectedBundleId);
  } catch (err) {
    // Rollback
    state.members = oldMembers;
    showToast(`Failed to remove member: ${err.message}`, "error");
  } finally {
    state.saving = false;
  }
}

async function saveMembers(container) {
  if (!state.selectedBundleId) return;
  const memberIds = state.members.map((m) => m.id);
  await fetchJSON(`/api/tags/${state.selectedBundleId}/bundle-members`, {
    method: "PUT",
    body: JSON.stringify({ memberTagIds: memberIds }),
  });
}

async function refreshBundleList(container) {
  try {
    const resp = await fetchJSON("/api/tags/bundles?limit=200");
    state.bundles = resp.data || [];
    const list = container.querySelector("#tb-bundle-list");
    if (list) {
      list.innerHTML = renderBundleList(state.bundles, state.selectedBundleId);
    }
  } catch {
    // Non-critical — list will refresh on next page load
  }
}

async function loadFilePreview(container, bundleId) {
  const previewContent = container.querySelector("#tb-file-preview-content");
  if (!previewContent) return;

  try {
    // Fetch files tagged with this bundle tag (first 10)
    const resp = await fetchJSON(
      `/api/files?tags=${encodeURIComponent(state.selectedBundle?.name || "")}&limit=10`,
    );
    const files = resp.data || [];

    if (files.length === 0) {
      previewContent.innerHTML =
        '<span style="font-size:0.85rem;color:var(--text-subtle);">No files with this bundle tag yet.</span>';
      return;
    }

    const rows = files
      .map((f) => {
        const tags = f.tags || [];
        const tagStr = tags
          .slice(0, 4)
          .map((t) => escapeHtml(t.tagName || t.name))
          .join(", ");
        const more = tags.length > 4 ? ` +${tags.length - 4}` : "";
        return `
          <div style="display:flex;align-items:center;gap:0.75rem;padding:0.3rem 0;border-bottom:1px solid var(--border);font-size:0.85rem;">
            <span style="min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
              <strong>${escapeHtml(f.title)}</strong>
            </span>
            <span style="color:var(--text-muted);flex-shrink:0;">${escapeHtml(f.artist || "—")}</span>
            <span style="color:var(--text-subtle);font-size:0.75rem;flex-shrink:0;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
              ${escapeHtml(tagStr)}${more}
            </span>
          </div>
        `;
      })
      .join("");

    previewContent.innerHTML = rows;
  } catch {
    previewContent.innerHTML =
      '<span style="font-size:0.85rem;color:var(--text-subtle);">Could not load file preview.</span>';
  }
}

/* ------------------------------------------------------------------ */
/*  New Tag Modal                                                     */
/* ------------------------------------------------------------------ */

function showNewTagModal(container) {
  showModal({
    title: "New Bundle Tag",
    bodyHtml: `
      <div style="padding:var(--space-6);">
        <div class="form-group">
          <label>Tag Name</label>
          <input type="text" class="input-text w-full" id="new-bundle-tag-name" placeholder="e.g. afterhour-jonas">
        </div>
        <div style="font-size:0.8rem;color:var(--text-subtle);margin-top:var(--space-2);">
          Created as a Setlist category tag. You can add member tags afterwards.
        </div>
      </div>
      <div class="modal-actions" style="padding:0 var(--space-6) var(--space-6);">
        <button class="btn" data-modal-action="close">Cancel</button>
        <button class="btn btn-primary" data-modal-action="save-tag">Create</button>
      </div>
    `,
    onAction: async (action, close) => {
      if (action !== "save-tag") return;
      const name = document.getElementById("new-bundle-tag-name")?.value.trim();
      if (!name) {
        showToast("Tag name is required", "error");
        return;
      }
      try {
        // Create the tag as Setlist (categoryId=1)
        const resp = await fetchJSON("/api/tags", {
          method: "POST",
          body: JSON.stringify({ name, categoryId: 1 }),
        });
        showToast(`Tag "${name}" created`, "success");
        close();

        // Add the new tag directly to state — it has 0 members so it won't
        // appear in GET /api/tags/bundles (which uses EXISTS on tag_bundles).
        const createdTag = resp.data; // { id, name, category, ... }
        const bundleEntry = {
          id: createdTag.id,
          name: createdTag.name,
          categoryId: 1,
          categoryName: "Setlist",
          memberCount: 0,
          backpack: false,
        };
        state.bundles = [...state.bundles, bundleEntry];
        state.selectedBundleId = createdTag.id;
        state.selectedBundle = bundleEntry;
        state.members = [];
        renderFullPage(container);
        wireEvents(container, null);
        // Auto-navigate to the detail view so the user can start adding members
        await selectBundle(container, createdTag.id);
      } catch (err) {
        showToast(`Failed to create tag: ${err.message}`, "error");
      }
    },
  });
}
