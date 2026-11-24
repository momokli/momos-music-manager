# Plan: accounts-mail

**Status**: proposed
**Milestone**: `hub-accounts-0.33.0` (#46)
**Migration needed**: ja, additiv (`hub_email_tokens`, `hub_users.email` UNIQUE)

> **Ziel**: Menschen kommen ohne Admin-Handgriff in den Hub und wieder heraus —
> Einladungslink zur Registrierung, Magic-Link zum Passwort-Reset. Admins erzeugen
> Links fuer **beliebige** User und stellen sie selbst zu, falls kein Mailversand
> konfiguriert ist.

## Ausgangslage (verifiziert 2026-11-17, `main`)

| Teil | Stand |
| --- | --- |
| Routen | nur `/login` (GET+POST) und `/logout` (POST) — **kein** `/register`, **kein** Reset |
| Auth | bcrypt-Verify, Session-Cookie `hub_session`, Tabelle `hub_web_sessions` |
| `hub_users` | `id, slug, display_name, email, created_at` — `email` **nullable und nicht UNIQUE** |
| Mail | **keine Infrastruktur** (kein `lettre`, SMTP, Resend, Postmark, Mailgun) |
| Schalter | `settings::REGISTRATION_OPEN` existiert und wird an das Login-Formular gegeben, ist aber nur Anzeige |

## Entscheidungen (Vorschlag, im Review aenderbar)

| # | Entscheidung | Begruendung |
| --- | --- | --- |
| D1 | **Token = 32 Byte random (base64url)**, in der DB nur der **SHA-256-Hash** | Ein DB-Leak darf keine gueltigen Links liefern |
| D2 | TTL **30 min** fuer `reset`, **7 Tage** fuer `register`; beide single-use | Reset ist dringend, eine Einladung darf liegen bleiben |
| D3 | Mailversand hinter `Mailer`-Trait, `SmtpMailer` (lettre) per Env; ohne Konfiguration `NoopMailer` | Provider-Entscheidung blockiert nicht den Rest; Admin-Links funktionieren immer |
| D4 | Admin-Links nutzen **denselben** Token-Mechanismus, nur ohne Versand: Link wird einmal angezeigt | Kein zweiter Codepfad, gleiche Sicherheitseigenschaften |
| D5 | Neutrale Antworten + konstante Zeit bei `/forgot` und `/register` | Keine User-Enumeration |
| D6 | `hub_users.email` wird `UNIQUE COLLATE NOCASE` | Ohne das ist Reset per Adresse mehrdeutig |

## Datenmodell (eine additive Migration)

```sql
CREATE TABLE hub_email_tokens (
  id          INTEGER PRIMARY KEY,
  user_id     INTEGER REFERENCES hub_users(id),   -- NULL bei Einladung ohne Account
  email       TEXT NOT NULL,                      -- Adresse, an die der Token gebunden ist
  purpose     TEXT NOT NULL CHECK (purpose IN ('register','reset')),
  token_hash  TEXT NOT NULL,                      -- SHA-256(hex) des Klartext-Tokens
  expires_at  TEXT NOT NULL,
  used_at     TEXT,
  created_by  INTEGER REFERENCES hub_users(id),   -- Admin-Link: wer ihn erzeugt hat
  created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX hub_email_tokens_hash ON hub_email_tokens(token_hash);
CREATE INDEX hub_email_tokens_user ON hub_email_tokens(user_id, purpose, created_at);
```

`hub_users.email` bekommt einen `UNIQUE`-Index. **Vorpruefung** vor dem Deploy: bestehende
Duplikate und `''`-Werte auflisten (leer → `NULL`), sonst scheitert die Migration.

## Routen

| Route | Zweck |
| --- | --- |
| `POST /forgot` | erzeugt `reset`-Token (nur wenn Adresse existiert) + mailt |
| `GET/POST /reset?token=…` | Passwort setzen; invalidiert **alle** Sessions des Users |
| `GET/POST /register?token=…` | Einladung einloesen (Adresse kommt aus dem Token) |
| `POST /admin/users/{id}/link` | Admin: Link fuer beliebigen User erzeugen, **einmal** anzeigen |
| `POST /admin/invites` | Admin: Einladung fuer eine Adresse ohne Account |

Rate-Limits: pro IP **und** pro Adresse; Cooldown je User. Alle Token-Routen: `no-store`.

## Umsetzung ohne Mail (Branch `feat/accounts-password`)

Zwei Teile brauchen keine Provider-Entscheidung und laufen voraus:

| Teil | Stand |
| --- | --- |
| Passwort-Storage | bcrypt `$2b$`, Cost **12**, Salt pro Hash (`music-hub/src/auth.rs`). Hashes unter Cost 12 werden beim naechsten Login re-gehasht. Prod-Stand vor der Aenderung: alle drei Accounts bereits `$2b$10$` (also gesalzen, nur schwaechere Cost). |
| `music-hub set-password` (CLI) | liest per `--password-stdin` statt argv (argv landet in `ps`/History), erzwingt `MIN_PASSWORD_LEN` und beendet **alle** Sessions des Users |
| `POST /account/password` | eingeloggt: altes + neues + Wiederholung, min. 8 Zeichen, beendet **andere** Sessions (die eigene bleibt) — verlinkt aus `#settings` |
| Migration `068` | `hub_email_tokens` + `email UNIQUE COLLATE NOCASE` (partial index: NULLs kollidieren nicht). Schema steht, Routen folgen |

Damit ist #558/#559/#560 reine Routen-Arbeit auf vorhandenem Unterbau.

**Offene Voraussetzung fuer den Mail-Weg**: kein Account hat eine E-Mail (Prod-Stand
2026-11-22: `momo`, `Mctoastus`, `ANKD` alle `NULL`). Ohne Adresse laeuft `/forgot` fuer
niemanden; bis dahin sind Admin-Links (#560) der einzige Reset-Weg ausser der CLI.

## Umgesetzt (Branch `feat/accounts-password`, 2026-11-22)

| Slice | Datei | Was |
| --- | --- | --- |
| Passwort-Storage | `music-hub/src/auth.rs` | bcrypt `$2b$`, Cost **12**, Salt pro Hash; Hashes unter Cost 12 werden beim naechsten Login re-gehasht |
| CLI `set-password` | `music-hub/src/main.rs` | `--password-stdin` statt argv (argv landet in `ps`/History), Min-Laenge, beendet **alle** Sessions |
| `POST /account/password` | `music-hub/src/account.rs` | eingeloggt aendern (alt + neu + Wiederholung), beendet **andere** Sessions |
| Migration `068` | `migrations/068_hub_accounts_mail.sql` | `hub_email_tokens` + `email UNIQUE COLLATE NOCASE` (partial index, NULLs kollidieren nicht) |
| `Mailer` | `music-hub/src/mail.rs` | Trait + `NoopMailer` (Default) + `SmtpMailer` (lettre, Rustls, kein OpenSSL) |
| Tokens | `music-hub/src/mail_tokens.rs` | 32-Byte-Token, **nur SHA-256** gespeichert, single-use via `UPDATE … RETURNING`, TTL 30 min (reset) / 7 d (register) |
| `/forgot` + `/reset` | `music-hub/src/account.rs` | neutrale Antwort **und** gleicher Codepfad fuer bekannte/unbekannte Adressen, Throttle pro Adresse, Reset beendet alle Sessions |
| CLI `set-email`, `link` | `music-hub/src/main.rs` | Adresse setzen; Reset-Link einmalig ausgeben (Admin-Fallback ohne Mail) |

Nicht enthalten: `/register` per Einladung (#559) und die Admin-UI fuer Links
(#560 — die CLI deckt den Bedarf). Tests: 9 Integrationstests (`tests/mail_reset.rs`),
7 fuer die Passwortaenderung (`tests/account_password.rs`).

**Offene Voraussetzung**: kein Account hat eine E-Mail (Prod-Stand 2026-11-22:
`momo`, `Mctoastus`, `ANKD` alle `NULL`). Ohne Adresse laeuft `/forgot` fuer
niemanden; `music-hub set-email` und `music-hub link` sind bis dahin der Weg.

## Mailtransport: Relay statt Selbstzustellung (#555)

Gemessen am 2026-11-22 auf dem Deploy-Host `192.168.1.4`:

| Messung | Ergebnis |
| --- | --- |
| Ausgehend Port 25 | **offen** (`220 mx.google.com ESMTP …`) |
| Ausgehend Port 587 | offen |
| PTR der oeffentlichen IP `79.140.117.252` | **keiner** |
| MX / TXT (SPF) fuer `zukkafabrik.de` | **keine** |

Port 25 offen heisst nicht zustellbar. Gmail/Outlook verlangen fuer Direktversand
einen forward-confirmed PTR, den eine Vodafone-DSL-Pool-IP nicht hat: die Mail
wird abgewiesen oder aussortiert — unabhaengig davon, ob den Absender Spam
stoert. Der Entscheider ist der empfangende Server, nicht wir.

Deshalb spricht `SmtpMailer` Submission (587 STARTTLS bzw. 465 Wrapper) mit einem
bestehenden Postfach oder Dienst; Konfiguration ueber `HUB_SMTP_HOST`,
`HUB_SMTP_PORT`, `HUB_SMTP_TLS` (`starttls` \| `wrapper` \| `none`),
`HUB_SMTP_USERNAME`, `HUB_SMTP_PASSWORD`, `HUB_SMTP_FROM`. Ohne `HUB_SMTP_HOST`
bleibt `NoopMailer` aktiv: alle Routen laufen, nichts wird verschickt, und
Admin-Links (`music-hub link --user <slug>`) decken den Bedarf.

## Meilensteine → Issues

- **#555** [Design] Mail-Provider + Absenderdomain (SPF/DKIM) — Mensch entscheidet
- **#556** Migration `hub_email_tokens` + `email` UNIQUE
- **#557** `Mailer`-Trait + SMTP (Env), `NoopMailer` ohne Konfig
- **#558** `/forgot` + `/reset`
- **#559** `/register` per Einladung (Adresse aus dem Token)
- **#560** Admin-Links (register/reset) fuer beliebige User

Reihenfolge: 556 → 557 → 558 → 559 → 560. #555 kann parallel entschieden werden; ohne
Entscheidung laufen alle Routen, nur ohne Versand (Admin-Links decken den Bedarf).

## Risiken

- **Zustellbarkeit**: ohne SPF/DKIM landet alles im Spam — deshalb ist #555 eine
  Menschen-Entscheidung, nicht Code.
- **Enumeration**: jede Abweichung in Antwortzeit oder Text verraet, welche Adressen existieren.
- **Token-Leaks**: Referer (deshalb nach dem Absenden Redirect ohne Token), Logs (nie loggen),
  und die Admin-Anzeige (einmalig, danach nur der Hash).
- **Fremde Adressen**: ein Admin-Link fuer eine falsche Adresse ist ein Account-Uebernahme-Weg —
  deshalb Audit (`created_by`) und einmalige Anzeige.
- **Migration**: `email UNIQUE` ist die einzige nicht-additive Aenderung im Sinne der Daten
  (Constraint), deshalb Vorpruefung + DB-Backup (der Deploy sichert bei neuen Migrationen).

## Tests

- Token-Lebenszyklus: gueltig / abgelaufen / benutzt / fremde Adresse → gleiche neutrale Seite.
- `/reset` invalidiert alle Sessions des Users.
- `/register` mit Token fuer Adresse A + Formularadresse B ⇒ abgelehnt.
- Admin-Link: Nicht-Admin 403, fremder User 403, Link nach Reload nicht mehr abrufbar.
- `NoopMailer`: Routen funktionieren, es wird nichts versendet; Mock-SMTP-Test fuer `SmtpMailer`.
- Kein Token in Logs (tracing-Capture).
