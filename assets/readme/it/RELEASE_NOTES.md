Tapa 1.0.3 apre spazi di lavoro su altri computer tramite SSH e suggerisce comandi mentre scrivi.

## Novità
- Spazi di lavoro remoti: «Nuovo spazio di lavoro» offre una cartella su un altro computer: un host da `~/.ssh/config` o un indirizzo, porta, utente e password se servono, e la cartella lì. I terminali e gli agenti installati lì vengono eseguiti su quel computer tramite un'unica connessione SSH. La password può restare nel portachiavi, e una chiave host sconosciuta non viene mai accettata senza conferma. L'elenco dei file, il pannello Git, l'editor e le anteprime lì non sono disponibili.
- La barra laterale mostra la connessione di uno spazio di lavoro remoto con «Riconnetti», «Disconnetti» e «Modifica connessione». Quando una connessione persa ritorna, i terminali ripartono e le schede degli agenti continuano la conversazione.
- Suggerimenti mentre scrivi: dal secondo carattere, il resto di un comando suggerito appare tenue dopo il cursore, e → lo accetta. Tab accetta un suggerimento sicuro o apre un menu di completamento che descrive ogni candidato, con o senza il campo di input.
- Colori dei comandi: comandi, sottocomandi, opzioni, variabili e operatori prendono i colori del tema, e un comando che la shell non può eseguire viene sottolineato con una linea rossa tratteggiata. «Impostazioni › Input» disattiva suggerimenti e colori.
- In una scheda agente, i pulsanti per subagenti e processi in background e /tasks aprono una vista attività con lo stato e il tempo di esecuzione di ogni elemento, con «Mostra» e «Interrompi».

## Miglioramenti
- «Impostazioni › Generali › Finestra» può disattivare la rinomina di uno spazio di lavoro facendo clic sul nome.
- La barra laterale mostra il numero di file modificati, il pulsante Git e il pulsante della cartella in quest'ordine, e il pulsante Git solo all'interno di un repository Git.
