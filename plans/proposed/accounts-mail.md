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
