# RFC 0001 – Server, Wake-on-LAN und Geräte

Status: Entwurf

## Ziel

Ein Linux-Rechner stellt Weboberfläche und API bereit, speichert Geräte dauerhaft und weckt Rechner im eigenen Netzsegment. Der erste nutzbare Ablauf dauert wenige Minuten: anmelden, MAC-Adresse hinzufügen, Netzwerk auswählen, Start drücken, Ergebnis prüfen.

## Stack und Monorepo

| Pfad | Zuständigkeit |
| --- | --- |
| `crates/wol-core` | Geräte, Netzwerkprofile, Wake-Paket, Zustände und Validierung |
| `crates/wol-server` | Tokio-Runtime, Axum-Routen, Socket-Ausführung, SQLite und eingebettete Web-Dateien |
| `crates/wol-protocol` | Nachrichten und Versionierung der Standortkopplung |
| `apps/web` | React, TypeScript, Vite |
| `apps/mobile` | Expo, React Native, Expo Router |
| `packages/api-client` | Generierte HTTP-Typen, API-Aufrufe und gemeinsame Fehlerabbildung |
| `deploy/` | Dockerfile, Compose-Profile und Beispiele für den TLS-Reverse-Proxy |
| `docs/` | RFCs, Geräteanleitungen und Betrieb |

Cargo Workspace für Rust und Bun Workspace für TypeScript. Axum ist die vorgeschlagene Zuordnung zu Tokio.rs. Tower übernimmt HTTP-Middleware, Serde die Serialisierung, tracing strukturierte Logs und SQLx Migrationen und Zugriff auf SQLite. Axum liefert das Vite-Build aus demselben Prozess aus. Der Cargo-Build übernimmt die fertigen Web-Assets. API- und Asset-Routen bleiben von der SPA-Fallback-Route getrennt. Festgeschriebene Lockfiles und Toolchains machen Builds nachvollziehbar. Actix Web wäre eine Alternative für HTTP. Next.js bringt für diese angemeldete Oberfläche wenig zusätzlichen Nutzen, da keine serverseitige Seitenerzeugung geplant ist.

## Daten und API

SQLite auf lokalem persistentem Volume mit Foreign Keys, WAL und versionierten Migrationen. Tabellen für Installation, Netzprofile, Geräte, Sitzungen, Kopplungen, Wake-Aufträge und Audit. Jede Installation erhält eine dauerhafte zufällige ID. Geräte besitzen eine eigene undurchsichtige ID und eine eindeutige Kombination aus Netzprofil und normalisierter MAC. Ein Netzprofil enthält Interface, IPv4-Adresse oder CIDR und die daraus ermittelte Broadcast-Adresse. Ein optionaler Tailscale-Name gehört zu den Verbindungsdaten des Geräts. Änderungen am Netzprofil sind administrativ und werden protokolliert.

| HTTP | Zweck |
| --- | --- |
| `POST /api/v1/sessions` | Zugangstoken gegen eine Sitzung tauschen |
| `DELETE /api/v1/sessions/current` | Sitzung widerrufen |
| `GET /api/v1/sites` | Lokale Installation und gekoppelte Standorte |
| `GET /api/v1/sites/{site}/devices` | Inventar des Standorts |
| `POST /api/v1/sites/{site}/devices` | Gerät anlegen |
| `PATCH /api/v1/sites/{site}/devices/{id}` | Gerät bearbeiten, mit Revision gegen verlorene Änderungen |
| `DELETE /api/v1/sites/{site}/devices/{id}` | Gerät entfernen |
| `POST /api/v1/sites/{site}/devices/{id}/wake` | Auftrag mit Idempotency-Key anlegen, Antwort 202 mit Auftrags-ID |
| `GET /api/v1/sites/{site}/wake-jobs/{id}` | Ergebnis abrufen |

JSON-Fehler enthalten einen stabilen Code, Request-ID und gegebenenfalls Feldnamen. OpenAPI aus Rust-Definitionen, etwa über utoipa. CI vergleicht Spezifikation und generierte TypeScript-Typen. Eine Anfrage mit demselben Schlüssel und identischer Operation liefert innerhalb der Aufbewahrungsfrist denselben Auftrag. Derselbe Schlüssel mit abweichendem Inhalt liefert 409. Die Aufbewahrungsfrist muss länger sein als jedes erlaubte Wiederholungsfenster.

