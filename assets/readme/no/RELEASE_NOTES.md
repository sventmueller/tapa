Tapa 1.0.3 åpner arbeidsområder på andre datamaskiner over SSH og foreslår kommandoer mens du skriver.

## Nytt
- Eksterne arbeidsområder: «Nytt arbeidsområde» tilbyr en mappe på en annen datamaskin: en vert fra `~/.ssh/config` eller en adresse, port, bruker og passord ved behov, og mappen der. Terminalene og agentene som er installert der, kjører på den datamaskinen gjennom én SSH-tilkobling. Et passord kan lagres i nøkkelringen, og en ukjent vertsnøkkel godtas aldri uten bekreftelse. Fillisten, Git-panelet, redigeringsprogrammet og forhåndsvisningene er ikke tilgjengelige der.
- Sidepanelet viser tilkoblingen til et eksternt arbeidsområde med «Koble til på nytt», «Koble fra» og «Rediger tilkobling». Når en tapt tilkobling kommer tilbake, starter terminalene på nytt, og agentfaner fortsetter samtalen.
- Forslag mens du skriver: fra andre tegn vises resten av en foreslått kommando svakt etter markøren, og → tar den. Tab tar et sikkert forslag eller åpner en fullføringsmeny som beskriver hver kandidat, med eller uten inndatafeltet.
- Kommandofarger: kommandoer, underkommandoer, alternativer, variabler og operatorer får temaets farger, og en kommando skallet ikke kan kjøre, får en rød stiplet understreking. «Innstillinger › Inndata» slår av forslag og farger.
- I en agentfane åpner knappene for underagenter og bakgrunnsprosesser og /tasks en aktivitetsvisning med status og kjøretid for hvert element, med «Vis» og «Stopp».

## Forbedringer
- «Innstillinger › Generelt › Vindu» kan slå av endring av navn på et arbeidsområde ved klikk på navnet.
- Sidepanelet viser antall endrede filer, Git-knappen og mappeknappen i denne rekkefølgen, og Git-knappen bare inne i et Git-repository.
