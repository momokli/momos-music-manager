# MMM ist sunset — wohin stattdessen

**Momo's Music Manager (MMM)** ist **sunset**: die App ist feature-freezed und wird
nur noch bis zum Abschluss der Migration gepflegt (Bugfixes, keine neuen Features).

Wenn du bisher MMM benutzt hast, ist der neue Stack dein Zuhause. Diese Seite sagt
dir gebündelt, wohin was wandert.

## Wohin statt MMM

| Aufgabe | Neues Projekt | Wo |
| --- | --- | --- |
| Deine Library, Tags, Playlists, Digging | **music-hub** | [`music-hub/`](../music-hub/) |
| Lokaler Begleiter auf Mac/PC: Backups, Traktor-`collection.nml`-Upload, Downloads | **kellner** | [`kellner/`](../kellner/) |
| Downloads, Object-Store, BPM-Key-Metadaten | **music-api** | [`music-api/`](../music-api/) |

Kurz: **music-hub** ist das neue Zuhause der Library und der Ort, an dem du
tagst, diggst und Playlists baust. **kellner** läuft lokal auf deinem Mac/PC und
kümmert sich um Backups, den Upload deiner Traktor-`collection.nml` und
Downloads. **music-api** ist die Autorität hinter den Kulissen: Downloads,
Object-Store und die BPM-Key-Metadaten.

## Umstieg

- Deine Daten werden bzw. wurden in den **Hub migriert** (siehe Issue #372).
- **MMM läuft read-only weiter**, bis die Migration abgeschlossen ist — du
  verlierst also nichts, aber neue Arbeit passiert im Hub.
- Details zum Freeze-Zustand: [`../legacy/mmm/FROZEN.md`](../legacy/mmm/FROZEN.md)
  (Escape-Label `mmm:unfreeze`).
- Kontext und Reihenfolge: [`ROADMAP.md`](ROADMAP.md) und
  [`ARCHITECTURE.md`](ARCHITECTURE.md).
- Die archivierte MMM-Doku: [`legacy/mmm-README.md`](legacy/mmm-README.md).