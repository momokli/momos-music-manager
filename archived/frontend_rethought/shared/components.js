/**
 * components.js — Reusable UI building blocks.
 * Stateless render helpers returning HTML strings.
 */

function escapeHtml(str) {
  if (typeof str !== "string") return str;
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

export function renderLoading(label = "Loading...") {
  return `<div class="loading"><div class="spinner"></div><p>${escapeHtml(label)}</p></div>`;
}

export function renderEmpty({ icon = "inbox", title, message, actionHtml = "" }) {
  return `<div class="empty-state">
    <div class="empty-icon"><i class="fas fa-${icon}"></i></div>
    <h3>${escapeHtml(title)}</h3>
    <p>${message}</p>
    ${actionHtml}
  </div>`;
}

export function renderErrorBlock({ title, detail, retryFn }) {
  return `<div class="error-block">
    <div class="error-icon"><i class="fas fa-exclamation-triangle"></i></div>
    <h3>${escapeHtml(title)}</h3>
    <p>${escapeHtml(detail)}</p>
    <button onclick="${escapeHtml(retryFn)}" class="btn btn-primary"><i class="fas fa-redo"></i> Try Again</button>
  </div>`;
}

export function useErrorBanner(el) {
  let timer = null;
  function showError(message) {
    el.textContent = message;
    el.style.display = "block";
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      el.style.display = "none";
    }, 5000);
  }
  function hideError() {
    el.style.display = "none";
    if (timer) clearTimeout(timer);
  }
  return { showError, hideError };
}

export function renderBadge(text, color, opts = {}) {
  const tc = opts.textColor || "#fff";
  return `<span class="badge" style="background:${color};color:${tc}">${escapeHtml(text)}</span>`;
}

export function renderTable(headers, rowsHtml) {
  const thead = headers.map((h) => `<th>${escapeHtml(h)}</th>`).join("");
  return `<div class="table-wrap"><table class="data-table"><thead><tr>${thead}</tr></thead><tbody>${rowsHtml}</tbody></table></div>`;
}

export function td(content, opts = {}) {
  const style = opts.style || "";
  return `<td${style ? ` style="${style}"` : ""}>${content}</td>`;
}

/**
 * Initialize a search bar with consistent behavior across pages.
 * Wires up ENTER key, search button click, and global keyboard shortcuts.
 *
 * Global shortcuts:
 * - Cmd/Ctrl+F focuses the search input
 * - Escape on focused search clears the search and triggers it
 *
 * @param {object} opts
 * @param {string} [opts.inputId] - ID of search input (default "search-input")
 * @param {string} [opts.btnId] - ID of search button (default "search-btn")
 * @param {function} opts.onSearch - Called with (term) when search is triggered
 * @returns {object} { getTerm, setTerm, focus, clear, trigger }
 */
export function initSearchBar(opts) {
  const input = document.getElementById(opts.inputId || "search-input");
  const btn = document.getElementById(opts.btnId || "search-btn");

  function getTerm() {
    return (input?.value || "").trim();
  }

  function trigger() {
    opts.onSearch(getTerm());
  }

  function clear() {
    if (input) input.value = "";
    trigger();
  }

  // Enter key on input
  if (input) {
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        trigger();
      }
    });
  }

  // Search button click
  if (btn) {
    btn.addEventListener("click", trigger);
  }

  // Global keyboard shortcuts
  document.addEventListener("keydown", (e) => {
    // Cmd/Ctrl+F -> focus search
    if ((e.ctrlKey || e.metaKey) && e.key === "f") {
      e.preventDefault();
      input?.focus();
      return;
    }
    // Escape on search input -> clear + trigger
    if (e.key === "Escape" && document.activeElement === input && input) {
      input.value = "";
      trigger();
    }
  });

  return {
    getTerm,
    setTerm: (v) => {
      if (input) input.value = v;
    },
    focus: () => input?.focus(),
    clear,
    trigger,
  };
}

export class Pagination {
  constructor(opts) {
    this.itemsPerPage = opts.itemsPerPage || 50;
    this.page = opts.initialPage ?? 0;
    this.totalItems = 0;
    this._isLoading = false;
    this._onPageChange = opts.onPageChange;
    this._showFirstLast = opts.showFirstLast || false;
    this._bindings = {};
    const b = opts.bindings || {};
    for (const key of ["prev", "next", "info", "first", "last", "total", "showing"]) {
      if (b[key]) this._bindings[key] = document.getElementById(b[key]);
    }
    this._onClick = (dir) => {
      if (this._isLoading) return;
      if (dir === "prev" && this.hasPrev()) this.goTo(this.page - 1);
      if (dir === "next" && this.hasNext()) this.goTo(this.page + 1);
      if (dir === "first" && this.hasPrev()) this.goTo(0);
      if (dir === "last" && this.hasNext()) this.goTo(this.totalPages - 1);
    };
    const bind = (key, dir) => {
      const el = this._bindings[key];
      if (el) el.addEventListener("click", () => this._onClick(dir));
    };
    bind("prev", "prev");
    bind("next", "next");
    bind("first", "first");
    bind("last", "last");
  }
  get totalPages() {
    return Math.ceil(this.totalItems / this.itemsPerPage) || 1;
  }
  get offset() {
    return this.page * this.itemsPerPage;
  }
  hasPrev() {
    return this.page > 0 && this.totalItems > 0;
  }
  hasNext() {
    return this.page < this.totalPages - 1 && this.totalItems > 0;
  }
  update(totalItems, returnedCount) {
    this.totalItems = totalItems;
    this._refreshUI(returnedCount || this.itemsPerPage);
  }
  setLoading(v) {
    this._isLoading = v;
    this._refreshUI();
  }
  goTo(page) {
    if (page < 0 || page >= this.totalPages) return;
    this.page = page;
    this._onPageChange(this.page, this.offset);
    this._refreshUI();
  }
  reset() {
    this.goTo(0);
  }
  _refreshUI(returnedCount) {
    const { prev, next, info, first, last, total: totalEl, showing } = this._bindings;
    const tp = this.totalPages;
    this._setDisabled(prev, !this.hasPrev());
    this._setDisabled(next, !this.hasNext());
    if (first) this._setDisabled(first, !this.hasPrev());
    if (last) this._setDisabled(last, !this.hasNext());
    if (info) info.textContent = `Page ${this.page + 1} of ${tp}`;
    if (totalEl) totalEl.textContent = `Total: ${this.totalItems.toLocaleString()}`;
    if (showing && returnedCount != null) {
      const start = this.totalItems > 0 ? this.offset + 1 : 0;
      const end = Math.min(this.offset + returnedCount, this.totalItems);
      showing.textContent = `Showing: ${start}-${end}`;
    }
  }
  _setDisabled(el, disabled) {
    if (!el) return;
    el.disabled = disabled || this._isLoading;
    el.style.opacity = el.disabled ? "0.5" : "1";
    el.style.cursor = el.disabled ? "not-allowed" : "pointer";
  }
}
