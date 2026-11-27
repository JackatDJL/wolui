<img src="assets/icon.svg" width="64" height="64" alt="">

# wolui

Created in [T3 Code](https://t3.codes).

WOL UI ist eine selbst gehostete Oberfläche für Wake-on-LAN über mehrere Standorte.

Der lokale Einstieg für Linux steht in der [Docker-Anleitung](deploy/README.md). Ein lokales Netzwerkprofil legt fest, über welche Host-IP und Broadcast-Adresse das Magic Packet gesendet wird.

Die [Architektur und der Umsetzungsplan](docs/rfcs/README.md) beschreiben den Rust-Server, WOL Connect, Web und Expo sowie den Docker-Betrieb.

Die ersten Web- und Mobil-Clients sowie die ausgehende Standortkopplung sind implementiert. Einrichten und Betreiben steht unter [WOL Connect](docs/connect-betrieb.md); die [Expo-App](apps/mobile/README.md) unterstützt Anmeldung, Standortwahl und Wake-on-LAN.
