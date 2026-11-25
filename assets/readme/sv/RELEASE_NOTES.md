Tapa 1.0.0 är den första versionen som skivavbild: öppna `Tapa-1.0.0.dmg` och dra Tapa till mappen Appar. Tapa 0.1.0 uppdaterar sig själv till den, och Homebrew installerar samma avbild.

## Nytt
- När du stänger Tapas sista fönster avslutas Tapa och allt bevaras: arbetsytor, flikar, delningar, historik och agentsessioner kommer tillbaka vid nästa start som du lämnade dem. Tapa frågar först bara när ett annat program än ett skal fortfarande körs.
- Dra arbetsytorna i sidofältet till valfri ordning; Terminal ligger alltid överst.
- Ett nytt fönster öppnas med storleken och sidofältsbredden från det fönster du senast ändrade storlek på, även när Tapa inte återställer några fönster, och Tapa kommer ihåg om inmatningsfältet visas.
- När Tapa startar med sidofältet hopfällt förblir knappen **Ny arbetsyta** dold.
- Utdata som ett program skriver i en skur visas när programmet pausar, högst 8 ms senare, så att halvritade skärmar inte längre blinkar.
- Varje session körs på Tapas egen terminalkärna; inställningen som växlade till Alacrittys kärna är borta.
- Mapp och gren ovanför inmatningsfältet är vanlig grå text, justerad efter platshållartexten.
- I en agentflik listar `/resume` agentens tidigare sessioner i den här mappen och fortsätter en av dem i en egen flik.
- En lång lista i överlägget i en agentflik rullar i stället för att göra fönstret högre.
- En agent räknas som klar först efter tre sekunders inaktivitet, så dess status flimrar inte längre mellan stegen.
- I helskärm ligger sidofältet kvar under verktygsfältet och glider ner med det när menyraden visas.

## Tapa i korthet
- Arbetsytor i ett sidofält, var och en med sina flikar, delningar, agenter, redigerare och webbläsarflikar; fönster och agentsessioner kommer tillbaka efter en omstart.
- Agentflikar för Claude, Codex, OpenCode och alla ACP-agenter från registret, som du installerar, uppdaterar och återställer till föregående version i inställningarna.
- Panelerna Filer och Git, förhandsvisningar av Markdown och bilder, ett inmatningsfält för kommandon med diktering.
- Ljust, mörkt eller systemets utseende, fyra textstorlekar, 35 språk i gränssnittet.
