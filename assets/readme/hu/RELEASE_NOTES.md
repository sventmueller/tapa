A Tapa 1.0.3 SSH-n keresztül más számítógépeken nyit meg munkaterületeket, és gépelés közben parancsokat javasol.

## Újdonságok
- Távoli munkaterületek: az „Új munkaterület” egy másik számítógépen lévő mappát is felkínál: egy gazdagépet a `~/.ssh/config` fájlból vagy egy címet, szükség esetén portot, felhasználót és jelszót, valamint az ottani mappát. A terminálok és az ott telepített ügynökök egyetlen SSH-kapcsolaton át azon a számítógépen futnak. A jelszó a kulcskarikában tárolható, ismeretlen gazdagépkulcsot pedig a Tapa soha nem fogad el megerősítés nélkül. A fájllista, a Git-panel, a szerkesztő és az előnézetek ott nem érhetők el.
- Az oldalsáv mutatja a távoli munkaterület kapcsolatát az „Újracsatlakozás”, a „Kapcsolat bontása” és a „Kapcsolat szerkesztése” lehetőséggel. Ha egy megszakadt kapcsolat helyreáll, a terminálok újraindulnak, az ügynöklapok pedig folytatják a beszélgetést.
- Javaslatok gépelés közben: a második karaktertől a javasolt parancs többi része halványan jelenik meg a kurzor után, és a → elfogadja. A Tab elfogad egy biztos javaslatot, vagy megnyit egy kiegészítő menüt, amely minden jelöltet leír, a beviteli mezővel vagy anélkül.
- Parancsszínek: a parancsok, alparancsok, opciók, változók és operátorok a téma színeit kapják, a shell által nem futtatható parancsot pedig piros szaggatott aláhúzás jelöli. A „Beállítások › Bevitel” kikapcsolja a javaslatokat és a színeket.
- Az ügynöklapon az alügynökök és háttérfolyamatok gombjai, valamint a /tasks egy tevékenységnézetet nyitnak meg az egyes elemek állapotával és futási idejével, „Megjelenítés” és „Leállítás” lehetőséggel.

## Fejlesztések
- A „Beállítások › Általános › Ablak” alatt kikapcsolható a munkaterület átnevezése a nevére kattintva.
- Az oldalsáv ebben a sorrendben mutatja a módosított fájlok számát, a Git-gombot és a mappagombot, a Git-gombot pedig csak Git-tárolóban.
