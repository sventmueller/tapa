Tapa 1.0.3 öffnet Arbeitsbereiche auf anderen Rechnern über SSH und schlägt beim Tippen Befehle vor.

## Neu
- Entfernte Arbeitsbereiche: „Neuer Arbeitsbereich“ bietet einen Ordner auf einem anderen Rechner an: einen Host aus `~/.ssh/config` oder eine Adresse, bei Bedarf Port, Benutzer und Passwort, und den Ordner dort. Terminals und die dort installierten Agenten laufen über eine SSH-Verbindung auf diesem Rechner. Ein Passwort kann im Schlüsselbund bleiben, und ein unbekannter Host-Schlüssel wird nie ohne Bestätigung angenommen. Dateiliste, Git-Panel, Editor und Vorschauen gibt es dort nicht.
- Die Seitenleiste zeigt die Verbindung eines entfernten Arbeitsbereichs mit „Neu verbinden“, „Trennen“ und „Verbindung bearbeiten“. Kommt eine verlorene Verbindung zurück, starten die Terminals neu und Agent-Tabs setzen ihre Unterhaltung fort.
- Vorschläge beim Tippen: Ab dem zweiten Zeichen erscheint der Rest eines vorgeschlagenen Befehls blass hinter dem Cursor, → übernimmt ihn. Tab übernimmt einen sicheren Vorschlag oder öffnet ein Vervollständigungsmenü, das jeden Kandidaten beschreibt, mit und ohne Eingabefeld.
- Befehlsfarben: Befehle, Unterbefehle, Optionen, Variablen und Operatoren erhalten die Farben des Themas, und ein Befehl, den die Shell nicht ausführen kann, wird rot gestrichelt unterstrichen. „Einstellungen › Eingabe“ schaltet Vorschläge und Farben ab.
- In einem Agent-Tab öffnen die Schaltflächen für Subagenten und Hintergrundprozesse sowie /tasks eine Aktivitätsansicht mit Status und Laufzeit jedes Eintrags, mit „Zeigen“ und „Stoppen“.

## Verbesserungen
- „Einstellungen › Allgemein › Fenster“ kann das Umbenennen eines Arbeitsbereichs per Klick auf seinen Namen abschalten.
- Die Seitenleiste zeigt die Zahl der geänderten Dateien, die Git-Schaltfläche und die Ordner-Schaltfläche in dieser Reihenfolge, die Git-Schaltfläche nur in einem Git-Repository.
