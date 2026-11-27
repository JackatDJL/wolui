# WOL Connect betreiben

WOL Connect verbindet einen Hub mit lokalen Standortinstanzen. Der Hub braucht eine öffentliche HTTPS-Adresse. Die Standortserver öffnen selbst eine ausgehende Verbindung zum Hub; für das Heimnetz ist kein eingehender Port nötig. Jeder Standort braucht ein eigenes persistentes Datenvolume und sein Netzwerkprofil für das lokale LAN.

## Browser-Terminal über Tailscale SSH

Der Hub kann ein Terminal im Browser öffnen und `tailscale ssh` als Gateway-Client ausführen. Das gilt für lokale und verbundene Geräte, deren Tailscale-Name aus dem Standortinventar bekannt ist. Dafür muss der Hub den Tailscale-CLI-Prozess und dessen `tailscaled`-Socket erreichen können. Das Image enthält Tailscale CLI und den dafür benötigten OpenSSH-Client; der Tailscale-Daemon und seine Socket-Freigabe werden separat betrieben. Der Hub legt Tailscales geprüfte SSH-Hostschlüssel im persistenten `/data/.config/tailscale` ab.

Aktiviere den Gateway auf dem Hub:

```dotenv
WOLUI_SSH_ENABLED=true
WOLUI_TAILSCALE_BIN=/usr/local/bin/tailscale
WOLUI_TAILSCALE_SOCKET=/var/run/tailscale/tailscaled.sock
WOLUI_PUBLIC_URL=https://wolui.example.com
```

Stelle dem Container den Socket eines angemeldeten Tailscale-Daemons unter dem konfigurierten Pfad bereit. Der WOL UI Prozess läuft als UID `10001`; er braucht sowohl Dateisystemzugriff auf den Socket als auch die Berechtigung, den lokalen Tailscale-Daemon zu bedienen. Bei einer vorhandenen Host-Installation kann das Compose beispielsweise so aussehen; setze die ergänzende Gruppen-ID passend zu deinem Socket:

```yaml
services:
  wolui:
    group_add:
      - "${WOLUI_TAILSCALE_SOCKET_GID}"
    volumes:
      - /var/run/tailscale:/var/run/tailscale:ro
```

