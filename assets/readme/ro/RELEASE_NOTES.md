Tapa 1.0.3 deschide spații de lucru pe alte computere prin SSH și sugerează comenzi în timp ce tastați.

## Noutăți
- Spații de lucru la distanță: „Spațiu de lucru nou” oferă un dosar pe alt computer: o gazdă din `~/.ssh/config` sau o adresă, portul, utilizatorul și parola dacă e nevoie, și dosarul de acolo. Terminalele și agenții instalați acolo rulează pe acel computer printr-o singură conexiune SSH. Parola poate fi păstrată în portchei, iar o cheie de gazdă necunoscută nu este acceptată niciodată fără confirmare. Lista de fișiere, panoul Git, editorul și previzualizările nu sunt disponibile acolo.
- Bara laterală arată conexiunea unui spațiu de lucru la distanță cu „Reconectați”, „Deconectați” și „Editați conexiunea”. Când o conexiune pierdută revine, terminalele pornesc din nou, iar filele agenților își continuă conversația.
- Sugestii în timp ce tastați: de la al doilea caracter, restul unei comenzi sugerate apare estompat după cursor, iar → o acceptă. Tab acceptă o sugestie sigură sau deschide un meniu de completare care descrie fiecare variantă, cu sau fără câmpul de introducere.
- Culorile comenzilor: comenzile, subcomenzile, opțiunile, variabilele și operatorii primesc culorile temei, iar o comandă pe care shell-ul nu o poate rula este subliniată cu o linie roșie întreruptă. „Configurări › Introducere” dezactivează sugestiile și culorile.
- Într-o filă de agent, butoanele pentru subagenți și procese în fundal, precum și /tasks, deschid o vizualizare a activității cu starea și durata de rulare a fiecărui element, cu „Afișați” și „Opriți”.

## Îmbunătățiri
- „Configurări › General › Fereastră” poate dezactiva redenumirea unui spațiu de lucru prin clic pe nume.
- Bara laterală arată numărul de fișiere modificate, butonul Git și butonul dosarului în această ordine, iar butonul Git doar într-un depozit Git.
