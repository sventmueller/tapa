Tapa 1.0.3 obre espais de treball en altres ordinadors per SSH i suggereix ordres mentre escrius.

## Novetats
- Espais de treball remots: «Espai de treball nou» ofereix una carpeta en un altre ordinador: un host de `~/.ssh/config` o una adreça, el port, l'usuari i la contrasenya si cal, i la carpeta d'allà. Els terminals i els agents instal·lats allà s'hi executen a través d'una sola connexió SSH. La contrasenya es pot desar al clauer, i una clau de host desconeguda no s'accepta mai sense confirmació. La llista de fitxers, el tauler de Git, l'editor i les previsualitzacions no hi són disponibles.
- La barra lateral mostra la connexió d'un espai de treball remot amb «Torna a connectar», «Desconnecta» i «Edita la connexió». Quan una connexió perduda torna, els terminals es tornen a iniciar i les pestanyes d'agent continuen la conversa.
- Suggeriments mentre escrius: a partir del segon caràcter, la resta d'una ordre suggerida apareix atenuada després del cursor, i → l'accepta. Tab accepta un suggeriment segur o obre un menú de compleció que descriu cada candidat, amb o sense el camp d'entrada.
- Colors de les ordres: les ordres, subordres, opcions, variables i operadors prenen els colors del tema, i una ordre que l'intèrpret no pot executar queda subratllada amb una línia vermella discontínua. «Configuració › Entrada» desactiva els suggeriments i els colors.
- En una pestanya d'agent, els botons de subagents i processos en segon pla i /tasks obren una vista d'activitat amb l'estat i el temps d'execució de cada element, amb «Mostra» i «Atura».

## Millores
- «Configuració › General › Finestra» permet desactivar el canvi de nom d'un espai de treball fent clic al nom.
- La barra lateral mostra el nombre de fitxers canviats, el botó de Git i el botó de carpeta en aquest ordre, i el botó de Git només dins d'un repositori Git.
