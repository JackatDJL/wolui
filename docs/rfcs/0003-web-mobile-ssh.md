# RFC 0003 – Web, Expo und Browser-SSH

Status: Web-MVP, Expo-Client und Browser-Terminal implementiert. Android-JavaScript-Bundle ist in CI geprüft; native Store-Builds, Accessibility-Abnahme und Live-Tailnet-Test sind offen.

## Web und mobile Bedienung

Web: React, TypeScript und Vite. TanStack Query verwaltet Ladezustände, Mutationen und den Cache je Standort. Wake-Aufträge werden kurzzeitig per Polling aktualisiert. Die aktuelle UI ist eine einzelne App-Ansicht ohne React-Router-Routen. Die Anmeldung übernimmt aus CLIProxyAPI die Idee eines kompakten Zugangs mit Management-Token. Das visuelle Design und die Komponenten entstehen für WOL UI.

Mobil: Expo mit React Native und Expo SecureStore; Expo Router ist noch nicht eingerichtet. Der Einstieg akzeptiert eine HTTPS-Serveradresse und einen Zugangstoken zur Sitzungsausstellung. Die App verwirft den Zugangstoken nach dem Tausch und speichert Sitzung und CSRF-Token sicher. Pairing per QR, serverseitiger Widerruf beim Logout und mehrere gespeicherte Serverprofile sind noch nicht implementiert. Web und Mobil teilen derzeit keinen generierten API-Client. Browser- und Native-Komponenten bleiben plattformspezifisch.

## Bedienabläufe

| Ansicht | Ablauf |
| --- | --- |
| Anmeldung | Token einfügen, verbinden, verständliche Rückmeldung bei falschem Token, Timeout und Zertifikatsfehler |
| Startseite | Standort wählen, Gerät finden, „Starten“ drücken, Rückmeldung am Gerät sehen |
| Gerät hinzufügen | Name und MAC eingeben, sichtbares Standort-Netzprofil auswählen, optional Tailscale-Namen hinterlegen |
| Gerätedetails | Letzten Wake-Auftrag mit Zeit und Ergebnis zeigen, Einrichtungshinweise verlinken, SSH anbieten |
| Standorte | Lokale und verknüpfte Instanzen, letzter Kontakt, Rechte, koppeln und widerrufen |
| App | Server wechseln, Geräte starten und SSH über denselben Server im externen Browser öffnen |

Beim Start bleibt der Klick am Gerät. Während der Anfrage ist der Button deaktiviert. Nach Antwort erscheint je nach Zustand „Wird gesendet“, „Paket gesendet“, „Versand fehlgeschlagen“ oder „Versand unklar“. Der Fokus bleibt am Bedienelement, Statusmeldungen werden per Live-Region beziehungsweise über die native Accessibility-API zugänglich gemacht. Für erneute Auslösung nach einem unklaren Ergebnis zeigt die Oberfläche die bereits erfolgte Anfrage und verlangt einen bewussten erneuten Klick nach Ablauf des Cooldowns.

Der Web-Dialog enthält eine allgemeine Anleitung für Windows, Linux, Mac, NAS und Mini-PCs sowie eine Checkliste für MAC-Adresse, Netzwerkprofil, Stromversorgung und lokalen Test. Hersteller- und modellbezogene Anleitungen, eine eigene macOS-Anleitung und Hardware-Tests pro Schlafzustand bleiben offen.

## Tailscale und Browser-SSH

**Erste Stufe.** Ein optional konfiguriertes Gerät zeigt „Tailscale öffnen“ sowie den gespeicherten Gerätenamen zum Kopieren. Der Einstieg führt zur dokumentierten Machines-Ansicht. SSH-Berechtigungen und Anmeldung bleiben beim Tailnet. Die offizielle Browser-Konsole startet aus der Tailscale-Adminoberfläche, verlangt eine unterstützte Adminrolle, passende Tailnet-Regeln und erneute Authentifizierung. Der Einstieg erfüllt deshalb das Browser-SSH-Ziel mit mehreren Schritten. Ein integriertes Terminal ist ein eigener Meilenstein.

**Zweite Stufe implementiert; Live-Tailnet-Abnahme offen.** Ein optionales Terminal mit xterm.js im Browser verbindet sich per same-origin WSS mit dem Axum-Server. Der Rust-Server startet in einem PTY den Tailscale-CLI-Befehl `tailscale ssh user@host`. Das nutzt den Tailscale-Daemon am Gateway und dessen Host-Key-Prüfung; ein russh-Client oder eigener SSH-Schlüssel wird nicht eingesetzt. Der Browser sieht nur die Terminaldaten der laufenden Sitzung. Bei einem Gateway sieht das Tailnet die Gateway-Identität. Die Web-Sitzung übernimmt daher keine Tailscale-Benutzeridentität.

Das Gateway startet ausschließlich Sitzungen zu vorab freigegebenen Geräteprofilen mit validiertem Tailscale-Ziel und expliziter Benutzerzuordnung. SSH wird pro Gerät separat vom Wake-Recht erteilt. Die Funktion bleibt aus, bis `WOLUI_SSH_ENABLED=true` gesetzt ist und der Server Zugriff auf den konfigurierten Tailscale-Socket hat. Der WebSocket erfordert eine gültige Web-Sitzung und exakt dieselbe `WOLUI_PUBLIC_URL`-Origin. Pro Instanz sind vier gleichzeitige Sitzungen und 15 Minuten Leerlauf erlaubt. Das Audit speichert Sitzungs-ID, Standort, Gerät, Beginn, Ende und Ergebnis; Terminalinhalte und Authentifizierungsdaten werden nicht mitgeschrieben. Der Wake-Kanal transportiert keine Terminaldaten. Geräte ohne Gateway-Erreichbarkeit nutzen weiter den Tailscale-Einstieg. Ob ein Gateway in der Schule sinnvoll oder zulässig ist, hängt von den dortigen Regeln ab.

## Abnahme

- Web und Expo starten dasselbe Gerät über dieselbe Hub-API. Mobile Nutzer können Heimatserver wechseln, ohne dass Ergebnisse eines anderen Servers sichtbar bleiben.
- Tastatur, Screenreader, kleine Displays und langsame Verbindungen sind im Login-, Hinzufüge- und Wake-Ablauf geprüft.
- Bei fehlenden Tailscale-Rechten bleibt der Wake-Ablauf voll benutzbar. Der WebSocket-/PTY-Pfad und Clients bauen erfolgreich. Vor einem öffentlichen Rollout wird ein realer Tailscale-SSH-Login gegen ein erlaubtes Zielgerät geprüft; das benötigte Tailnet steht in der Entwicklungsumgebung nicht zur Verfügung.

[CLIProxyAPI Management Center](https://github.com/router-for-me/Cli-Proxy-API-Management-Center) · [Expo-Monorepos](https://docs.expo.dev/guides/monorepos/) · [Tailscale SSH Console](https://tailscale.com/docs/features/tailscale-ssh/tailscale-ssh-console)
