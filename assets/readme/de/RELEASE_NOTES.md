Tapa 1.0.2 verbraucht weniger Speicher, wenn Tabs im Hintergrund liegen.

## Verbesserungen
- Ausgeblendete Terminal-Tabs geben ihre Metal-Puffer frei. Ausgeblendete Agent-Tabs geben die Darstellung ihrer Verläufe frei. Terminalsitzungen, Scrollback und Unterhaltungen bleiben erhalten.
- Tab-Wechsel, Scrollen und Größenänderungen belasten den Hauptthread weniger. Auch beim Wiederherstellen eines Arbeitsbereichs bleibt die Oberfläche reaktionsfähig.
- Claude und Codex verarbeiten wartende Anfragen und Antworten zuverlässiger.
