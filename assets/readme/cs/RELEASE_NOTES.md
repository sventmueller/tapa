Tapa 1.0.3 otevírá pracovní prostory na jiných počítačích přes SSH a při psaní navrhuje příkazy.

## Novinky
- Vzdálené pracovní prostory: „Nový pracovní prostor“ nabízí složku na jiném počítači: hostitele z `~/.ssh/config` nebo adresu, podle potřeby port, uživatele a heslo, a složku tam. Terminály a agenti, kteří jsou tam nainstalovaní, běží na tom počítači přes jedno spojení SSH. Heslo lze uložit do klíčenky a neznámý klíč hostitele se nikdy nepřijme bez potvrzení. Seznam souborů, panel Git, editor a náhledy tam k dispozici nejsou.
- Boční panel ukazuje připojení vzdáleného pracovního prostoru s volbami „Znovu připojit“, „Odpojit“ a „Upravit připojení“. Když se ztracené připojení obnoví, terminály se znovu spustí a karty agentů pokračují v konverzaci.
- Návrhy při psaní: od druhého znaku se zbytek navrženého příkazu zobrazuje bledě za kurzorem a → ho převezme. Tab převezme jistý návrh, nebo otevře nabídku doplňování s popisem každé možnosti, se vstupním polem i bez něj.
- Barvy příkazů: příkazy, podpříkazy, volby, proměnné a operátory dostávají barvy motivu a příkaz, který shell nemůže spustit, je podtržen červenou přerušovanou čarou. „Nastavení › Vstup“ návrhy i barvy vypne.
- Na kartě agenta otevírají tlačítka pro podagenty a procesy na pozadí i /tasks přehled aktivity se stavem a dobou běhu každé položky, s volbami „Zobrazit“ a „Zastavit“.

## Vylepšení
- V „Nastavení › Obecné › Okno“ lze vypnout přejmenování pracovního prostoru kliknutím na jeho název.
- Boční panel ukazuje počet změněných souborů, tlačítko Git a tlačítko složky v tomto pořadí, tlačítko Git jen uvnitř repozitáře Git.
