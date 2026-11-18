// pull-playlists.ts
// Holt Playlists aus deiner Apple-Music-Bibliothek (privat, per User-Token)
// und ergänzt pro Song die Katalog-Metadaten (inkl. ISRC).
//
// Ausgabe:
//   playlists.raw.json   vollständige Antworten (Archiv, NICHT ins Repo!)
//   playlists.slim.json  schlanke, flache Variante (später an Vertrag §3a anpassen)
//
// Umgebungsvariablen (nie in den Code schreiben):
//   APPLE_DEV_TOKEN    Bearer-Token (eyJ0eXAi...) vom Scraper
//   APPLE_USER_TOKEN   dein media-user-token aus dem Browser-Cookie
//   APPLE_STOREFRONT   optional, Ländercode, Standard: de
//   APPLE_ONLY         optional, Playlist-Namen, kommagetrennt (Alternative zu --only)
//
// Aufruf:
//   npx tsx pull-playlists.ts                   alle Playlists
//   npx tsx pull-playlists.ts --list            nur auflisten (Name, ID, Songanzahl)
//   npx tsx pull-playlists.ts --only "all,Gym"  nur diese Playlists (Name, ohne Groß/Klein)

import { writeFile } from 'node:fs/promises';

const BASE = 'https://amp-api.music.apple.com';
const DEV_TOKEN = process.env.APPLE_DEV_TOKEN;
const USER_TOKEN = process.env.APPLE_USER_TOKEN;
const STOREFRONT = process.env.APPLE_STOREFRONT ?? 'de';

const args = process.argv.slice(2);
const LIST_ONLY = args.includes('--list');
const onlyIdx = args.indexOf('--only');
const onlyRaw = onlyIdx >= 0 ? args[onlyIdx + 1] : process.env.APPLE_ONLY;
const ONLY = onlyRaw
    ?.split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);

if (!DEV_TOKEN) throw new Error('APPLE_DEV_TOKEN ist nicht gesetzt');
if (!USER_TOKEN) throw new Error('APPLE_USER_TOKEN ist nicht gesetzt');

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Eine Anfrage an die API (Retry bei 429 = zu viele Anfragen)
async function apiGet(path: string, retries = 3): Promise<any> {
    const res = await fetch(BASE + path, {
        headers: {
            Authorization: `Bearer ${DEV_TOKEN}`,
            'Music-User-Token': USER_TOKEN!,
            Origin: 'https://music.apple.com',
            'User-Agent':
                'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36',
        },
    });

    if (res.status === 429 && retries > 0) {
        await sleep(2000);
        return apiGet(path, retries - 1);
    }
    if (res.status === 401 || res.status === 403) {
        throw new Error(
            `Token abgelehnt (HTTP ${res.status}). Dev-Token oder User-Token ist ungültig/abgelaufen. ` +
                `Hol dir einen neuen media-user-token aus dem Browser (Cookie auf music.apple.com).`
        );
    }
    if (!res.ok) {
        throw new Error(`HTTP ${res.status} bei ${path}`);
    }
    return res.json();
}

// Alle Seiten eines Endpunkts einsammeln (Apple liefert max. ~100 pro Seite)
async function getAll(path: string): Promise<any[]> {
    const items: any[] = [];
    let next: string | undefined = path;
    while (next) {
        const json = await apiGet(next);
        items.push(...(json.data ?? []));
        next = json.next; // undefined, wenn es keine weitere Seite gibt
    }
    return items;
}

// Songs aus dem Katalog in Blöcken nachladen -> Map: Katalog-ID -> Song
async function getCatalogSongs(ids: string[]): Promise<Map<string, any>> {
    const map = new Map<string, any>();
    const unique = [...new Set(ids)];

    for (let i = 0; i < unique.length; i += 100) {
        const chunk = unique.slice(i, i + 100).join(',');
        try {
            // nur Album-Infos mitladen; Künstler-Ballast (Alben des Künstlers) bewusst weggelassen
            const json = await apiGet(`/v1/catalog/${STOREFRONT}/songs?ids=${chunk}&include=albums`);
            for (const song of json.data ?? []) map.set(song.id, song);
        } catch (e) {
            if (String(e).includes('Token abgelehnt')) throw e; // sofort abbrechen
            console.warn(`  Katalog-Block ab ${i} fehlgeschlagen:`, String(e));
        }
        await sleep(200);
    }
    return map;
}

