Tapa 1.0.6 chráni pred Markdownom vytvoreným na vyčerpanie pamäte a ponechá v termináli posledný výstup programu.

## Opravy
- Dokument Markdown so stovkami tisíc prázdnych buniek tabuľky, v náhľade alebo v odpovedi agenta, už nezaberie sekundy a gigabajty pamäte: nad 500 000 prázdnych buniek sa jeho tabuľky zobrazia ako text.
- Terminál už nezmešká koniec svojho programu ani nestratí jeho posledný výstup, keď je Mac vyťažený.

## Vylepšenia
- Priebeh agenta vynecháva hooky, ktoré prebehli v poriadku alebo ešte bežia; hook, ktorý zlyhal, sa zobrazuje ďalej.
