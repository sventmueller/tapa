Tapa 1.0.0 este prima versiune livrată ca imagine de disc: deschideți `Tapa-1.0.0.dmg` și trageți Tapa în dosarul Aplicații. Tapa 0.1.0 se actualizează singur la această versiune, iar Homebrew instalează aceeași imagine.

## Nou
- Închiderea ultimei ferestre a Tapa încheie Tapa și păstrează totul: spațiile de lucru, filele, împărțirile, istoricul și sesiunile agenților revin la următoarea pornire așa cum le-ați lăsat. Tapa întreabă mai întâi doar atunci când încă rulează un program care nu este un shell.
- Trageți spațiile de lucru din bara laterală în orice ordine; Terminal rămâne întotdeauna sus.
- O fereastră nouă se deschide cu dimensiunea și lățimea barei laterale ale ferestrei pe care ați redimensionat-o ultima dată, chiar și atunci când Tapa nu restaurează nicio fereastră, iar Tapa reține dacă este afișat câmpul de introducere.
- Când Tapa pornește cu bara laterală restrânsă, butonul „Spațiu de lucru nou” rămâne ascuns.
- Ieșirea pe care un program o scrie în rafală apare după ce programul face o pauză, cel mult 8 ms mai târziu, astfel încât ecranele desenate pe jumătate nu mai clipesc.
- Fiecare sesiune rulează pe nucleul de terminal propriu al Tapa; configurarea care comuta pe nucleul Alacritty a dispărut.
- Dosarul și ramura de deasupra câmpului de introducere sunt text gri simplu, aliniat cu textul substituent.
- Într-o filă de agent, `/resume` listează sesiunile anterioare ale agentului din acest dosar și continuă una dintre ele într-o filă proprie.
- O listă lungă din suprapunerea unei file de agent derulează, în loc să mărească fereastra pe înălțime.
- Un agent este considerat încheiat abia după trei secunde de inactivitate, astfel încât starea lui nu mai clipește între pași.
- Pe tot ecranul, bara laterală rămâne sub bara de instrumente și coboară odată cu ea când apare bara de meniu.

## Tapa pe scurt
- Spații de lucru într-o bară laterală, fiecare cu filele, împărțirile, agenții, editoarele și filele de browser proprii; ferestrele și sesiunile agenților revin după o repornire.
- File de agent pentru Claude, Codex, OpenCode și orice agent ACP din registru, instalați, actualizați și readuși la o versiune anterioară din configurări.
- Panouri pentru fișiere și Git, previzualizări pentru Markdown și imagini, un câmp de introducere a comenzilor cu dictare.
- Aspect luminos, întunecat sau după sistem, patru dimensiuni de font, interfață în 35 de limbi.
