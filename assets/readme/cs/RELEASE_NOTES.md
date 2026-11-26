Tapa 1.0.6 chrání před Markdownem, který má vyčerpat paměť, a ponechá v terminálu poslední výstup programu.

## Opravy
- Dokument Markdown se stovkami tisíc prázdných buněk tabulky, v náhledu nebo v odpovědi agenta, už nezabere sekundy a gigabajty paměti: nad 500 000 prázdných buněk se jeho tabulky zobrazí jako text.
- Terminál už nepropásne konec svého programu ani neztratí jeho poslední výstup, když je Mac vytížený.

## Vylepšení
- Průběh agenta vynechává hooky, které proběhly v pořádku nebo ještě běží; hook, který selhal, se zobrazuje dál.