## Paketausführung

WOL wird im Rust-Code umgesetzt. Der UDP-Payload besteht aus sechs Bytes FF und sechzehn Wiederholungen der sechs Byte langen Ziel-MAC. Der Sender bindet an die im Profil festgelegte lokale IPv4-Adresse und verwendet einen Datagramm-Socket mit Broadcast-Option. Standardport ist 9. Der Handler erhält nur Standort und Geräte-ID, prüft die Berechtigung und lädt MAC und Zielsegment aus der Datenbank. Der Benutzer kann im Wake-Aufruf weder Broadcast-Adresse noch Port überschreiben.

Vor dem Versand wird der Auftrag dauerhaft erfasst. Ein Worker reserviert ihn atomar, prüft Profil, Geräterevision und Ablaufzeit und sendet mit einem kurzen Timeout. Der erste Release sendet ein Datagramm pro angenommenem Auftrag. Der Worker persistiert danach `sent` oder `failed`. Bei Prozessabbruch zwischen Reservierung und Ergebnis erhält der Auftrag `unknown`. Ein solcher Auftrag wird nie automatisiert erneut gesendet. Lokale und entfernte Aufträge verwenden diese selben Regeln. Ein begrenzter Cooldown pro Gerät bremst Doppelklicks und mehrere Bediengeräte.

`sent` bedeutet, dass der lokale Socket das Paket angenommen hat. Start und Erreichbarkeit sind getrennte Beobachtungen. Optional wird später ein explizit konfigurierter Dienst am Ziel beobachtet. Ein Timeout zeigt „Erreichbarkeit unbekannt“. Ein schlafender Rechner kann in der Nachbartabelle fehlen. Das Formular unterstützt daher zuerst MAC-Eingabe und später explizit ausgelöste Vorschläge aus der lokalen Nachbartabelle des gewählten Interfaces. Vorschläge werden vor dem Speichern bestätigt. In schulischen VLANs ist pro Zielsegment ein berechtigter Sender der Referenzfall.

## Zugang und Betrieb der API

Der Betreiber hinterlegt bei der Einrichtung einen kryptographisch zufälligen Zugangstoken mit mindestens 32 Byte Entropie über eine Secret-Datei. Der Token wird serverseitig über einen Digest und konstantzeitlichen Vergleich geprüft. Der Browser erhält einen opaken Sitzungscookie mit Secure, HttpOnly, SameSite und definierter Gültigkeit. Für schreibende Browseranfragen prüft der Server zusätzlich den Origin und ein sitzungsgebundenes CSRF-Token. Die App nutzt einen separat ausgestellten widerrufbaren Inhaber-Token. Die Datenbank speichert Sitzungstoken als Digests. Login ist rate-limitiert. Bei Rotation des Einrichtungs-Tokens werden daraus abgeleitete Sitzungen widerrufen. Bearer-Header, Cookies und Pairing-Secrets werden in allen Logs redigiert. Management läuft über HTTPS, für lokale Entwicklung gibt es einen expliziten Loopback-Modus.

## Abnahme

- Referenzvektor für den 102-Byte-Payload, Prüfung ungültiger, Multicast- und Null-MACs sowie korrekte Broadcast-Berechnung.
- Integrationstest prüft Anmeldung, Ablauf und Widerruf, Admin-Routen und mehrfaches Wake mit demselben Schlüssel.
- Ein Linux-Docker-Host sendet nachweislich auf dem gewählten Interface. Ein separater Rechner wird aus einem unterstützten Schlafzustand geweckt.
- Neustart erhält Geräte und Auftragsdaten. Abbruch unmittelbar nach Senden ergibt einen sichtbar unklaren Ausgang.

[Axum](https://github.com/tokio-rs/axum) · [CLIProxyAPI Management API](https://github.com/router-for-me/CLIProxyAPIDocs/blob/main/docs/en/management/api.md)
