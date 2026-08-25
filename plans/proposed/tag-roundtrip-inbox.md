# Plan: Tag Roundtrip Inbox (komplettes Feature-Set)

**Status**: foundation implemented + full feature set implemented (this PR)
**Branch**: `feat/tag-roundtrip-inbox`
**Ready for review**: yes — rename/merge/suggestions implemented, build+tests grün (abzgl. metaflac)
**Depends on**: nothing
**Migration needed**: ja — `023_tag_inbox.sql` (Tag-Staging/Mapping, via `sqlx::migrate!()` automatisch)

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

## Implementiert (Full Feature Set)

### A. Tag-Vokabular / Similar-Matching (implementiert)

- **Fuzzy-Default**: case-insensitive + Levenshtein ≤ 2 gegen bestehende Tag-Namen
  (Vorarbeit existiert: Migration `004_unique_tags_nocase.sql`, Branch
  `fix/tag-case-duplicates`).
- Quelle der „bestehenden Tags“: alle `tags`-Tabellen-Namen; File-Count pro
  Vorschlag aus der materialisierten `file_resolved_tags`.
- Erweiterung von `/api/inbox`: pro Inbox-Item die neuen Tags (`diff.tags_added` ∪
  `diff.tags_removed`) mit `suggestions: [{tag, distance, count}]`.

### B. Rename / Merge-Workflow (Staging) (implementiert)

- Migration `023_tag_inbox.sql`: `tag_inbox (id, raw_tag, action, target_tag,
  status, file_count, created_at, resolved_at)` — globale Entscheidung pro
  Typo-Wortlaut.
- **Rename**: `raw_tag` → neuer Wortlaut (Typo-Fix), assoziiert mit Track(s).
- **Merge**: `raw_tag` → `target_tag` (bestehender Tag). Beim nächsten Write werden
  alle Tracks mit `raw_tag` auf `target_tag` umgetaggt; `raw_tag` verschwindet aus
  allen geschriebenen Kommentaren.
- **Staging-Semantik**: Vor dem Write ist der Mapping nur in der Inbox, nicht in
  den geschriebenen Kommentaren. Beim nächsten Write (`write-comment` / Sync) wird
  der kanonische Tag geschrieben.
- API: `POST /api/inbox/resolve` (einheitlich, `action` rename|merge|dismiss),
  `GET /api/inbox/mappings`.

### C. Frontend-Erweiterung (implementiert)

- Inbox-UI: pro neuem Tag ein Inline-Edit (Rename) + Suggestions-Chips (klickbare
  ähnliche Tags → Merge) + Dismiss-Button; Active-Mappings-Strip über der Tabelle.
- Merge/Rename wirkt sofort auf die Inbox-Ansicht (Refresh des Diffs mit staged
  Target).

### D. Write-Path-Härtung (implementiert)

- `write-comment` / Sync-Pfad respektiert offene Inbox-Mappings: ein gemappter Typo-Tag
  wird beim Schreiben durch den kanonischen Tag ersetzt. Kein Auto-Apply — nur wenn der
  User die Inbox bearbeitet hat.

---

## Acceptance Criteria (Full Feature Set)

- [x] Roundtrip-Diff + Inbox-Liste + Count (Foundation, bereits grün)
- [x] `/api/inbox` liefert pro neuem Tag Similar-Suggestions (Levenshtein ≤ 2,
      case-insensitive) — `newTags[].suggestions[]`, inkl. File-Count pro Vorschlag
- [x] Rename eines neuen Tags in der Inbox persistiert den Mapping
      (`POST /api/inbox/resolve` mit `action=rename`)
- [x] Merge eines Typo-Tags in einen bestehenden Tag taggt ALLE betroffenen Tracks um
      (`action=merge`; Write-Pfad ersetzt den Typo überall durch den kanonischen Tag)
- [x] Typo-Wortlaut verschwindet nach Merge aus dem System (nur Inbox-Historie/
      `tag_inbox`; kein Auto-Apply, erst der nächste Write schreibt den kanonischen Tag)
- [x] Vor Inbox-Bearbeitung: Tag nicht im System committed (Staging — Mapping liegt
      nur in `tag_inbox`, die Tags-Tabelle/Vokabular wird nicht verändert)
- [x] Kein Auto-Apply, kein zirkulärer Pfad (User entscheidet per Klick; Dismiss =
      bewusst ignorieren)
- [x] `cargo build` + `cargo test` grün (abzgl. vorbestehender metaflac-Tests;
      `api_storage`-Concurrency-Tests sind vorbestehend flaky und berühren dieses
      Feature nicht)

## Implementierungsnotizen

- **Similar-Matching**: `levenshtein_distance` (case-insensitive, char-wise) +
  `similar_tags` in `src/comment.rs`; Distanz ≤ 2, Selbst-Match (Distanz 0)
  ausgeschlossen. Vorschläge pro „neuem Tag“ = Tags aus `tags_added ∪ tags_removed`,
  die noch nicht kanonisch im System stehen (oder als Kandidat für Click-to-Merge
  in Frage kommen — auch ein bereits vorhandener Tag kann gemergt werden, wenn der
  User ihn als Typo erkennt).
- **Staging-Tabelle**: `tag_inbox (raw_tag UNIQUE COLLATE NOCASE, action,
  target_tag, status, file_count, …)` — eine Entscheidung pro Typo-Wortlaut
  („taucht nur noch 1× in der Inbox auf“).
- **Write-Pfad**: `compute_target_comment` konsultiert offene rename/merge-Mappings
  (`load_tag_inbox_mapping_map`); `apply_tag_mappings_to_target` ersetzt gemappte
  Tags im generierten Target UND übernimmt gemappte getippte Tags aus dem
  Stored-Comment (sonst würden sie beim Write stillschweigend verworfen).
  Rename-auf-sich-selbst = „Tag behalten“ (File verlässt die Inbox ohne Write).
- **Merge löscht KEINE Tags-Zeile**: Tracks hängen per Playlist-Chain an Tags; das
  Löschen des Typo-Tags würde die Auflösung brechen und Tracks das Tag komplett
  nehmen statt sie auf den kanonischen Tag umzutaggen. Der Mapping im Write-Pfad
  retaggt alle Tracks korrekt und dauerhaft (Playlist-Typo wird bei jedem Target
  auf den kanonischen Tag umgeschrieben).

---

## Build-Fix (bereits im PR #1)

`origin/main` baut nicht: unbenutzte `youtube = "0.1.1"`-Dependency existiert nicht mehr
auf crates.io. Im PR #1 aus `Cargo.toml`/`Cargo.lock` entfernt.
