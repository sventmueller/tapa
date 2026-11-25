Tapa 1.0.3 otvara radne prostore na drugim računalima preko SSH-a i predlaže naredbe dok tipkate.

## Novo
- Udaljeni radni prostori: „Novi radni prostor” nudi mapu na drugom računalu: host iz `~/.ssh/config` ili adresu, prema potrebi port, korisnika i lozinku te mapu tamo. Terminali i agenti instalirani tamo rade na tom računalu kroz jednu SSH vezu. Lozinka se može spremiti u privjesak ključeva, a nepoznati ključ hosta nikad se ne prihvaća bez potvrde. Popis datoteka, ploča Git, uređivač i pretpregledi tamo nisu dostupni.
- Bočna traka prikazuje vezu udaljenog radnog prostora s opcijama „Ponovno poveži”, „Prekini vezu” i „Uredi vezu”. Kad se izgubljena veza vrati, terminali se ponovno pokreću, a kartice agenata nastavljaju razgovor.
- Prijedlozi tijekom tipkanja: od drugog znaka ostatak predložene naredbe pojavljuje se blijedo iza kursora, a → ga prihvaća. Tab prihvaća siguran prijedlog ili otvara izbornik dovršavanja koji opisuje svakog kandidata, s poljem za unos ili bez njega.
- Boje naredbi: naredbe, podnaredbe, opcije, varijable i operatori dobivaju boje teme, a naredba koju ljuska ne može pokrenuti dobiva crveno isprekidano podcrtavanje. „Postavke › Unos” isključuje prijedloge i boje.
- Na kartici agenta gumbi za podagente i pozadinske procese te /tasks otvaraju prikaz aktivnosti sa stanjem i vremenom izvođenja svake stavke, uz „Prikaži” i „Zaustavi”.

## Poboljšanja
- U „Postavke › Općenito › Prozor” može se isključiti preimenovanje radnog prostora klikom na njegov naziv.
- Bočna traka prikazuje broj promijenjenih datoteka, gumb Git i gumb mape tim redoslijedom, a gumb Git samo unutar Git repozitorija.
