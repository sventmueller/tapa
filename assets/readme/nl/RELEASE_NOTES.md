Tapa 1.0.3 opent werkruimtes op andere computers via SSH en stelt commando's voor terwijl je typt.

## Nieuw
- Externe werkruimtes: "Nieuwe werkruimte" biedt een map op een andere computer aan: een host uit `~/.ssh/config` of een adres, zo nodig poort, gebruiker en wachtwoord, en de map daar. De terminals en de daar geïnstalleerde agents draaien op die computer via één SSH-verbinding. Een wachtwoord kan in de sleutelhanger blijven, en een onbekende hostsleutel wordt nooit zonder bevestiging geaccepteerd. De bestandenlijst, het Git-paneel, de editor en de voorvertoningen zijn daar niet beschikbaar.
- De navigatiekolom toont de verbinding van een externe werkruimte met "Maak opnieuw verbinding", "Verbreek verbinding" en "Wijzig verbinding". Komt een verloren verbinding terug, dan starten de terminals opnieuw en zetten agenttabbladen hun gesprek voort.
- Suggesties tijdens het typen: vanaf het tweede teken verschijnt de rest van een voorgesteld commando vaag achter de cursor, en → neemt het over. Tab neemt een zekere suggestie over of opent een aanvulmenu dat elke kandidaat beschrijft, met of zonder het invoerveld.
- Commandokleuren: commando's, subcommando's, opties, variabelen en operatoren krijgen de kleuren van het thema, en een commando dat de shell niet kan uitvoeren krijgt een rode stippellijn eronder. "Instellingen › Invoer" schakelt suggesties en kleuren uit.
- In een agenttabblad openen de knoppen voor subagents en achtergrondprocessen en /tasks een activiteitenweergave met de status en looptijd van elk item, met "Toon" en "Stop".

## Verbeteringen
- Met "Instellingen › Algemeen › Venster" kun je het wijzigen van de naam van een werkruimte door op de naam te klikken uitschakelen.
- De navigatiekolom toont het aantal gewijzigde bestanden, de Git-knop en de mapknop in deze volgorde, en de Git-knop alleen binnen een Git-repository.
