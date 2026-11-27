# RFC 0004 – Docker, Betrieb und Umsetzung

Status: amd64-Image-Build und Smoke-Test in CI implementiert. PR-Builds überspringen die Registry-Veröffentlichung; Pushes auf `main` veröffentlichen nach erfolgreichem CI.

## Image und Start

Ein OCI-Image unterstützt einen einzelnen Standort und die Hub-Rolle. CI baut zuerst Vite, dann das Rust-Release-Binary und kopiert das Binary in ein schlankes Laufzeitimage mit CA-Zertifikaten und dem Tailscale-CLI. Der Prozess benötigt einen persistenten Schreibpfad für SQLite und Secret-Dateien als separate Mounts. Der Einstieg arbeitet als nicht privilegierter Benutzer mit UID/GID `10001`. CI baut und startet derzeit linux/amd64, führt einen Smoke-Test aus und veröffentlicht nach Push auf `main` die Tags `sha-<commit>` und `latest`. Ein ARM64- und Multiarch-Image ist nicht eingerichtet.

Geplante Operator-Schnittstelle:

| Einstellung | Beschreibung |
| --- | --- |
| `WOLUI_ROLE` | `site` für lokalen Betrieb mit optionalem Connect, `hub` für die zentrale Verbindung |
| `WOLUI_LISTEN` | Bind-Adresse, im Hostnetz standardmäßig Loopback |
| `WOLUI_PUBLIC_URL` | Kanonische HTTPS-Adresse für öffentliche Links und Origin-Prüfung |
| `WOLUI_DATA_DIR` | Persistenter Pfad, im Image standardmäßig `/data` |
| `WOLUI_ACCESS_TOKEN_FILE` | Pfad zu einem pro Installation eingerichteten Token |
| `WOLUI_LOG` | Logging-Level mit Rücksicht auf sensible Daten |

Der Hub läuft hinter einem Reverse Proxy mit HTTPS; die Dokploy-Anleitung beschreibt diesen Betrieb. Das Repository-Compose-Beispiel ist ein lokaler Site-Betrieb mit Host Networking, um sich an die IPv4-Adresse des tatsächlichen LAN-Interfaces zu binden. In diesem Modus sind Port-Mappings wirkungslos. WOL Connect nutzt ausgehendes HTTPS-Polling; nur das optionale Browser-SSH verwendet WSS. Für Schule und Zuhause genügt aus Sicht von WOL UI ausgehendes TCP zu ihrem Hub über HTTPS.

Der Prozess startet mit Read-only-Rootfs, `no-new-privileges` und ohne zusätzliche Linux-Capabilities. UDP-Broadcast auf Port 9 benötigt für den vorgesehenen Sender keinen Raw Socket. Lokale Nachbartabellen werden später je Plattform evaluiert. Docker Desktop und Rootless-Netze sind separate Testfälle. Bis zur Verifikation richten sich Betrieb und Anleitung an Linux Docker Engine mit einem physischen oder dafür eingerichteten virtuellen LAN-Interface.

## Betriebsprofile

| Standort | Betriebsform | Prüfung vor Inbetriebnahme |
| --- | --- | --- |
| Falkenstein | Hub mit bestehendem HTTPS-Reverse-Proxy, persistentes Volume | TLS und WSS von extern, Registry-Zugang, Wiederherstellung vom Backup |
| Zuhause | Site im Host-Netz auf einem Rechner im Zielsegment | Wake-Paket am richtigen Interface, Sender bleibt erreichbar |
| Schule | Site in einem ausdrücklich freigegebenen Netzsegment | Zuständige IT genehmigt Installation und ausgehenden Hub-Verkehr, Test gegen freigegebenes Gerät |
| Einzellösung | Site mit lokalem Management und eigenem Inventar | Anmeldung, Neustart, erfolgreicher Wake ohne Hub |

Wenn Zuhause der Laptop das ausführende System ist, muss er an und verbunden sein. Soll er selbst geweckt werden, übernimmt sein Auftrag beispielsweise ein Raspberry Pi oder NAS im selben LAN. Falkenstein erreicht private Standorte über ihre ausgehenden Kanäle. Am VPS selbst wird kein Broadcast zum Heimnetz oder zur Schule vorausgesetzt.

## Wiederherstellung, Releases und Messung

Health-Endpunkte liefern eine schmale Liveness- und Readiness-Antwort ohne Inventar. Readiness prüft lokal die Datenbank. Offline-Standorte werden in der Oberfläche separat angezeigt. Strukturierte Logs erlauben Korrelation über Auftrags-ID. Die Betriebsanleitung enthält Backup mit SQLite-Backup-Schnittstelle oder bei gestopptem Container, Restore-Probe, Start auf dem vorherigen Image und das Zurückspielen eines zur Version passenden Backups. Beim Klonen einer Installation zu einem zweiten Standort werden Standort-ID und Kopplungscredentials neu erstellt.

CI umfasst Rust-Format, Rust-Tests, Web-Build, Expo-Typecheck/Android-JavaScript-Bundle und Container-Smoke-Test. Ein Connect-Integrationstest mit getrennten Datenbanken und HTTP/UDP-Loopback deckt Pairing, Snapshot und Wake ab. OpenAPI-Vertragsprüfung, echter Zwei-Host-Test, Restore-Probe, reale Netzwerk- und Hardwareabnahme sowie Ressourcenmessung sind noch nicht eingerichtet. Vorgeschlagene Startbudgets ohne SSH-Gateway sind 64 MiB Idle-RSS, 60 MiB komprimierte Image-Größe und lokale API-Annahme binnen 200 ms im 95. Perzentil; diese Werte sind Planungsziele und noch nicht auf Referenzhardware gemessen.

## Umsetzung und Freigabepunkte

1. **Lokaler Schnitt.** Umgesetzt: Cargo- und Bun-Workspace, Browser-Login, Geräteverwaltung, Wake und Docker-Image. Physischer Wake und Restore-Probe bleiben offen.
2. **WOL Connect.** Umgesetzt: Pairing, Hub-Übersicht, Inventar, Wake und Widerruf. Echter Zwei-Host-Test und feinere Rechte bleiben offen.
3. **Betreibbarer Release.** Runbooks und CI-Publish-Regel sind vorhanden. GHCR-Veröffentlichung des aktuellen Heads und produktiver Referenzbetrieb stehen nach Merge auf `main` aus.
4. **Expo.** Umgesetzt: Login, Standort-/Geräteauswahl, Startaktion und Tailscale-Konsolenlink. Native iOS-/Android-Store-Builds und Geräteprüfung bleiben offen.
5. **Browser-SSH.** Umgesetzt mit konfigurierbarem Gateway, Geräterecht, Audit und Tailscale-CLI. Live-Abnahme mit einem erlaubten Tailnet-Ziel steht aus.

Offen sind ein produktiver Hub mit öffentlicher Adresse, reale Wake-Testgeräte samt Energiestatus, die Tailnet-Identität für Browser-SSH, ein Restore-Probeablauf und ARM64. Diese Punkte brauchen Betreiberzugänge oder Referenzhardware.

[Docker Host Networking](https://docs.docker.com/engine/network/drivers/host/)
