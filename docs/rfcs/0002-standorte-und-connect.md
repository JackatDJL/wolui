# RFC 0002 – Standorte und WOL Connect

Status: Entwurf

## Topologie und Vertrauensgrenzen

Falkenstein ist der zentrale Hub. Zuhause und Schule verwenden dasselbe Image mit eigener Installation, eigenem Gerätebestand und lokalem Zugang. Jede Standortinstanz baut eine ausgehende WSS-Verbindung zum Hub auf. NAT und wechselnde öffentliche IP-Adressen an den Standorten sind damit im Referenzaufbau unerheblich. Alternativ läuft derselbe Hub über eine per Tailnet erreichbare HTTPS-Adresse. Der Verbindungsaufbau hängt von der Erlaubnis für den jeweiligen ausgehenden Zugang ab.

Ein Standort entscheidet bei der Kopplung, welche Berechtigungen der Hub erhält. Standard sind Inventar lesen und wecken. Inventar bearbeiten ist eine ausdrückliche Freigabe. Netzprofile, lokale Administratoren und weitere Links verwaltet der Standort vor Ort. Auch eine kompromittierte Hub-Sitzung bleibt auf die erteilten Standortrechte begrenzt. Eine Standortidentität am Hub kann nur ihren eigenen Standort aktualisieren. Standorte sind am Hub sichtbar und können den Hub nicht als Transitverbindung zu anderen Standorten verwenden.

Das T3-Vorbild liefert die Bedienidee: Ein Standort erscheint mit eigenem Namen, Verbindungszustand und widerrufbarem Zugang. WOL Connect besteht aus dem Pairing und einem eng begrenzten Nachrichtenkanal, betrieben in der eigenen Installation. Eine einzelne lokale Instanz kann denselben Web- und Mobilablauf ohne Kopplung anbieten.

## Kopplung

1. Der Hub-Admin legt einen Standortplatz mit Anzeigename und maximalen Rechten an. Der Hub erzeugt eine zufällige Einmal-Einladung, speichert ihren Digest und zeigt eine kopierbare Einladung mit öffentlicher Hub-URL. Gültigkeit als Startwert zehn Minuten.
2. Der lokale Admin trägt die Einladung in der lokalen Oberfläche ein oder übergibt sie über eine Datei an den CLI-Befehl. Er prüft den Hub-Namen und bestätigt die Rechte. Der Standort fordert eine Kopplung über TLS an.
3. Der Hub reserviert die Einladung transaktional für diese Installation. Beide Oberflächen zeigen denselben Vergleichscode. Der Hub-Admin bestätigt den wartenden Standort. Der Hub erteilt eine widerrufbare Standortkennung mit zufälligem Bearer-Secret. Der Standort persistiert sie mit eingeschränktem Dateizugriff.
4. Der Standort öffnet den Kanal und meldet unterstützte Protokollversionen sowie die lokal gewährten Berechtigungen. Ein Verbindungsabbruch vor der Aktivierung zeigt einen offenen oder abgelaufenen Pairingzustand. Falls die Ausgabe des Secrets verloren gegangen ist, widerruft der Admin die Reservierung und erstellt eine neue Einladung.

Secrets werden weder in URL-Query-Parametern noch in Shell-Argumenten übergeben. Der Client folgt bei der Kopplung und auf dem WSS-Kanal keinen Redirects. Der Hub speichert den Credential-Digest. Standorte können den Hub lokal entkoppeln, laufende Verbindungen beenden und das Secret löschen. Hub-Widerruf beendet den Kanal und blockiert erneute Verbindungen. Rotation ist durch Ersetzen der Kopplung möglich, später über einen eigenen Ablauf mit kurzer Überlappung.

## Nachrichten und Ausfallverhalten

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

- Zwei Standortinstanzen koppeln sich aus getrennten Netzen und können über eine Hub-Sitzung unabhängig bedient werden.
- Dieselbe MAC in beiden Standorten bleibt getrennt adressierbar. Wake-Pakete werden am richtigen Standort beobachtet.
- Abgelaufene und verbrauchte Einladungen schlagen fehl, widerrufene Kanäle schließen sich und ein read-only Link kann kein Wake auslösen.
- Abbruch vor und nach Dispatch, Hub- und Standortneustart, alter Client und Wiedereinwahl werden geprüft. Offline-Aufträge lösen nach Reconnect nichts aus.
- Offene Entscheidung nach Praxistest: maximale Inventargröße und Größenrahmen für Snapshots.

[T3 Code – Remote Access](https://github.com/pingdotgg/t3code/blob/main/docs/user/remote-access.md)