async function main() {
    console.log('Hole Playlists ...');
    const all = await getAll('/v1/me/library/playlists?limit=100');
    console.log(`${all.length} Playlists gefunden.\n`);

    // --list: nur auflisten, ohne Songs zu laden
    if (LIST_ONLY) {
        for (const pl of all) {
            console.log(`${(pl.attributes?.name ?? '(ohne Name)').padEnd(40)} ${pl.id}`);
        }
        console.log('\nTipp: --only "name1,name2" lädt gezielt einzelne Playlists.');
        return;
    }

    const playlists = ONLY
        ? all.filter((p) => ONLY.includes((p.attributes?.name ?? '').toLowerCase()))
        : all;

    if (ONLY) {
        console.log(`Filter aktiv: ${playlists.length} von ${all.length} Playlists ausgewählt.\n`);
        if (playlists.length === 0) {
            console.log('Keine Playlist passt. Mit --list siehst du die genauen Namen.');
            return;
        }
    }

    const raw: any[] = [];
    const slim: any[] = [];
    let totalTracks = 0;
    let withIsrc = 0;

    for (const [index, pl] of playlists.entries()) {
        const name = pl.attributes?.name ?? '(ohne Name)';
        console.log(`[${index + 1}/${playlists.length}] ${name}`);

        // Songs der Playlist; leere Playlists liefern teils einen Fehler
        let tracks: any[] = [];
        try {
            tracks = await getAll(`/v1/me/library/playlists/${pl.id}/tracks?limit=100`);
        } catch (e) {
            if (String(e).includes('Token abgelehnt')) throw e;
            console.warn('  Songs konnten nicht geladen werden:', String(e));
        }

        // Katalog-IDs sammeln und volle Metadaten holen
        const catalogIds = tracks
            .map((t) => t.attributes?.playParams?.catalogId)
            .filter((id): id is string => Boolean(id));
        const catalog = await getCatalogSongs(catalogIds);

        const merged = tracks.map((t) => {
            const catalogId: string | null = t.attributes?.playParams?.catalogId ?? null;
            return {
                libraryId: t.id,
                catalogId,
                library: t.attributes,
                catalog: catalogId ? (catalog.get(catalogId) ?? null) : null,
            };
        });

        raw.push({
            id: pl.id,
            name,
            attributes: pl.attributes,
            trackCount: merged.length,
            tracks: merged,
        });

        slim.push({
            id: pl.id,
            name,
            trackCount: merged.length,
            tracks: merged.map((t, position) => {
                const c = t.catalog?.attributes;
                const l = t.library;
                const album = t.catalog?.relationships?.albums?.data?.[0]?.attributes;
                const isrc: string | null = c?.isrc ?? null;
                totalTracks++;
                if (isrc) withIsrc++;
                return {
                    position,
                    title: c?.name ?? l?.name ?? null,
                    artist: c?.artistName ?? l?.artistName ?? null,
                    album: c?.albumName ?? l?.albumName ?? null,
                    isrc,
                    durationMs: c?.durationInMillis ?? l?.durationInMillis ?? null,
                    releaseDate: c?.releaseDate ?? l?.releaseDate ?? null,
                    genres: c?.genreNames ?? l?.genreNames ?? [],
                    appleCatalogId: t.catalogId,
                    upc: album?.upc ?? null,
                    url: c?.url ?? null,
                };
            }),
        });

        await sleep(200);
    }

    await writeFile('playlists.raw.json', JSON.stringify(raw, null, 2), 'utf8');
    await writeFile('playlists.slim.json', JSON.stringify(slim, null, 2), 'utf8');

    console.log('\nFertig.');
    console.log(`  ${playlists.length} Playlists, ${totalTracks} Songs`);
    console.log(`  mit ISRC: ${withIsrc}, ohne ISRC: ${totalTracks - withIsrc}`);
    console.log('  Gespeichert: playlists.raw.json, playlists.slim.json');
}

main().catch((e) => {
    console.error('Fehler:', e instanceof Error ? e.message : e);
    process.exit(1);
});
