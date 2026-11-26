Tapa 1.0.6 skyddar mot Markdown som är gjord för att tömma minnet och behåller ett programs sista utdata i terminalen.

## Åtgärdat
- Ett Markdown-dokument med hundratusentals tomma tabellceller, i en förhandsvisning eller i en agents svar, tar inte längre sekunder och gigabyte minne: över 500 000 tomma celler visas tabellerna som text.
- En terminal missar inte längre slutet på sitt program och förlorar inte programmets sista utdata när Macen är upptagen.

## Förbättringar
- Agentens historik utelämnar hooks som körde utan fel eller fortfarande körs; en hook som misslyckades visas fortfarande.
