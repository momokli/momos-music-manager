## Plan: fix-comment-diff-display

**Status**: proposed
**Branch**: `fix/comment-diff-display`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Fix two bugs with the Files page comment diff column:

**Bug 1**: `✓null` shown for unchanged comments. When `comment` is null and target is also empty, `escapeHtml(null)` produces `"null"` in the HTML. Should show empty or `(empty)`.

**Bug 2**: When filtering for "Needs Update", some rows show unchanged comments (✓). The `renderCommentDiff` function uses `f.commentUnchanged` (client-computed) instead of `f.needsUpdate` (server-computed from `comment_needs_update`). These can disagree.

### Root cause

In `computeDiff`: `oldStr = oldComment || ""` handles null. But `renderCommentDiff` passes `f.comment` to `escapeHtml` which on null → `"null"`.

In `renderCommentDiff`: decision to show diff vs unchanged uses `f.commentUnchanged` from `computeDiff`. The server's `comment_needs_update` is also available as `f.needsUpdate` but is not used for rendering. When they disagree, the visual and the filter are mismatched.

### Fix

**`frontend/pages/files.js`** — `renderCommentDiff`:

- Use `f.needsUpdate` (server value) to decide whether to show diff or unchanged
- When unchanged and no comment, show `(empty)` instead of `null`
- When diff view, show `(empty)` for empty old/new values

```javascript
function renderCommentDiff(f) {
  if (f.needsUpdate) {
    return `<div class="diff-line">
      <div class="diff-line-old"><span class="diff-sign minus">−</span>${escapeHtml(f.diffOld || "(empty)")}</div>
      <div class="diff-line-new"><span class="diff-sign plus">+</span>${escapeHtml(f.diffNew)}</div>
    </div>`;
  }
  return `<div class="diff-line-unchanged"><span class="diff-sign check">✓</span>${f.comment ? escapeHtml(f.comment) : '<span class="text-muted">(empty)</span>'}</div>`;
}
```

### Acceptance Criteria

- [ ] "Needs Update" filter shows ONLY files with actual comment changes
- [ ] No `✓null` display — empty comments show `(empty)` instead
- [ ] Diff view shows `(empty)` for empty old/new lines
- [ ] No regressions: "Up to Date" filter still works
- [ ] `cargo build` passes (frontend only change)

---

