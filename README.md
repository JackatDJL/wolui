<img src="assets/icon.svg" width="64" height="64" alt="">

# wolui

Created in [T3 Code](https://t3.codes).

WOL UI ist als selbst gehostete Oberfläche für Wake-on-LAN über mehrere Standorte geplant.

Der lokale Einstieg für Linux steht in der [Docker-Anleitung](deploy/README.md). Ein lokales Netzwerkprofil legt fest, über welche Host-IP und Broadcast-Adresse das Magic Packet gesendet wird.

Die [Architektur und der Umsetzungsplan](docs/rfcs/README.md) beschreiben den Rust-Server, WOL Connect, Web und Expo sowie den Docker-Betrieb.
