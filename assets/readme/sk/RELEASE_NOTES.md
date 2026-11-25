Tapa 1.0.0 je prvé vydanie v podobe obrazu disku: otvorte `Tapa-1.0.0.dmg` a pretiahnite Tapu do priečinka Aplikácie. Tapa 0.1.0 sa naň aktualizuje sama a Homebrew nainštaluje ten istý obraz.

## Novinky
- Zatvorenie posledného okna Tapy ukončí Tapu a zachová všetko: pracovné priestory, taby, rozdelenia, históriu a relácie agentov sa pri ďalšom spustení vrátia tak, ako ste ich nechali. Tapa sa spýta vopred, len keď ešte beží iný program než shell.
- Presúvajte pracovné priestory v bočnom paneli do ľubovoľného poradia; Terminál vždy zostane hore.
- Nové okno sa otvorí s veľkosťou a šírkou bočného panela okna, ktorého veľkosť ste naposledy zmenili, aj keď Tapa neobnovuje žiadne okná, a Tapa si pamätá, či je zobrazené vstupné pole.
- Keď sa Tapa spustí so zbaleným bočným panelom, tlačidlo „Nový pracovný priestor“ zostane skryté.
- Výstup, ktorý program zapíše naraz, sa zobrazí, keď program urobí pauzu, najneskôr o 8 ms neskôr, takže napoly vykreslené obrazovky už nebliknú.
- Každá relácia beží na vlastnom jadre terminálu Tapy; nastavenie, ktoré prepínalo na jadro Alacritty, zmizlo.
- Priečinok a vetva nad vstupným poľom sú obyčajný sivý text zarovnaný so zástupným textom.
- V tabe agenta `/resume` zobrazí predchádzajúce relácie agenta v tomto priečinku a pokračuje v jednej z nich vo vlastnom tabe.
- Dlhý zoznam v prekrytí tabu agenta sa posúva, namiesto toho aby okno zväčšil do výšky.
- Agent sa považuje za dokončeného až po troch sekundách nečinnosti, takže jeho stav už medzi krokmi nebliká.
- Na celej obrazovke zostáva bočný panel pod panelom nástrojov a posúva sa nadol spolu s ním, keď sa zobrazí lišta ponúk.

## Tapa v skratke
- Pracovné priestory v bočnom paneli, každý s vlastnými tabmi, rozdeleniami, agentmi, editormi a tabmi prehliadača; okná a relácie agentov sa po reštarte vrátia.
- Taby agentov pre Claude, Codex, OpenCode a ľubovoľného agenta ACP z registra, ktoré sa inštalujú, aktualizujú a vracajú na predchádzajúcu verziu v nastaveniach.
- Panely súborov a Gitu, náhľady Markdownu a obrázkov, vstupné pole pre príkazy s diktovaním.
- Svetlý, tmavý alebo systémový vzhľad, štyri veľkosti písma, rozhranie v 35 jazykoch.
