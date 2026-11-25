Tapa 1.0.0 ist die erste Version als Disk-Image: Öffne `Tapa-1.0.0.dmg` und ziehe Tapa auf den Ordner „Programme“. Tapa 0.1.0 aktualisiert sich selbst darauf, und Homebrew installiert dasselbe Image.

## Neu
- Schließt du das letzte Fenster von Tapa, beendet sich Tapa und behält alles: Arbeitsbereiche, Tabs, Teilungen, Verlauf und Agenten-Sitzungen kommen beim nächsten Start so zurück, wie du sie verlassen hast. Tapa fragt vorher nur, wenn noch ein anderes Programm als eine Shell läuft.
- Ziehe Arbeitsbereiche in der Seitenleiste in eine beliebige Reihenfolge; das Terminal bleibt immer oben.
- Ein neues Fenster öffnet sich mit der Größe und Seitenleistenbreite des Fensters, dessen Größe du zuletzt geändert hast, auch wenn Tapa keine Fenster wiederherstellt, und Tapa merkt sich, ob das Eingabefeld eingeblendet ist.
- Startet Tapa mit eingeklappter Seitenleiste, bleibt die Schaltfläche **Neuer Arbeitsbereich** ausgeblendet.
- Ausgabe, die ein Programm schubweise schreibt, erscheint, sobald das Programm pausiert, spätestens 8 ms später, sodass halb gezeichnete Bildschirme nicht mehr aufblitzen.
- Jede Sitzung läuft auf dem eigenen Terminal-Kern von Tapa; die Einstellung, die auf den Kern von Alacritty umschaltete, entfällt.
- Ordner und Branch über dem Eingabefeld sind schlichter grauer Text, bündig mit dem Platzhalter.
- In einem Agenten-Tab listet /resume die früheren Sitzungen des Agenten in diesem Ordner auf und setzt eine in einem eigenen Tab fort.
- Eine lange Liste im Overlay eines Agenten-Tabs scrollt, statt das Fenster höher zu machen.
- Ein Agent gilt erst nach drei Sekunden Leerlauf als fertig, sodass sein Status zwischen den Schritten nicht mehr flackert.
- Im Vollbild bleibt die Seitenleiste unter der Symbolleiste und rutscht mit ihr nach unten, wenn die Menüleiste erscheint.

## Tapa auf einen Blick
- Arbeitsbereiche in einer Seitenleiste, jeder mit seinen Tabs, Teilungen, Agenten, Editoren und Browser-Tabs; Fenster und Agenten-Sitzungen kommen nach einem Neustart zurück.
- Agenten-Tabs für Claude, Codex, OpenCode und jeden ACP-Agenten aus dem Register, die du in den Einstellungen installierst, aktualisierst und auf die vorige Version zurücksetzt.
- Panels für Dateien und Git, Vorschauen für Markdown und Bilder, ein Eingabefeld für Befehle mit Diktat.
- Helles, dunkles oder System-Erscheinungsbild, vier Schriftgrößen, 35 Sprachen der Oberfläche.
