# Plan: Tag Roundtrip Inbox

**Status**: implemented
**Branch**: `feat/tag-roundtrip-inbox`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Eine "Tag Inbox" für den Comment-Roundtrip: Sie listet alle Files, deren
**gespeicherter Kommentar** (`files.comment`) NICHT dem **generierten
Ziel-Kommentar** entspricht — also Files, die (neu) kommentiert / re-getaggt
werden müssen.

Kernidee ("Roundtrip"): Vergleich über die **geparste Struktur**
(`parse(comment) → generate(parsed) → vergleichen`), nie über den rohen
String. Dadurch erzeugen Formatierungs-Unterschiede (Tag-Reihenfolge,
Quoting, Groß-/Kleinschreibung) KEINE False-Positives — nur echte
Inhalts-Änderungen landen in der Inbox.

### Scope-Entscheidung (im PR dokumentiert)

Der ursprüngliche Entwurf (bidirektionales Disk↔DB-Staging mit
accept/reject/rename/merge-Workflow) wurde bewusst **verworfen**: Die Aufgabe
definiert den Round-Trip-Diff als Kern ("Bei unklarem Scope den
Round-Trip-Diff als Kern entscheiden und im PR klar dokumentieren"). Die
gelieferte Inbox ist daher fokussiert:

- **Liste + Count** statt Staging-Workflow. Keine `tag_inbox`-Tabelle, keine
  Migration, keine Disk-Read-Back-Änderungen am Scan- oder Write-Pfad.
- Wiederverwendung der **bestehenden needs-comment-Logik**
  (`compute_target_comment` / `compute_target_comments_batch`), deren roher
  String-Vergleich (`!=`) durch den Round-Trip-Diff ersetzt wird.

### Kontext

- `src/comment.rs` hat bereits `parse_comment`/`generate_comment` und
  round-trip-getestete Serialisierung des Formats `[PMV] tags sp:xxx sc:xxx`
  (multi-word Tags gequotet).
- `files`-Tabelle hat `file_hash` + `comment`-Spalten (Migration 001).
- needs-comment-Endpoints existieren: `POST /api/files/needs-comment-count`,
  `POST /api/files/needs-comment-count-all`, `POST /api/tracks/needs-comment-count`
  — Vergleich generierter vs. gespeicherter Kommentar.
- Doku: `docs/COMMENT_SYSTEM.md` (Target-Comment-Computation),
  `plans/done/files-bulk-comments.md`, `plans/done/tracks-bulk-comments.md`.

### Design

#### 1. Round-Trip-Diff (pure Logik, `src/comment.rs`)

```rust
pub struct CommentDiff {
    pub tags_added: Vec<String>,        // im Ziel, nicht im gespeicherten
    pub tags_removed: Vec<String>,      // im gespeicherten, nicht im Ziel
    pub phase_changed: Option<(char, char)>,  // (db, disk)
    pub mood_changed: Option<(char, char)>,
    pub vibe_changed: Option<(char, char)>,
    pub source_ids_added: Vec<String>,
    pub source_ids_removed: Vec<String>,
    pub raw_comment_changed: bool,      // unparsebar & raw != db
}

pub fn diff_comments(db: &ParsedComment, disk: &ParsedComment) -> CommentDiff;
pub fn diff_comment_strings(db: Option<&str>, disk: Option<&str>) -> CommentDiff;
pub fn comment_fingerprint(comment: &str) -> String;   // sha256(kanonische Form)
```

- Diff arbeitet case-insensitiv auf Tags (Parser lowercased bereits), PMV
  zeichenweise, Source-IDs exakt.
- Unit-getestet in `src/comment.rs` (added/removed, PMV, source_ids,
  multi-word quoted, unparsebar, Fingerprint-Stabilität/kanonisch).

#### 2. Inbox-Queries (`src/db/inbox.rs`)

- `get_inbox_files(pool, limit, offset) -> Vec<InboxFileItem>` — alle Files
  laden, Ziel-Kommentare **batched** berechnen (`compute_target_comments_batch`),
  pro File `diff_comment_strings(stored, target)`; Files mit leerem Diff
  werden verworfen; Pagination **nach** dem Filter (stabile Seiten).
- `get_inbox_count(pool) -> i64` — Anzahl Files mit nicht-leerem Diff.
- Files ohne resolved tags: Ziel = `[___]` + Source-IDs (aus der File-Zeile),
  konsistent zur Einzel-Berechnung.

#### 3. API (`src/api/inbox.rs`)

| Endpoint | Methode | Bedeutung |
|---|---|---|
| `/api/inbox` | GET | Liste (`{files:[...], total}`), Query `limit` (default 100, max 1000) + `offset` |
| `/api/inbox/count` | GET | Anzahl pending (Badge) |

Response-Item: `fileId`, `filePath`, `title`, `artist`, `comment` (stored),
`targetComment` (generiert), `diff` (strukturierter `CommentDiff`, camelCase).

#### 4. Frontend (`frontend/pages/inbox.js`)

- SPA-Seite `#inbox`, registriert in `frontend/app.js` PAGE_MAP +
  `frontend/shared/nav.js` (Tools → "Tag Inbox", `fa-inbox`).
- Tabelle: File (Link zu `#file-detail?id=`), Stored Comment, Target Comment
  (grün), Diff-Chips (added + / removed − / PMV / raw), "Write"-Button
  (queued `POST /api/files/{id}/write-comment`, bestehender Task).
- Pagination (`?page=N`), Refresh-Button, Badge mit pending-Count.
- CSS in `frontend/style.css` (`.inbox-*` Klassen).

### Tests

- Unit-Tests `src/comment.rs`: `diff_comments`, `diff_comment_strings`,
  `comment_fingerprint` (stabil, kanonisch, unparsebar, roundtrip-equivalent).
- Integration-Tests `tests/api_inbox.rs` (9 Tests, frisches In-Memory-DB +
  alle Migrationen + Axum-Router):
  - Roundtrip: identischer Inhalt mit anderer Formatierung (Uppercase,
    doppelte Spaces) → NICHT in Inbox.
  - Echte Deltas (fehlende Source-ID, NULL-Comment, Extra-Tag + falsches PMV)
    → in Inbox, Diff-Details korrekt.
  - Pagination stabil, Count konsistent, leere DB → leere Inbox.
  - API: `/api/inbox` (Liste + total + camelCase-Diff),
    `/api/inbox/count`, Pagination-Params.

### Build-Fix (im PR dokumentiert)

`origin/main` baut derzeit **nicht**: Die Dependency `youtube = "0.1.1"`
existiert nicht mehr auf crates.io ("no matching package named `youtube`
found"). Sie ist im Code unbenutzt und wurde aus `Cargo.toml`/`Cargo.lock`
entfernt (inkl. transitiver `prost`/`urlencoding`/`prost-derive`). Ohne diesen
Fix ist `cargo build`/`cargo test` auf main nicht möglich.

### Files to modify

| File | Change |
|------|--------|
| `Cargo.toml` / `Cargo.lock` | Unbenutzte `youtube`-Dependency entfernt (Build-Fix) |
| `src/comment.rs` | `CommentDiff`, `diff_comments`, `diff_comment_strings`, `comment_fingerprint` + Unit-Tests |
| `src/db/inbox.rs` | Neu: `get_inbox_files`, `get_inbox_count` (Roundtrip-Diff über needs-comment-Targets) |
| `src/db/mod.rs` | `pub mod inbox;` + Re-Export |
| `src/api/inbox.rs` | Neu: `GET /api/inbox`, `GET /api/inbox/count` |
| `src/api/mod.rs` | `pub mod inbox;` + Router-Merge |
| `frontend/pages/inbox.js` | Neu: Inbox-UI |
| `frontend/app.js`, `frontend/shared/nav.js` | Page-Mapping + Nav-Eintrag |
| `frontend/style.css` | Inbox-CSS |
| `tests/api_inbox.rs` | Neu: Integrationstests (Roundtrip-Diff + API) |

### Out of Scope (deferred)

- Disk↔DB-Staging mit accept/reject/rename/merge/associate (ursprünglicher
  Grobentwurf — bewusst verworfen, siehe Scope-Entscheidung).
- `tag_inbox`-Tabelle, `files.comment_fingerprint`-Spalte, Scan-Read-Back,
  Write-Path-Härtung.
- Auto-Apply von Inbox-Einträgen (bewusst: Liste + Count, User entscheidet
  über den bestehenden Write-/Sync-Button).

### Acceptance Criteria

- [x] `cargo build` passes (0 neue Warnings vs. origin/main; baseline 23 lib
  warnings identisch)
- [x] `cargo test` passes (Unit- + Integrationstests inklusive; einzige
  Failures: 2 vorbestehende metaflac-abhängige Tests, metaflac fehlt in der
  Sandbox-Umgebung — auch auf origin/main rot)
- [x] Formatierungs-Unterschiede erzeugen keine Inbox-Einträge (Roundtrip)
- [x] Echte Comment-Deltas erzeugen Inbox-Einträge mit strukturiertem Diff
- [x] Inbox-UI: Liste, Diff-Chips, Write-Button, Pagination, Badge
