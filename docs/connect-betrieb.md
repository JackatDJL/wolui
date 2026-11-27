# WOL Connect betreiben

WOL Connect verbindet einen Hub mit lokalen Standortinstanzen. Der Hub braucht eine öffentliche HTTPS-Adresse. Die Standortserver öffnen selbst eine ausgehende Verbindung zum Hub; für das Heimnetz ist kein eingehender Port nötig. Jeder Standort braucht ein eigenes persistentes Datenvolume und sein Netzwerkprofil für das lokale LAN.

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

Lege in Dokploy keine `latest`-Tags als einzige Release-Referenz fest, wenn du kontrollierte Rollbacks brauchst. Das CI veröffentlicht `ghcr.io/jackatdjl/wolui:latest` und `ghcr.io/jackatdjl/wolui:sha-<commit>`; verwende den SHA-Tag für eine reproduzierbare Bereitstellung.

## Verbindung prüfen

- Hub: `GET /health/ready` muss `200` liefern.
- Web: Login muss Sitzung und CSRF-Token ausgeben.
- Hub UI: Site erscheint nach dem ersten Snapshot als online.
- Wake: Remote-Auftrag endet mit `sent`, `failed` oder `unknown`; bei `unknown` erst den Standort und das Gerät prüfen, bevor du erneut startest.

Die aktuelle MVP-Variante nutzt HTTPS-Polling in Fünf-Sekunden-Intervallen. Sie implementiert weder WSS noch ein integriertes Terminal. Die Grenzen und geplanten Erweiterungen stehen in [RFC 0002](rfcs/0002-standorte-und-connect.md) und [RFC 0003](rfcs/0003-web-mobile-ssh.md).
