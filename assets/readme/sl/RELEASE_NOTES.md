Tapa 1.0.3 odpira delovne prostore na drugih računalnikih prek SSH in med tipkanjem predlaga ukaze.

## Novo
- Oddaljeni delovni prostori: »Nov delovni prostor« ponuja mapo na drugem računalniku: gostitelja iz `~/.ssh/config` ali naslov, po potrebi vrata, uporabnika in geslo ter mapo tam. Terminali in agenti, nameščeni tam, tečejo na tistem računalniku prek ene povezave SSH. Geslo je mogoče shraniti v obesek za ključe, neznan ključ gostitelja pa nikoli ni sprejet brez potrditve. Seznam datotek, plošča Git, urejevalnik in predogledi tam niso na voljo.
- Stranska vrstica prikazuje povezavo oddaljenega delovnega prostora z možnostmi »Znova poveži«, »Prekini povezavo« in »Uredi povezavo«. Ko se izgubljena povezava vrne, se terminali znova zaženejo, zavihki agentov pa nadaljujejo pogovor.
- Predlogi med tipkanjem: od drugega znaka naprej se preostanek predlaganega ukaza bledo prikaže za kazalko, → pa ga sprejme. Tab sprejme zanesljiv predlog ali odpre meni za dokončanje, ki opiše vsako možnost, z vnosnim poljem ali brez njega.
- Barve ukazov: ukazi, podukazi, možnosti, spremenljivke in operatorji dobijo barve teme, ukaz, ki ga lupina ne more zagnati, pa je podčrtan z rdečo črtkano črto. »Nastavitve › Vnos« izklopi predloge in barve.
- Na zavihku agenta gumbi za podagente in procese v ozadju ter /tasks odprejo pregled dejavnosti s stanjem in časom izvajanja vsake postavke ter z »Prikaži« in »Ustavi«.

## Izboljšave
- V »Nastavitve › Splošno › Okno« je mogoče izklopiti preimenovanje delovnega prostora s klikom na ime.
- Stranska vrstica prikazuje število spremenjenih datotek, gumb Git in gumb mape v tem vrstnem redu, gumb Git pa le znotraj repozitorija Git.
