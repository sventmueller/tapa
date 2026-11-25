Tapa 1.0.0 è la prima release distribuita come immagine disco: apri `Tapa-1.0.0.dmg` e trascina Tapa nella cartella Applicazioni. Tapa 0.1.0 si aggiorna da solo a questa versione, e Homebrew installa la stessa immagine.

## Novità
- Chiudere l’ultima finestra di Tapa chiude Tapa e conserva tutto: gli spazi di lavoro, i pannelli, le divisioni, la cronologia e le sessioni degli agenti tornano al prossimo avvio come li hai lasciati. Tapa chiede prima solo se è ancora in esecuzione un programma diverso da una shell.
- Trascina gli spazi di lavoro nella barra laterale nell’ordine che preferisci; il Terminale resta sempre in cima.
- Una nuova finestra si apre con le dimensioni e la larghezza della barra laterale dell’ultima finestra che hai ridimensionato, anche quando Tapa non ripristina alcuna finestra, e Tapa ricorda se il campo di input è visibile.
- Quando Tapa si avvia con la barra laterale chiusa, il pulsante Nuovo spazio di lavoro resta nascosto.
- L’output che un programma scrive a raffica compare quando il programma fa una pausa, al massimo 8 ms dopo, così le schermate disegnate a metà non lampeggiano più.
- Ogni sessione viene eseguita sul core di terminale di Tapa; l’impostazione che passava al core di Alacritty non esiste più.
- La cartella e il ramo sopra il campo di input sono semplice testo grigio, allineato al segnaposto.
- In un pannello agente, /resume elenca le sessioni passate dell’agente in questa cartella e ne continua una in un pannello a sé.
- Un lungo elenco nell’overlay di un pannello agente scorre invece di rendere la finestra più alta.
- Un agente conta come terminato solo dopo tre secondi di inattività, così il suo stato non sfarfalla più tra un passaggio e l’altro.
- A tutto schermo la barra laterale resta sotto la barra strumenti e scende con essa quando compare la barra dei menu.

## Tapa in breve
- Spazi di lavoro in una barra laterale, ognuno con i suoi pannelli, divisioni, agenti, editor e pannelli del browser; finestre e sessioni degli agenti tornano dopo un riavvio.
- Pannelli agente per Claude, Codex, OpenCode e qualsiasi agente ACP del registro, da installare, aggiornare e riportare alla versione precedente dalle impostazioni.
- Pannelli File e Git, anteprime di Markdown e immagini, un campo di input per i comandi con dettatura.
- Aspetto chiaro, scuro o di sistema, quattro dimensioni del carattere, 35 lingue dell’interfaccia.
