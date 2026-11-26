Tapa 1.0.6 protegeix contra Markdown pensat per esgotar la memòria i conserva l'última sortida d'un programa al terminal.

## Correccions
- Un document Markdown amb centenars de milers de cel·les de taula buides, en una previsualització o en la resposta d'un agent, ja no costa segons i gigabytes de memòria: a partir de 500.000 cel·les buides, les taules es mostren com a text.
- Un terminal ja no es perd el final del seu programa ni perd l'última sortida del programa quan el Mac està ocupat.

## Millores
- L'historial de l'agent omet els hooks que s'han executat bé o que encara s'executen; un hook que ha fallat encara es mostra.
