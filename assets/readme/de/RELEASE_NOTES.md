Tapa 1.0.6 schützt vor Markdown, das den Speicher erschöpfen soll, und behält die letzte Ausgabe eines Programms im Terminal.

## Behoben
- Ein Markdown-Dokument mit Hunderttausenden leeren Tabellenzellen, in einer Vorschau oder in der Antwort eines Agenten, kostet nicht mehr Sekunden und Gigabytes an Speicher: Ab 500.000 leeren Zellen erscheinen seine Tabellen als Text.
- Ein Terminal verpasst das Ende seines Programms nicht mehr und verliert dessen letzte Ausgabe nicht, wenn der Mac ausgelastet ist.

## Verbesserungen
- Der Agent-Verlauf lässt Hooks weg, die fehlerfrei liefen oder noch laufen; ein fehlgeschlagener Hook erscheint weiterhin.
