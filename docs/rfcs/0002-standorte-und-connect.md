# RFC 0002 – Standorte und WOL Connect

Status: MVP implementiert mit HTTPS-Polling. Rollenrechte, ein zweiter Pairing-Schritt und der beschriebene WSS-Kanal bleiben geplant.

## Implementierter MVP

Der Hub erstellt im Management-Interface ein zehn Minuten gültiges Einmal-Token. Der Standort tauscht es über einen ausgehenden HTTPS-Aufruf gegen ein zufälliges Bearer-Credential, schreibt das Credential atomar mit Modus `0600` in das persistente Datenvolume und löscht die Einladungsdatei. Der Hub speichert nur den Digest. Der Hub-Admin hat mit dem Erstellen der Einladung die Kopplung freigegeben; ein zusätzlicher Vergleichscode und eine zweite Bestätigung sind im MVP noch nicht vorhanden.

Der Standort lädt Inventar-Snapshots und fragt alle fünf Sekunden nach genau einem Wake-Auftrag. Ein Auftrag wird vor Ausgabe am Hub atomar auf `accepted` gesetzt und läuft nach 30 Sekunden ab. Der Standort nutzt seinen lokalen Wake-Dispatcher und dedupliziert anhand der Hub-Auftrags-ID. Nach Ablauf oder verlorenem Ergebnis zeigt der Hub `unknown` und sendet den Auftrag nicht erneut. Snapshots enthalten Gerätenamen, MAC-Adresse, Profilname, Revision und optional den Tailscale-Namen. Remote-Inventar ist am Hub schreibgeschützt.

MVP-Endpunkte: `POST /api/v1/connect/invitations`, `POST /api/v1/connect/enroll`, `POST /api/v1/connect/snapshot`, `GET /api/v1/connect/commands/next`, `POST /api/v1/connect/commands/{id}/result`, `GET /api/v1/connect/sites` und `DELETE /api/v1/connect/sites/{id}`. Das Web- und Mobil-Login gibt eine zwölf Stunden gültige Sitzung zusätzlich als `session_token` zurück; mobile Clients halten Sitzung und CSRF-Token im SecureStore und senden Schreibanforderungen weiter mit `X-CSRF-Token`.

Transport-Sicherheit: Zieladressen des Connectors müssen HTTPS verwenden. Für lokale Entwicklung ist HTTP auf `localhost` zugelassen. Redirects werden abgelehnt. Das MVP verwendet fünfsekündiges HTTPS-Polling statt WebSockets und sendet noch keine Rollen- oder Berechtigungsstaffelung innerhalb einer Site-Verbindung. Eine gekoppelte Site kann ihr eigenes Inventar aktualisieren und Wake-Aufträge empfangen. Inventar bearbeiten und Transit über andere Sites werden nicht angeboten. Snapshots sind auf 10.000 Geräte, 256 Profile und 16 MiB begrenzt.

## Topologie und Vertrauensgrenzen

Falkenstein ist der zentrale Hub. Zuhause und Schule verwenden dasselbe Image mit eigener Installation, eigenem Gerätebestand und lokalem Zugang. Jede Standortinstanz baut ausgehende HTTPS-Anfragen zum Hub auf und fragt Inventar- und Wake-Aufträge im Fünf-Sekunden-Takt ab. NAT und wechselnde öffentliche IP-Adressen an den Standorten sind damit im Referenzaufbau unerheblich. Alternativ läuft derselbe Hub über eine per Tailnet erreichbare HTTPS-Adresse. Der Verbindungsaufbau hängt von der Erlaubnis für den jeweiligen ausgehenden Zugang ab.

Im MVP ist die Hub-Freigabe auf Inventar-Synchronisierung und Wake für genau die gekoppelte Site festgelegt. Eine wählbare Rechtevergabe und Standortverwaltung vor Ort sind noch nicht implementiert. Eine Site-Identität am Hub kann nur ihr eigenes Inventar aktualisieren; das Hub-Inventar ist schreibgeschützt. Sites können den Hub nicht als Transitverbindung zu anderen Sites verwenden.

Das T3-Vorbild liefert die Bedienidee: Ein Standort erscheint mit eigenem Namen, Verbindungszustand und widerrufbarem Zugang. WOL Connect besteht aus dem Pairing und einem eng begrenzten Nachrichtenkanal, betrieben in der eigenen Installation. Eine einzelne lokale Instanz kann denselben Web- und Mobilablauf ohne Kopplung anbieten.

## Erweiterter Pairing-Zielablauf (noch nicht MVP)

1. Der Hub-Admin legt einen Standortplatz mit Anzeigename und maximalen Rechten an. Der Hub erzeugt eine zufällige Einmal-Einladung, speichert ihren Digest und zeigt eine kopierbare Einladung mit öffentlicher Hub-URL. Gültigkeit als Startwert zehn Minuten.
2. Der lokale Admin trägt die Einladung in der lokalen Oberfläche ein oder übergibt sie über eine Datei an den CLI-Befehl. Er prüft den Hub-Namen und bestätigt die Rechte. Der Standort fordert eine Kopplung über TLS an.
3. Der Hub reserviert die Einladung transaktional für diese Installation. Beide Oberflächen zeigen denselben Vergleichscode. Der Hub-Admin bestätigt den wartenden Standort. Der Hub erteilt eine widerrufbare Standortkennung mit zufälligem Bearer-Secret. Der Standort persistiert sie mit eingeschränktem Dateizugriff.
4. Der Standort öffnet den Kanal und meldet unterstützte Protokollversionen sowie die lokal gewährten Berechtigungen. Ein Verbindungsabbruch vor der Aktivierung zeigt einen offenen oder abgelaufenen Pairingzustand. Falls die Ausgabe des Secrets verloren gegangen ist, widerruft der Admin die Reservierung und erstellt eine neue Einladung.

