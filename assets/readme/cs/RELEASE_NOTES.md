Tapa 1.0.0 je první vydání v podobě obrazu disku: otevřete `Tapa-1.0.0.dmg` a přetáhněte Tapu do složky Aplikace. Tapa 0.1.0 se na ni aktualizuje sama a Homebrew instaluje stejný obraz.

## Novinky
- Zavřením posledního okna Tapa skončí a zachová vše: pracovní prostory, panely, rozdělení, historie a relace agentů se při dalším spuštění vrátí tak, jak jste je nechali. Tapa se předem zeptá jen tehdy, když ještě běží jiný program než shell.
- Přetahujte pracovní prostory v bočním panelu do libovolného pořadí; „Terminál“ zůstává vždy nahoře.
- Nové okno se otevře s velikostí a šířkou bočního panelu okna, jehož velikost jste naposledy změnili, i když Tapa neobnovuje žádná okna, a Tapa si pamatuje, zda je zobrazené vstupní pole.
- Když se Tapa spustí se sbaleným bočním panelem, tlačítko „Nový pracovní prostor“ zůstane skryté.
- Výstup, který program zapíše po dávkách, se zobrazí, jakmile program na chvíli přestane, nejpozději o 8 ms později, takže napůl vykreslené obrazovky už neblikají.
- Každá relace běží na vlastním jádru terminálu Tapy; nastavení, které přepínalo na jádro Alacritty, je pryč.
- Složka a větev nad vstupním polem jsou prostý šedý text zarovnaný se zástupným textem.
- V panelu agenta /resume vypíše dřívější relace agenta v této složce a jednu z nich obnoví v samostatném panelu.
- Dlouhý seznam v překryvu panelu agenta se posouvá, místo aby zvětšoval okno.
- Agent se považuje za hotového až po třech sekundách nečinnosti, takže jeho stav mezi kroky už nebliká.
- Na celé obrazovce zůstává boční panel pod nástrojovým panelem a sjede s ním dolů, když se zobrazí řádek nabídek.

## Tapa v kostce
- Pracovní prostory v bočním panelu, každý s vlastními panely, rozděleními, agenty, editory a panely prohlížeče; okna a relace agentů se po restartu vrátí.
- Panely agentů pro Claude, Codex, OpenCode a libovolného agenta ACP z registru, které se instalují, aktualizují a vracejí na předchozí verzi v nastavení.
- Panely souborů a Gitu, náhledy Markdownu a obrázků, vstupní pole pro příkazy s diktováním.
- Světlý, tmavý nebo systémový vzhled, čtyři velikosti písma, rozhraní v 35 jazycích.
