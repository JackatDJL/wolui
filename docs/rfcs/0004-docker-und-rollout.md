# RFC 0004 – Docker, Betrieb und Umsetzung

Status: Entwurf

## Image und Start

Ein OCI-Image unterstützt einen einzelnen Standort und die Hub-Rolle. CI baut zuerst Vite, dann das Rust-Release-Binary und kopiert das Binary in ein schlankes Laufzeitimage mit CA-Zertifikaten. Der Prozess benötigt einen persistenten Schreibpfad für SQLite und Secret-Dateien als separate Mounts. Der Einstieg arbeitet als nicht privilegierter Benutzer mit fest dokumentierter UID. Multiarch-Ziele sind linux/amd64 und linux/arm64. CI testet jedes Release-Image und veröffentlicht bei eingerichteter Registry Versions-Tags und einen Multiarch-Digest.

Geplante Operator-Schnittstelle:

| Einstellung | Beschreibung |
| --- | --- |
| `WOLUI_ROLE` | `site` für lokalen Betrieb mit optionalem Connect, `hub` für die zentrale Verbindung |
| `WOLUI_LISTEN` | Bind-Adresse, im Hostnetz standardmäßig Loopback |
| `WOLUI_PUBLIC_URL` | Kanonische HTTPS-Adresse für öffentliche Links und Origin-Prüfung |
| `WOLUI_DATA_DIR` | Persistenter Pfad, im Image standardmäßig `/data` |
| `WOLUI_ACCESS_TOKEN_FILE` | Pfad zu einem pro Installation eingerichteten Token |
| `WOLUI_LOG` | Logging-Level mit Rücksicht auf sensible Daten |

Der Hub läuft unter Docker Compose im Bridge-Netz. Ein vorhandener Reverse Proxy terminiert HTTPS und leitet WebSocket-Upgrades weiter. Wenn der Proxy auf dem Host läuft, veröffentlicht Compose den App-Port auf Loopback. Bei einem Container-Proxy erfolgt die Verbindung über ein gemeinsames privates Docker-Netz. Der Standort läuft als Linux-Referenz mit Host Networking, um sich an die IPv4-Adresse des tatsächlichen LAN-Interfaces zu binden. In diesem Modus sind Port-Mappings wirkungslos. Lokales Management kann über einen Reverse Proxy oder über einen administrativen Loopback-Zugang erfolgen. Die WebSocket-Verbindung zum Hub geht vom Standort aus. Für Schule und Zuhause genügt aus Sicht von WOL UI ausgehendes TCP zu ihrem Hub über HTTPS.

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

CI umfasst Format, Lint, Rust- und Client-Builds, API-Vertragsprüfung, Container-Smoke-Test und einen Zwei-Standort-Test. Ein Release enthält geprüfte Anleitungen für Hub, Standort und erste Geräte. Die Referenzaufbauten werden mit tatsächlichem Netzwerkverkehr und einem realen Wake-Ziel getestet. Ein dokumentiertes Messprofil erfasst Idle-RSS, Image-Größe, CPU im Leerlauf, Auftragslatenz und Arbeitsspeicher bei mehreren verbundenen Standorten. Vorgeschlagene Startbudgets ohne SSH-Gateway sind 64 MiB Idle-RSS, 60 MiB komprimierte Image-Größe und lokale API-Annahme binnen 200 ms im 95. Perzentil. Diese Werte sind Planungsziele und werden auf Referenzhardware gemessen.

## Umsetzung und Freigabepunkte

1. **Lokaler Schnitt.** Cargo- und Bun-Workspace, Browser-Login, Geräteverwaltung, Wake und Docker-Image. Freigabe nach Paketprüfung plus realem Wake und Datenpersistenz.
2. **WOL Connect.** Kopplung, Hub-Übersicht, lokale Rechte, Wiederverbindung und Audit. Freigabe nach isoliertem Mehrstandorttest und Widerrufstest.
3. **Betreibbarer Release.** Compose-Beispiele, Registry-Veröffentlichung, Backup- und Upgrade-Anleitung, Geräteanleitungen sowie ein Referenzbetrieb mit Hub und einer Standortinstanz. Schule kommt hinzu, sobald die Freigabe vorliegt.
4. **Expo.** Native Android- und iOS-Builds, Sitzungen, Serverauswahl, Startaktion und Browser-Übergabe. Freigabe nach Prüfung auf beiden Plattformen.
5. **Browser-SSH.** Tailscale-Einstieg kann bereits mit der Web-Oberfläche ausgeliefert werden. Integriertes Terminal folgt nach der Prüfung von Identität, Rechtevergabe und Zielerreichbarkeit aus RFC 0003.

Offen für die Implementierung sind Registry und Release-Namensraum, die öffentliche oder private Hub-Adresse, die Testgeräte samt Energiestatus und die Tailnet-Identität für Browser-SSH. Der lokale Schnitt kann unabhängig davon beginnen.

[Docker Host Networking](https://docs.docker.com/engine/network/drivers/host/)
