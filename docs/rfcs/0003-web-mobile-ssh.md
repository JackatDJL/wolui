# RFC 0003 – Web, Expo und Browser-SSH

Status: Entwurf

## Web und mobile Bedienung

Web: React, TypeScript und Vite. TanStack Query verwaltet Ladezustände, Mutation und Cache je Server und Standort. Das erste Release aktualisiert sichtbare Wake-Aufträge kurzzeitig per Polling und hält Aktualisierungen für inaktive Tabs an. React Router bildet die wenigen direkten Seitenadressen ab. Die Anmeldung übernimmt aus CLIProxyAPI die Idee eines kompakten Zugangs mit Management-Token. Das visuelle Design und die Komponenten entstehen für WOL UI.

Mobil: Expo mit React Native, Expo Router und Expo SecureStore. Der Einstieg akzeptiert eine HTTPS-Serveradresse und einen Zugangstoken zur Sitzungsausstellung. Alternativ scannt die App einen einmaligen, kurzlebigen Einladungs-QR mit Serveradresse und Einlösecode. Der Einrichtungs-Token wird nach dem Tausch verworfen. Der App-Token wird pro Server sicher gespeichert und bei Abmeldung widerrufen, soweit der Server erreichbar ist. Eine Sitzung kann administrativ auch vom Server aus entfernt werden. Serverwechsel wechselt den Daten-Cache und die Sitzung gemeinsam. QR-Links nehmen keine Wake-Aktion vor. Beide Clients teilen generierte Typen, API-Client und Fehlertexte. Browser- und Native-Komponenten bleiben plattformspezifisch.

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

Geräteanleitungen werden nach Betriebssystem und Herstellermerkmalen aufgebaut und aus dem Formular verlinkt. Windows erklärt UEFI-, Adapter- und Energieeinstellungen, Linux erklärt die Prüfung des Adapters und Persistenz über die verwendete Netzwerkverwaltung, NAS und Mini-PCs erhalten Hinweise zu den Herstelleroptionen. macOS erhält eine eigene Anleitung mit Bedingungen für unterstützte Schlafzustände. Die Checkliste beginnt bei kabelgebundenem Netzwerk, verbleibender Stromversorgung, richtigem Interface und MAC-Adresse. Für Ruhemodus, Hibernate, vollständiges Ausschalten und WLAN wird die Unterstützung je Gerät ausdrücklich getestet und dokumentiert. Die Anleitung endet mit einem lokalen Wecktest am tatsächlichen Standort.

## Tailscale und Browser-SSH

**Erste Stufe.** Ein optional konfiguriertes Gerät zeigt „Tailscale öffnen“ sowie den gespeicherten Gerätenamen zum Kopieren. Der Einstieg führt zur dokumentierten Machines-Ansicht. SSH-Berechtigungen und Anmeldung bleiben beim Tailnet. Die offizielle Browser-Konsole startet aus der Tailscale-Adminoberfläche, verlangt eine unterstützte Adminrolle, passende Tailnet-Regeln und erneute Authentifizierung. Der Einstieg erfüllt deshalb das Browser-SSH-Ziel mit mehreren Schritten. Ein integriertes Terminal ist ein eigener Meilenstein.

**Zweite Stufe, Machbarkeitsprüfung vor Freigabe.** Ein optionales Terminal mit xterm.js im Browser verbindet sich per authentifiziertem WSS mit einem Rust-SSH-Gateway. Ein Prototyp prüft zuerst russh gegen das geplante Zielsystem und die Nutzung eines lokalen Tailscale-Zugangs am Gateway. Die Verbindung verwendet entweder OpenSSH über das Tailnet mit eigener Benutzeranmeldung oder, nach einem gesonderten Test, das Tailscale-SSH-Identitätsmodell. Bei einem Gateway sieht das Tailnet die Gateway-Identität. Die Web-Sitzung übernimmt daher keine Tailscale-Benutzeridentität. Eine erfolgreiche OpenSSH-Verbindung über das Tailnet beweist keine Unterstützung von Tailscale SSH.

Das Gateway startet ausschließlich Sitzungen zu vorab freigegebenen Geräteprofilen mit definierter Zielidentität und Benutzerzuordnung. Netzwerk- und SSH-Zugriff werden pro Gerät separat vom Wake-Recht erteilt. Die Implementierung prüft Hostkeys und verwendet einen eigenen sitzungsbezogenen SSH-Zugang. Eine passende Authentifizierung ohne zentralen dauerhaften Root-Schlüssel ist Voraussetzung. Das Terminal protokolliert Metadaten wie Beginn, Dauer, Ziel und Abbruch. Terminalinhalte und Authentifizierungsdaten werden nicht mitgeschrieben. Zeitgrenzen, Backpressure, Größenlimits, Origin-Prüfung und Entzug aktiver Sitzungen gehören in die Abnahme. Der Wake-Kanal transportiert keine Terminaldaten. Geräte ohne Gateway-Erreichbarkeit nutzen weiter den Tailscale-Einstieg. Ob ein Gateway in der Schule sinnvoll oder zulässig ist, hängt von den dortigen Regeln ab.

## Abnahme

- Web und Expo starten dasselbe Gerät über dieselbe Hub-API. Mobile Nutzer können Heimatserver wechseln, ohne dass Ergebnisse eines anderen Servers sichtbar bleiben.
- Tastatur, Screenreader, kleine Displays und langsame Verbindungen sind im Login-, Hinzufüge- und Wake-Ablauf geprüft.
- Bei fehlenden Tailscale-Rechten bleibt der Wake-Ablauf voll benutzbar. Die Oberfläche meldet keine erfolgreiche SSH-Verbindung, solange sie keine kennt.
- Terminal gilt erst als fertig, wenn ein normaler Klick aus einem berechtigten WOL-Geräteprofil eine Browser-Sitzung zu einem realen, über das Tailnet erreichbaren Testziel öffnet.

[CLIProxyAPI Management Center](https://github.com/router-for-me/Cli-Proxy-API-Management-Center) · [Expo-Monorepos](https://docs.expo.dev/guides/monorepos/) · [Tailscale SSH Console](https://tailscale.com/docs/features/tailscale-ssh/tailscale-ssh-console)
