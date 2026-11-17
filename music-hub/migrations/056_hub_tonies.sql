-- Tonies-Account-Verbindung (#499, M1): genau EINE Zeile pro User, die die
-- verschlüsselte Tonies-Session hält. Additiv und klein — `hub_tonie_bindings`
-- und `hub_tonie_syncs` kommen erst mit M2.
--
-- `secret_enc` ist base64(nonce(12 B) || AES-256-GCM-Ciphertext(+Tag)) des
-- Keycloak-Refresh-Tokens. Das Klartext-Passwort wird NIE gespeichert, und es
-- liegt auch kein Klartext-Token in dieser Tabelle. Fehlermeldungen werden nur
-- kategorisiert (kein Credential-Klartext) in `last_error` abgelegt.

CREATE TABLE hub_tonies_accounts (
    user_id       INTEGER PRIMARY KEY REFERENCES hub_users(id) ON DELETE CASCADE,
    household_id  TEXT,
    tonie_email   TEXT,
    secret_enc    TEXT NOT NULL,
    last_login_at TEXT,
    last_error    TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

SELECT 'Migration 056 applied: tonies accounts' AS status;