Ein eigener Tailscale-Daemon oder Sidecar mit persistentem State ist eine gute Wahl, damit der Hub eine dedizierte Geräteidentität hat. Gib dieser Identität nur die SSH-Zugriffe, die der Hub benötigt. Der lokale Daemon muss den WOL UI Prozess zudem als berechtigten CLI-Operator zulassen, falls dessen LocalAPI das für `tailscale ssh` verlangt. Der Tailscale-Operator ist eine ausdrückliche lokale Berechtigung; folge dafür der [Tailscale-CLI-Dokumentation](https://tailscale.com/docs/reference/tailscale-cli).

Bearbeite das Gerät an seinem Standort, trage den MagicDNS-Namen oder die Tailscale-IP und den Zielbenutzer ein und aktiviere **Tailscale SSH erlauben**. Bei Remote-Geräten wird diese Freigabe mit dem Inventar zum Hub synchronisiert. Die Aktion erscheint nur bei ausdrücklich freigegebenen Geräten und wenn `WOLUI_SSH_ENABLED=true` auf dem Hub aktiv ist. Auf dem Ziel müssen Tailscale SSH und passende SSH-Regeln der Tailnet-Policy eingerichtet sein. Tailscale SSH unterstützt derzeit Linux- und macOS-Ziele. Der [Tailscale-CLI-Client prüft den SSH-Host-Key gegen den vom Tailnet angekündigten Schlüssel](https://tailscale.com/docs/reference/tailscale-cli#ssh).

Die Webanmeldung übernimmt nicht die Identität des eingeloggten Nutzers im Tailnet: Tailscale autorisiert die Verbindung anhand der Gateway-Identität. Der WOL UI Zugangstoken ist derzeit ein gemeinsamer Administratorzugang. Exponiere das öffentliche Interface daher nur den Personen, denen du diese Gateway-Rechte geben willst. Das Audit speichert Geräte-/Standort-ID, Zeit und Ergebnis, aber keine Terminaleingaben oder -ausgaben. Pro Hub sind höchstens vier gleichzeitige Terminals und 15 Minuten Leerlauf pro Sitzung erlaubt.

## Hub einrichten

Setze auf der zentralen Instanz:

```dotenv
WOLUI_ROLE=hub
WOLUI_SITE_NAME=Falkenstein
WOLUI_LISTEN=0.0.0.0:3000
WOLUI_PUBLIC_URL=https://wolui.example.com
WOLUI_DATA_DIR=/data
WOLUI_ACCESS_TOKEN_FILE=/run/secrets/wolui_access_token
```

Das `/data`-Volume muss persistent sein. Veröffentliche den Port nur hinter einem HTTPS-Reverse-Proxy und erlaube den Zugriff auf den Dienst entsprechend deiner Firewall. Das Image startet standardmäßig ohne lokalen Entwicklungsmodus; `WOLUI_LISTEN` darf im öffentlichen Betrieb nicht auf Loopback stehen.

Melde dich mit dem Zugangstoken im Webinterface an und öffne **Standorte**. Gib dem Zielserver einen Namen und erstelle die Einladung. Das zufällige Token gilt zehn Minuten, kann genau einmal eingelöst werden und wird nur bei der Erstellung angezeigt.

## Standort koppeln

Trage auf dem Zielserver den Hub und den Pfad zur Einladungsdatei ein:

```dotenv
WOLUI_ROLE=site
WOLUI_SITE_NAME=Zuhause
WOLUI_CONNECT_URL=https://wolui.example.com
WOLUI_CONNECT_INVITATION_FILE=/run/secrets/wolui-connect-invitation
WOLUI_CONNECT_TOKEN_FILE=/data/connect-token
WOLUI_DATA_DIR=/data
```

Mount die Einladungsdatei als schreibgeschütztes Secret. Bei Docker Compose kann eine lokale Datei beispielsweise so eingebunden werden:

```yaml
services:
  wolui:
    environment:
      WOLUI_CONNECT_URL: https://wolui.example.com
      WOLUI_CONNECT_INVITATION_FILE: /run/secrets/wolui-connect-invitation
      WOLUI_CONNECT_TOKEN_FILE: /data/connect-token
    volumes:
      - ./data:/data
      - ./secrets/connect-invitation:/run/secrets/wolui-connect-invitation:ro
```

Die Secret-Datei muss für UID `10001` lesbar sein, zum Beispiel mit `root:10001` und Modus `0440`. Nach erfolgreicher Kopplung versucht der Standort, die Einladungsdatei zu löschen. Ein schreibgeschütztes Dokploy-Secret bleibt dort eingebunden und muss nach der Kopplung im Dokploy-Setup entfernt werden. Das Widerrufstoken wird mit Modus `0600` im Datenvolume abgelegt. Sichere dieses Volume und gib es nicht an andere Standorte weiter.

Im Standort muss mindestens ein korrektes `WOLUI_NETWORK_PROFILES`-Profil vorhanden sein. Bei Linux Docker Engine funktioniert dafür in der Referenzkonfiguration `network_mode: host`; `bind_address` und `broadcast_address` müssen zum tatsächlichen LAN-Interface passen. Ein falsches Profil kann Inventar anzeigen, aber keine Geräte wecken.

Nach dem Start meldet der Standort etwa alle fünf Sekunden Inventar. Der Hub zeigt danach Verbindung und Geräte. Wenn der Standort offline ist, bleiben Geräte sichtbar, Wake wird blockiert und ein Auftrag wird nicht für später vorgemerkt. Ein Admin kann den Standort in **Standorte → Trennen** widerrufen. Die Site-Credentials bleiben auf dem Standort gespeichert und müssen dort zusätzlich gelöscht werden, wenn die Installation dauerhaft außer Betrieb geht.

## Dokploy

In Dokploy kannst du für jede Instanz dasselbe Repository und dasselbe Dockerfile `deploy/Dockerfile` verwenden. Hub und Sites unterscheiden sich durch ihre Umgebungsvariablen und Netzwerk-/Volume-Konfiguration:

- Für den Hub: `WOLUI_ROLE=hub`, `WOLUI_LISTEN=0.0.0.0:3000`, öffentliche `WOLUI_PUBLIC_URL`, persistentes `/data` und das Zugangstoken-Secret.
- Für eine Site: `WOLUI_ROLE=site`, `WOLUI_SITE_NAME`, `WOLUI_CONNECT_URL`, persistentes `/data`, `WOLUI_NETWORK_PROFILES` und beim ersten Start ein schreibgeschütztes Mount für das Einladungstoken.
- Auf beiden Instanzen muss Dokploy `/data` dauerhaft speichern. Beim Site-Container muss das LAN erreichbar sein; für Docker Engine auf Linux ist dafür Host Networking die vorgesehene Referenz. Reverse Proxy und TLS werden nur am Hub benötigt.
- Für Browser-SSH am Hub zusätzlich `WOLUI_SSH_ENABLED=true` setzen und den `tailscaled`-Socket samt Zugriffsrechten bereitstellen. Ohne diese Option bleibt das Terminal deaktiviert.

Lege in Dokploy keine `latest`-Tags als einzige Release-Referenz fest, wenn du kontrollierte Rollbacks brauchst. Das CI veröffentlicht `ghcr.io/jackatdjl/wolui:latest` und `ghcr.io/jackatdjl/wolui:sha-<commit>`; verwende den SHA-Tag für eine reproduzierbare Bereitstellung.

## Verbindung prüfen

- Hub: `GET /health/ready` muss `200` liefern.
- Web: Login muss Sitzung und CSRF-Token ausgeben.
- Hub UI: Site erscheint nach dem ersten Snapshot als online.
- Wake: Remote-Auftrag endet mit `sent`, `failed` oder `unknown`; bei `unknown` erst den Standort und das Gerät prüfen, bevor du erneut startest.

Die aktuelle MVP-Variante nutzt HTTPS-Polling in Fünf-Sekunden-Intervallen für WOL Connect. Das optionale integrierte Terminal nutzt authentifiziertes WSS und einen lokalen `tailscale ssh`-Prozess am Hub. Die Webanmeldung ist kein Tailnet-Identitätsmapping. Die Grenzen und Designgründe stehen in [RFC 0002](rfcs/0002-standorte-und-connect.md) und [RFC 0003](rfcs/0003-web-mobile-ssh.md).