Secrets werden weder in URL-Query-Parametern noch in Shell-Argumenten übergeben. Der Client folgt bei der Kopplung und auf dem HTTPS-Kanal keinen Redirects. Der Hub speichert den Credential-Digest und prüft ihn bei jedem Snapshot, Poll und Ergebnis erneut, sodass ein widerrufenes Credential keine neu gekoppelte Site verändern kann. Hub-Widerruf blockiert weitere Requests. Ein eigenes lokales Unpair-UI und Credential-Rotation mit Überlappung sind noch nicht implementiert.

## Geplanter WSS-Nachrichtenkanal (noch nicht implementiert)

JSON über WebSocket. Nachrichten tragen Typ, Protokollversion, ID und zugehörige Korrelations-ID. Der Hub unterstützt während eines gestaffelten Upgrades nach Möglichkeit die vorherige kompatible Version. Bei inkompatibler Version zeigt er einen Upgrade-Hinweis. Die Verbindung hat einen Größenrahmen für Nachrichten, Ping/Pong, begrenzte Warteschlangen und Reconnect mit Jitter und exponentiellem Backoff. Startwerte für den Prototyp sind 20 Sekunden Heartbeat und Offline nach 60 Sekunden ohne Lebenszeichen. Proxy und Client müssen längere Idle-Timeouts unterstützen.

| Richtung | Nachricht | Verbindliche Bedeutung |
| --- | --- | --- |
| Standort → Hub | `hello` | Versionen, Installation und zugesagte Rechte |
| Standort → Hub | `inventory_snapshot` | Autoritativer Stand mit Revision, bei großen Beständen seitenweise |
| Hub → Standort | `wake_request` | Geräte-ID, Auftrags-ID, Geräterevision und Ablaufzeit |
| Standort → Hub | `wake_result` | Dauerhaftes Ergebnis für eine Auftrags-ID |
| Hub → Standort | `inventory_mutation` | Nur bei explizitem Recht, mit erwarteter Revision |
| Hub → Standort | `wake_status_request` | Bereits verbuchtes Ergebnis einer bekannten Auftrags-ID abfragen |

Der Hub legt den Auftrag vor Versand an. Der Standort leitet ihn an denselben Dispatcher wie lokale Aktionen weiter und führt die Deduplizierung mit der Auftrags-ID durch. Ein doppelter Request darf einen Auftrag nur wiederfinden. Bei Verbindungsverlust nach Übergabe fragt der Hub nach Reconnect den Status ab. Fehler oder Timeout ohne Ergebnis bleiben als unklar sichtbar. Aufträge für Offline-Standorte werden sofort abgelehnt und nicht für später geplant. Bereits übergebene Aufträge laufen nach einer kurzen Deadline ab, vorbehaltlich begrenzter Uhrabweichung. Das verhindert überraschendes Wecken bei Netzrückkehr.

Das Inventar liegt am Standort. Der Hub hält eine Anzeige-Kopie und Zeitstempel des letzten Abgleichs. Wenn ein Standort offline ist, markiert der Hub seine Kopie als veraltet und sperrt Aktionen. Die Kopie enthält nur Felder für die erlaubten Funktionen. Pro Standort gilt eine Verbindungs-Generation, damit eine alte Verbindung nach Reconnect keine neue überschreibt. Audit-Einträge verknüpfen Standort, Gerät, Auftrag und authentifizierten Akteur. Der Standort führt den Hub als lokalen Akteur und behandelt die vom Hub gemeldete Benutzerkennung als Kontext.

## Abnahme

- Ein Integrationstest startet einen Hub-HTTP-Server und einen unabhängigen Site-Connector mit getrennten Datenbanken, koppelt über eine Einmal-Einladung, synchronisiert Inventar und beobachtet das UDP-Wake-Paket am Site-Socket. Ein zusätzlicher Test mit getrennten Hosts/Netzen bleibt offen.
- Dieselbe MAC in beiden Standorten bleibt getrennt adressierbar. Wake-Pakete werden am richtigen Standort beobachtet.
- Abgelaufene und verbrauchte Einladungen schlagen fehl, widerrufene Kanäle schließen sich und ein read-only Link kann kein Wake auslösen.
- Abbruch vor und nach Dispatch, Hub- und Standortneustart, alter Client und Wiedereinwahl werden geprüft. Offline-Aufträge lösen nach Reconnect nichts aus.
- Offene Entscheidung nach Praxistest: maximale Inventargröße und Größenrahmen für Snapshots.

[T3 Code – Remote Access](https://github.com/pingdotgg/t3code/blob/main/docs/user/remote-access.md)
