Tapa 1.0.3 öppnar arbetsytor på andra datorer via SSH och föreslår kommandon medan du skriver.

## Nytt
- Fjärrarbetsytor: ”Ny arbetsyta” erbjuder en mapp på en annan dator: en värd från `~/.ssh/config` eller en adress, port, användare och lösenord vid behov, och mappen där. Terminalerna och agenterna som är installerade där körs på den datorn via en enda SSH-anslutning. Ett lösenord kan sparas i nyckelringen, och en okänd värdnyckel godtas aldrig utan bekräftelse. Fillistan, Git-panelen, redigeraren och förhandsvisningarna finns inte där.
- Sidofältet visar fjärrarbetsytans anslutning med ”Anslut igen”, ”Koppla från” och ”Redigera anslutning”. När en förlorad anslutning kommer tillbaka startar terminalerna igen och agentflikarna fortsätter sitt samtal.
- Förslag medan du skriver: från andra tecknet visas resten av ett föreslaget kommando svagt efter markören, och → tar det. Tab tar ett säkert förslag eller öppnar en kompletteringsmeny som beskriver varje kandidat, med eller utan inmatningsfältet.
- Kommandofärger: kommandon, underkommandon, alternativ, variabler och operatorer får temats färger, och ett kommando som skalet inte kan köra får en streckad röd understrykning. ”Inställningar › Inmatning” stänger av förslag och färger.
- I en agentflik öppnar knapparna för underagenter och bakgrundsprocesser samt /tasks en aktivitetsvy med status och körtid för varje post, med ”Visa” och ”Stoppa”.

## Förbättringar
- ”Inställningar › Allmänt › Fönster” kan stänga av namnbyte på en arbetsyta genom att klicka på namnet.
- Sidofältet visar antalet ändrade filer, Git-knappen och mappknappen i den här ordningen, och Git-knappen bara i ett Git-förråd.
