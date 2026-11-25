Tapa 1.0.3 åbner arbejdsområder på andre computere over SSH og foreslår kommandoer, mens du skriver.

## Nyt
- Eksterne arbejdsområder: "Nyt arbejdsområde" tilbyder en mappe på en anden computer: en vært fra `~/.ssh/config` eller en adresse, port, bruger og adgangskode efter behov og mappen dér. Terminalerne og de agenter, der er installeret dér, kører på den computer gennem én SSH-forbindelse. En adgangskode kan gemmes i nøgleringen, og en ukendt værtsnøgle accepteres aldrig uden bekræftelse. Fillisten, Git-panelet, editoren og eksemplerne er ikke tilgængelige dér.
- Indholdsoversigten viser et eksternt arbejdsområdes forbindelse med "Forbind igen", "Afbryd forbindelse" og "Rediger forbindelse". Når en mistet forbindelse kommer tilbage, starter terminalerne igen, og agentfaner fortsætter deres samtale.
- Forslag, mens du skriver: fra andet tegn vises resten af en foreslået kommando svagt efter markøren, og → tager den. Tab tager et sikkert forslag eller åbner en fuldførelsesmenu, der beskriver hver kandidat, med eller uden inputfeltet.
- Kommandofarver: kommandoer, underkommandoer, indstillinger, variabler og operatorer får temaets farver, og en kommando, som shellen ikke kan køre, får en stiplet rød understregning. "Indstillinger › Input" slår forslag og farver fra.
- I en agentfane åbner knapperne for underagenter og baggrundsprocesser samt /tasks en aktivitetsoversigt med hvert elements status og køretid samt "Vis" og "Stop".

## Forbedringer
- "Indstillinger › Generelt › Vindue" kan slå omdøbning af et arbejdsområde ved klik på navnet fra.
- Indholdsoversigten viser antallet af ændrede filer, Git-knappen og mappeknappen i denne rækkefølge, og Git-knappen kun i et Git-repository.
