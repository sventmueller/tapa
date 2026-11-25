Tapa 1.0.0 je prva izdaja v obliki slike diska: odprite `Tapa-1.0.0.dmg` in povlecite Tapo v mapo Aplikacije. Tapa 0.1.0 se nanjo posodobi sama, Homebrew pa namesti isto sliko.

## Novo
- Z zaprtjem zadnjega okna Tapa se Tapa zapre in ohrani vse: delovni prostori, zavihki, razdelitve, zgodovina in seje agentov se ob naslednjem zagonu vrnejo, kakor ste jih pustili. Tapa vpraša vnaprej le, če še teče program, ki ni lupina.
- Delovne prostore v stranski vrstici povlecite v poljubno zaporedje; Terminal vedno ostane na vrhu.
- Novo okno se odpre z velikostjo in širino stranske vrstice okna, ki ste mu nazadnje spremenili velikost, tudi kadar Tapa ne obnovi nobenega okna, Tapa pa si zapomni, ali je vnosno polje prikazano.
- Ko se Tapa zažene s strnjeno stransko vrstico, gumb »Nov delovni prostor« ostane skrit.
- Izhod, ki ga program zapiše naenkrat, se prikaže, ko program naredi premor, najpozneje 8 ms kasneje, tako da napol narisani zasloni ne utripajo več.
- Vsaka seja teče na lastnem terminalskem jedru Tape; nastavitev, ki je preklapljala na jedro Alacritty, je izginila.
- Mapa in veja nad vnosnim poljem sta navadno sivo besedilo, poravnano z nadomestnim besedilom.
- V zavihku agenta `/resume` navede prejšnje seje agenta v tej mapi in eno od njih nadaljuje v lastnem zavihku.
- Dolg seznam v prekrivnem pogledu zavihka agenta se pomika, namesto da bi okno naredil višje.
- Agent velja za končanega šele po treh sekundah mirovanja, zato njegovo stanje med koraki ne utripa več.
- V celozaslonskem načinu stranska vrstica ostane pod orodno vrstico in se z njo spusti, ko se prikaže menijska vrstica.

## Tapa na prvi pogled
- Delovni prostori v stranski vrstici, vsak s svojimi zavihki, razdelitvami, agenti, urejevalniki in zavihki brskalnika; okna in seje agentov se po ponovnem zagonu vrnejo.
- Zavihki agentov za Claude, Codex, OpenCode in katerega koli agenta ACP iz registra, ki jih v nastavitvah namestite, posodobite in vrnete na prejšnjo različico.
- Plošči za datoteke in Git, predogledi Markdowna in slik, vnosno polje za ukaze z narekovanjem.
- Svetel, temen ali sistemski videz, štiri velikosti pisave, vmesnik v 35 jezikih.
