# Lokaler Linux-Betrieb

Dieses Compose-Beispiel betreibt eine einzelne `site`-Instanz mit Docker Engine
auf Linux. Es verwendet Host Networking, damit der Server das LAN-Interface des
Rechners für Wake-on-LAN nutzen kann. Bei `network_mode: host` sind Docker-
Portfreigaben wirkungslos. Docker Desktop und Rootless Networking sind nicht
Teil dieses Referenzbetriebs.

## Voraussetzungen

- Linux mit Docker Engine und Docker Compose v2
- Ein LAN-Interface im gewünschten Zielsegment
- Freier Listener-Port, standardmäßig `127.0.0.1:3000`

## Einrichten und starten

Lege in `deploy/.env` mindestens ein Profil für das tatsächliche LAN an.
Ersetze Interface, Host-IP und Broadcast-Adresse im Beispiel durch die Werte
des Linux-Hosts im Zielsegment.

```dotenv
WOLUI_NETWORK_PROFILES='[{"id":"home","name":"Heimnetz","interface":"enp3s0","bind_address":"192.168.1.10","broadcast_address":"192.168.1.255"}]'
```

Im Repository-Root:

```sh
mkdir -p deploy/data deploy/secrets
umask 077
openssl rand -base64 48 > deploy/secrets/access-token
chmod 0440 deploy/secrets/access-token
sudo chown root:10001 deploy/secrets deploy/secrets/access-token
sudo chmod 0750 deploy/secrets
sudo chown 10001:10001 deploy/data
```

Der Token ist ein Installationsgeheimnis. Sicher verwahren und nicht in Git
einchecken. Die Gruppe 10001 kann das Secret durchqueren und lesen. Das
Datenverzeichnis muss für UID/GID 10001 schreibbar sein; `deploy/data` speichert
unter anderem die SQLite-Datenbank und Tailscale-SSH-Hostschlüssel unter
`data/.config/tailscale` dauerhaft.

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
docker compose -f deploy/compose.yaml ps
docker compose -f deploy/compose.yaml logs -f wolui
```

Die Oberfläche ist über `http://localhost:3000` auf dem Docker-Host erreichbar.
Das Beispiel nutzt den expliziten lokalen Entwicklungsmodus. Für eine öffentliche
Hub-Instanz und weitere Docker- oder Dokploy-Standorte lies die
[WOL-Connect-Betriebsanleitung](../docs/connect-betrieb.md). Das Image startet
standardmäßig ohne `--allow-loopback-dev`; setze bei einer öffentlichen Instanz
`WOLUI_LISTEN=0.0.0.0:3000` und TLS am Reverse Proxy.

Der Container läuft als UID/GID 10001, mit schreibgeschütztem Root-Dateisystem,
ohne zusätzliche Linux-Capabilities und mit `no-new-privileges`. Schreibzugriff
gibt es nur auf `/data` und das temporäre `/tmp`. Das Compose-Healthcheck erwartet
`GET /health/ready`; der Server muss außerdem `GET /health/live` bereitstellen.

## Aktualisieren und sichern

Nach Änderungen am lokalen Quellstand:

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

Für eine einfache Sicherung mit gestopptem Server:

```sh
docker compose -f deploy/compose.yaml stop wolui
tar -C deploy -czf wolui-data-backup.tgz data
docker compose -f deploy/compose.yaml start wolui
```

Zum Wiederherstellen zuerst den Container stoppen, dann das Archiv nach
`deploy/data` entpacken und den Container erneut starten. Vor einem Upgrade
Image und Daten gemeinsam aufbewahren, damit ein Rollback mit passendem
Datenstand möglich bleibt. Ein produktiver Backup-/Restore-Probeablauf ist noch
nicht eingerichtet.

## Entwicklungsnotizen

Das Image wird aus dem Repository-Root gebaut. Es erwartet `package.json`,
`bun.lock`, `apps/web`, ein Cargo-Workspace und das Paket `wol-server`.
GitHub Actions prüft Web und Rust sowie den Start eines Linux/amd64-Containers,
die ausgelieferte Oberfläche und die Anmeldung. Nach einem erfolgreichen
Build auf `main` schiebt der Workflow das getestete Image als
`ghcr.io/jackatdjl/wolui:sha-<commit>` und `ghcr.io/jackatdjl/wolui:latest`
in die GitHub Container Registry.
Das Compose-Beispiel baut lokal aus dem Checkout. ARM64-Images sind ein weiterer
Meilenstein.
