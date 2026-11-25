Tapa 1.0.0 prvo je izdanje u obliku slike diska: otvorite `Tapa-1.0.0.dmg` i povucite Tapu u mapu Aplikacije. Tapa 0.1.0 sama se ažurira na nju, a Homebrew instalira istu sliku.

## Novo
- Zatvaranjem posljednjeg prozora Tapa se zatvara i sve zadržava: radni prostori, kartice, podjele, povijest i sesije agenata vraćaju se pri sljedećem pokretanju onako kako ste ih ostavili. Tapa prvo pita samo ako još radi neki program koji nije shell.
- Povucite radne prostore u bočnoj traci u bilo koji redoslijed; „Terminal” uvijek ostaje na vrhu.
- Novi prozor otvara se s veličinom i širinom bočne trake prozora kojem ste posljednji put promijenili veličinu, čak i kad Tapa ne vraća nijedan prozor, a Tapa pamti je li polje za unos prikazano.
- Kad se Tapa pokrene sa sklopljenom bočnom trakom, gumb „Novi radni prostor” ostaje skriven.
- Izlaz koji program ispiše u naletu prikazuje se čim program stane, najkasnije 8 ms poslije, pa napola iscrtani zasloni više ne trepere.
- Svaka sesija radi na Tapinoj vlastitoj jezgri terminala; postavka koja je prebacivala na jezgru Alacritty više ne postoji.
- Mapa i grana iznad polja za unos običan su sivi tekst, poravnat s tekstom rezerviranog mjesta.
- U kartici agenta /resume navodi prijašnje sesije agenta u ovoj mapi i nastavlja jednu u zasebnoj kartici.
- Dugi popis u preklopu kartice agenta pomiče se umjesto da prozor postane viši.
- Agent se smatra gotovim tek nakon tri sekunde mirovanja, pa njegov status više ne treperi između koraka.
- U cijelom zaslonu bočna traka ostaje ispod alatne trake i spušta se zajedno s njom kad se prikaže traka izbornika.

## Tapa ukratko
- Radni prostori u bočnoj traci, svaki sa svojim karticama, podjelama, agentima, uređivačima i karticama preglednika; prozori i sesije agenata vraćaju se nakon ponovnog pokretanja.
- Kartice agenata za Claude, Codex, OpenCode i bilo koji ACP agent iz registra, koje instalirate, ažurirate i vraćate na prethodnu verziju u postavkama.
- Paneli za datoteke i Git, pregledi Markdowna i slika, polje za unos naredbi s diktatom.
- Svijetli, tamni ili sustavni izgled, četiri veličine fonta, sučelje na 35 jezika.
