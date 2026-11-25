Tapa 1.0.3 otvára pracovné priestory na iných počítačoch cez SSH a pri písaní navrhuje príkazy.

## Novinky
- Vzdialené pracovné priestory: „Nový pracovný priestor“ ponúka priečinok na inom počítači: hostiteľa z `~/.ssh/config` alebo adresu, podľa potreby port, používateľa a heslo, a priečinok tam. Terminály a agenti, ktorí sú tam nainštalovaní, bežia na tom počítači cez jedno pripojenie SSH. Heslo možno uložiť do kľúčenky a neznámy kľúč hostiteľa sa nikdy neprijme bez potvrdenia. Zoznam súborov, panel Git, editor a náhľady tam nie sú k dispozícii.
- Bočný panel zobrazuje pripojenie vzdialeného pracovného priestoru s voľbami „Znova pripojiť“, „Odpojiť“ a „Upraviť pripojenie“. Keď sa stratené pripojenie obnoví, terminály sa znova spustia a karty agentov pokračujú v konverzácii.
- Návrhy pri písaní: od druhého znaku sa zvyšok navrhnutého príkazu zobrazuje bledo za kurzorom a → ho prevezme. Tab prevezme istý návrh alebo otvorí ponuku dopĺňania s popisom každej možnosti, so vstupným poľom aj bez neho.
- Farby príkazov: príkazy, podpríkazy, voľby, premenné a operátory dostávajú farby témy a príkaz, ktorý shell nemôže spustiť, je podčiarknutý červenou prerušovanou čiarou. „Nastavenia › Vstup“ vypne návrhy aj farby.
- Na karte agenta otvárajú tlačidlá pre podagentov a procesy na pozadí aj /tasks prehľad aktivity so stavom a časom behu každej položky, s voľbami „Zobraziť“ a „Zastaviť“.

## Vylepšenia
- V „Nastavenia › Všeobecné › Okno“ možno vypnúť premenovanie pracovného priestoru kliknutím na jeho názov.
- Bočný panel zobrazuje počet zmenených súborov, tlačidlo Git a tlačidlo priečinka v tomto poradí, tlačidlo Git len v repozitári Git.
