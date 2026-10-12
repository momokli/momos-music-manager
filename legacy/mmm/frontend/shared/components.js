/**
 * components.js — Reusable UI building blocks.
 * Stateless render helpers returning HTML strings.
 */

export function escapeHtml(str) {
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
  const thead = headers
    .map((h) => {
      if (typeof h === "string") return `<th>${escapeHtml(h)}</th>`;
      return `<th${h.style ? ` style="${h.style}"` : ""}>${escapeHtml(h.label)}</th>`;
    })
    .join("");
  return `<div class="table-wrap"><table class="data-table"><thead><tr>${thead}</tr></thead><tbody>${rowsHtml}</tbody></table></div>`;
}

export function td(content, opts = {}) {
  const style = opts.style || "";
  return `<td${style ? ` style="${style}"` : ""}>${content}</td>`;
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

/* ------------------------------------------------------------------ */
/*  Toast notification                                                 */
/* ------------------------------------------------------------------ */

/**
 * Show a toast notification. Auto-dismisses after 4s.
 * @param {string} message - Text to display
 * @param {"success"|"error"|"info"} type
 */
export function showToast(message, type) {
  const existing = document.querySelector(".toast-notification");
  if (existing) existing.remove();

  const bg =
    type === "error"
      ? "var(--red, #ef4444)"
      : type === "success"
        ? "var(--green, #22c55e)"
        : "var(--accent, #6366f1)";

  const toast = document.createElement("div");
  toast.className = "toast-notification";
  toast.textContent = message;
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "24px",
    right: "24px",
    background: bg,
    color: "#fff",
    padding: "12px 20px",
    borderRadius: "8px",
    fontSize: "0.9rem",
    zIndex: "9999",
    boxShadow: "0 4px 20px rgba(0,0,0,0.3)",
    transition: "opacity 0.3s ease",
    cursor: "pointer",
  });
  toast.addEventListener("click", () => toast.remove());
  document.body.appendChild(toast);

  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => toast.remove(), 300);
  }, 4000);
}

/* ------------------------------------------------------------------ */
/*  Modal helpers                                                      */
/* ------------------------------------------------------------------ */

/**
 * Render a modal overlay. Returns cleanup function.
 * @param {object} opts
 * @param {string} opts.title - Modal title (HTML allowed)
 * @param {string} opts.bodyHtml - Modal body content
 * @param {string} [opts.width] - Max-width override (e.g. "600px")
 * @param {Function} [opts.onClose] - Called when modal closes
 * @param {Function} [opts.onAction] - Called when a data-modal-action button is clicked. Receives (action, cleanup).
 * @returns {Function} cleanup — call to remove modal
 */
export function showModal({ title, bodyHtml, width, onClose, onAction }) {
  const modalHtml = `
    <div class="modal open" id="shared-modal">
      <div class="modal-content" style="${width ? `max-width:${width}` : ""}">
        <div class="modal-header">
          <h3>${title}</h3>
          <button class="close-btn" data-modal-action="close">&times;</button>
        </div>
        ${bodyHtml}
      </div>
    </div>
  `;

  const wrapper = document.createElement("div");
  wrapper.innerHTML = modalHtml;
  document.body.appendChild(wrapper.firstElementChild);

  const modal = document.getElementById("shared-modal");

  function cleanup() {
    modal?.classList.remove("open");
    modal?.remove();
    if (onClose) onClose();
  }

  modal?.addEventListener("click", (e) => {
    if (e.target === modal) cleanup();
    const actionBtn = e.target.closest("[data-modal-action]");
    if (actionBtn) {
      const action = actionBtn.dataset.modalAction;
      if (action === "close") {
        cleanup();
      } else if (onAction) {
        onAction(action, cleanup);
      }
    }
  });
  document.addEventListener("keydown", function escHandler(e) {
    if (e.key === "Escape") {
      cleanup();
      document.removeEventListener("keydown", escHandler);
    }
  });

  return cleanup;
}

/**
 * Show a confirm dialog that returns a Promise<boolean>.
 * Resolves true when the user clicks the confirm button, false on cancel/close.
 */
export function showConfirmModal(title, messageHtml, confirmLabel, confirmColor) {
  return new Promise((resolve) => {
    const isRed = confirmColor === "red";
    const btnClass = isRed ? "btn btn-red" : "btn btn-primary";
    const bodyHtml = `
      <div class="modal-body">
        <p>${messageHtml}</p>
      </div>
      <div class="modal-footer">
        <button class="btn" data-modal-action="cancel">Cancel</button>
        <button class="${btnClass}" data-modal-action="confirm">${escapeHtml(confirmLabel || "Confirm")}</button>
      </div>
    `;
    const cleanup = showModal({
      title,
      bodyHtml,
      onAction: (action, close) => {
        resolve(action === "confirm");
        close();
      },
      onClose: () => resolve(false),
    });
  });
}
