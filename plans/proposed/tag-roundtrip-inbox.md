# Plan: Tag Roundtrip Inbox (komplettes Feature-Set)

**Status**: foundation implemented + full feature set specced (this doc)
**Branch**: `feat/tag-roundtrip-inbox`
**Ready for review**: foundation yes; rename/merge/suggestions pending implementation
**Depends on**: nothing
**Migration needed**: ja (Tag-Staging/Mapping, siehe unten)

---

## Ziel (komplettes Feature-Set, Momo 2026-08-25)

Die Inbox ist der Ort, an dem ein **neuer Tag** (der noch nicht kanonisch im System
existiert) **bearbeitet** wird, bevor er geschrieben wird. Konkret:

1. **Roundtrip-Diff** (bereits implementiert): Inbox listet alle Files, deren
   gespeicherter Kommentar NICHT dem generierten Ziel-Kommentar entspricht
   (Struktur-Vergleich, keine False-Positives durch Formatierung).
2. **Rename**: Ein neuer Tag kann in der Inbox **umbenannt** werden (Typo-Fix),
   assoziiert mit dem Track (oder mehreren, wenn der Tag neu ist).
3. **Similar-Tag-Suggestions**: Für jeden neuen Tag werden **ähnliche, bereits
   existierende Tags** aus der DB vorgeschlagen (Fuzzy-Match), damit der User
   erkennt: „ich hab mich verschrieben, ich meinte eigentlich den".
4. **Click-to-Merge**: Klick auf einen ähnlichen (bestehenden) Tag → der neue/Typo-Tag
   wird **in den bestehenden gemergt**. ALLE Tracks, die den Typo-Tag tragen, werden
   automatisch auf den kanonischen Tag umgetaggt. Der exakte Typo-Wortlaut
   **verschwindet** aus dem System (taucht nur noch 1× in der Inbox auf).
5. **Staging-Semantik**: Bevor die Inbox bearbeitet ist, ist der neue Tag **noch nicht
   im System** übernommen. Beim nächsten Schreiben wird der **kanonische** (umgemappte)
   Tag geschrieben.
6. **Manuell, kein Over-Automation**: kein Auto-Apply, keine zirkuläre Logik, kein
   scrappy Workaround. Der User entscheidet per Klick.

### Beispiel (Momo)

Tag „peak" getippt als „peek" / „peeq" / „aufbau" — Inbox schlägt den bestehenden Tag
„peak" als ähnlich vor → User klickt ihn an → „peek" wird überall durch „peak" ersetzt.

---

## Bereits implementiert (Foundation, PR #1 bisher)

### Round-Trip-Diff (`src/comment.rs`)

```rust
pub struct CommentDiff {
    pub tags_added: Vec<String>,
    pub tags_removed: Vec<String>,
    pub phase_changed: Option<(char, char)>,
    pub mood_changed: Option<(char, char)>,
    pub vibe_changed: Option<(char, char)>,
    pub source_ids_added: Vec<String>,
    pub source_ids_removed: Vec<String>,
    pub raw_comment_changed: bool,
}
pub fn diff_comments(db: &ParsedComment, disk: &ParsedComment) -> CommentDiff;
pub fn diff_comment_strings(db: Option<&str>, disk: Option<&str>) -> CommentDiff;
pub fn comment_fingerprint(comment: &str) -> String;
```

### Inbox-Queries (`src/db/inbox.rs`)

- `get_inbox_files(pool, limit, offset) -> Vec<InboxFileItem>`
- `get_inbox_count(pool) -> i64`

### API (`src/api/inbox.rs`)

- `GET /api/inbox` (`{files:[...], total}`, `limit`/`offset`)
- `GET /api/inbox/count`

### Frontend (`frontend/pages/inbox.js`)

- SPA-Seite `#inbox`, Nav-Eintrag, Tabelle mit Diff-Chips, Write-Button, Pagination.

---

## Neu zu implementieren (Full Feature Set)

### A. Tag-Vokabular / Similar-Matching

- **Fuzzy-Default**: case-insensitive + Levenshtein ≤ 2 gegen bestehende Tag-Namen
  (Vorarbeit existiert: Migration `004_unique_tags_nocase.sql`, Branch
  `fix/tag-case-duplicates`).
- Quelle der „bestehenden Tags": alle bereits in `files.comment` (resolved tags)
  vorkommenden Tags, idealerweise normalisiert (lowercase).
- Neues Endpoint oder Erweiterung von `/api/inbox`: pro Inbox-Item die neuen Tags
  (`diff.tags_added`) mit `suggestions: [{tag, distance, count}]` liefern.

### B. Rename / Merge-Workflow (Staging)

- Neue Tabelle (Migration `023_tag_inbox.sql`) für Inbox-Bearbeitung, z.B.:
  `tag_inbox (id, file_id, raw_tag, action, target_tag, status, created_at)`.
- **Rename**: `raw_tag` → neuer Wortlaut (Typo-Fix), assoziiert mit Track(s).
- **Merge**: `raw_tag` → `target_tag` (bestehender Tag). Beim Commit werden alle
  Tracks mit `raw_tag` auf `target_tag` umgetaggt; `raw_tag` verschwindet.
- **Staging-Semantik**: Vor dem Commit ist der Mapping nur in der Inbox, nicht in den
  geschriebenen Kommentaren. Beim nächsten Write (`write-comment` / Sync) wird der
  kanonische Tag geschrieben.
- API: `POST /api/inbox/rename`, `POST /api/inbox/merge`, `POST /api/inbox/dismiss`
  (oder einheitlich `POST /api/inbox/resolve` mit `action`).

### C. Frontend-Erweiterung

- Inbox-UI: pro neuem Tag ein Inline-Edit (Rename) + Suggestions-Chips (klickbare
  ähnliche Tags → Merge) + Dismiss-Button.
- Merge/Rename wirkt sofort auf die Inbox-Ansicht (Refresh des Diffs).

### D. Write-Path-Härtung

- `write-comment` / Sync-Pfad respektiert offene Inbox-Mappings: ein gemappter Typo-Tag
  wird beim Schreiben durch den kanonischen Tag ersetzt. Kein Auto-Apply — nur wenn der
  User die Inbox bearbeitet hat.

---

## Acceptance Criteria (Full Feature Set)

- [ ] Roundtrip-Diff + Inbox-Liste + Count (Foundation, bereits grün)
- [ ] `/api/inbox` liefert pro neuem Tag Similar-Suggestions (Levenshtein ≤ 2,
      case-insensitive)
- [ ] Rename eines neuen Tags in der Inbox persistiert den Mapping
- [ ] Merge eines Typo-Tags in einen bestehenden Tag taggt ALLE betroffenen Tracks um
- [ ] Typo-Wortlaut verschwindet nach Merge aus dem System (nur Inbox-Historie)
- [ ] Vor Inbox-Bearbeitung: Tag nicht im System committed (Staging)
- [ ] Kein Auto-Apply, kein zirkulärer Pfad
- [ ] `cargo build` + `cargo test` grün (abzgl. vorbestehender metaflac-Tests)

---

## Build-Fix (bereits im PR #1)

`origin/main` baut nicht: unbenutzte `youtube = "0.1.1"`-Dependency existiert nicht mehr
auf crates.io. Im PR #1 aus `Cargo.toml`/`Cargo.lock` entfernt.
