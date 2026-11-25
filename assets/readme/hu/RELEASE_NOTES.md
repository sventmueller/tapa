A Tapa 1.0.0 az első kiadás lemezképként: nyissa meg a `Tapa-1.0.0.dmg` fájlt, és húzza a Tapát az Alkalmazások mappára. A Tapa 0.1.0 magától frissül erre a verzióra, a Homebrew pedig ugyanezt a lemezképet telepíti.

## Újdonságok
- Ha bezárja a Tapa utolsó ablakát, a Tapa kilép, és mindent megőriz: a munkaterületek, a lapok, a felosztások, az előzmények és az ügynökmunkamenetek a következő indításkor úgy térnek vissza, ahogyan hagyta őket. A Tapa csak akkor kérdez előtte, ha a shellen kívül más program még fut.
- Húzza az oldalsáv munkaterületeit tetszőleges sorrendbe; a Terminál mindig legfelül marad.
- Az új ablak annak az ablaknak a méretével és oldalsáv-szélességével nyílik meg, amelyet legutóbb átméretezett, akkor is, ha a Tapa nem állít vissza ablakokat, és a Tapa megjegyzi, hogy látható-e a beviteli mező.
- Ha a Tapa összecsukott oldalsávval indul, az Új munkaterület gomb rejtve marad.
- A program által löketekben írt kimenet akkor jelenik meg, amikor a program szünetet tart, legfeljebb 8 ms-mal később, így a félig megrajzolt képernyők már nem villognak.
- Minden munkamenet a Tapa saját terminálmagján fut; az a beállítás, amely az Alacritty magjára váltott, megszűnt.
- A beviteli mező feletti mappa és ág egyszerű szürke szöveg, a helyőrzőhöz igazítva.
- Egy ügynöklapon a /resume felsorolja az ügynök korábbi munkameneteit ebben a mappában, és egyiket a saját lapján folytatja.
- Egy ügynöklap fedőrétegében a hosszú lista görgethető, ahelyett hogy magasabbá tenné az ablakot.
- Egy ügynök csak három másodpercnyi tétlenség után számít késznek, így az állapota már nem villog a lépések között.
- Teljes képernyőn az oldalsáv az eszközsor alatt marad, és vele együtt csúszik lejjebb, amikor megjelenik a menüsor.

## A Tapa dióhéjban
- Munkaterületek az oldalsávban, mindegyik a saját lapjaival, felosztásaival, ügynökeivel, szerkesztőivel és böngészőlapjaival; az ablakok és az ügynökmunkamenetek újraindítás után is visszatérnek.
- Ügynöklapok a Claude, a Codex, az OpenCode és az ACP-jegyzék bármely ügynöke számára, amelyeket a beállításokból telepíthet, frissíthet és állíthat vissza egy korábbi verzióra.
- Fájl- és Git-panel, Markdown- és képelőnézet, parancsbeviteli mező diktálással.
- Világos, sötét vagy a rendszert követő megjelenés, négy betűméret, 35 nyelvű felület.
