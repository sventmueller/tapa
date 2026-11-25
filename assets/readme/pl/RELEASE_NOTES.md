Tapa 1.0.0 to pierwsze wydanie w postaci obrazu dysku: otwórz `Tapa-1.0.0.dmg` i przeciągnij Tapę do folderu Programy. Tapa 0.1.0 sama się do niego uaktualni, a Homebrew instaluje ten sam obraz.

## Nowości
- Zamknięcie ostatniego okna Tapy zamyka Tapę i zachowuje wszystko: obszary robocze, karty, podziały, historia i sesje agentów wracają przy następnym uruchomieniu takie, jak je zostawiłeś. Tapa pyta wcześniej tylko wtedy, gdy nadal działa program inny niż powłoka.
- Przeciągaj obszary robocze w panelu bocznym w dowolną kolejność; Terminal zawsze zostaje na górze.
- Nowe okno otwiera się z rozmiarem i szerokością panelu bocznego okna, którego rozmiar zmieniłeś ostatnio, także gdy Tapa nie przywraca żadnych okien, a Tapa pamięta, czy pole wprowadzania jest widoczne.
- Gdy Tapa uruchamia się ze zwiniętym panelem bocznym, przycisk „Nowy obszar roboczy” pozostaje ukryty.
- Dane wyjściowe, które program zapisuje seriami, pojawiają się, gdy program zrobi pauzę, najpóźniej po 8 ms, dzięki czemu na wpół narysowane ekrany już nie migają.
- Każda sesja działa na własnym rdzeniu terminala Tapy; ustawienie przełączające na rdzeń Alacritty zniknęło.
- Folder i gałąź nad polem wprowadzania to zwykły szary tekst, wyrównany do tekstu zastępczego.
- W karcie agenta `/resume` wyświetla dawne sesje agenta w tym folderze i kontynuuje jedną z nich we własnej karcie.
- Długa lista w nakładce karty agenta przewija się, zamiast powiększać okno.
- Agent jest uznawany za zakończonego dopiero po trzech sekundach bezczynności, więc jego stan nie migocze już między krokami.
- W trybie pełnoekranowym panel boczny pozostaje pod paskiem narzędzi i zjeżdża w dół razem z nim, gdy pojawia się pasek menu.

## Tapa w skrócie
- Obszary robocze w panelu bocznym, każdy z własnymi kartami, podziałami, agentami, edytorami i kartami przeglądarki; okna i sesje agentów wracają po ponownym uruchomieniu.
- Karty agentów dla Claude, Codex, OpenCode i dowolnego agenta ACP z rejestru, instalowanych, uaktualnianych i przywracanych do poprzedniej wersji w ustawieniach.
- Panele plików i Git, podglądy Markdown i obrazów, pole wprowadzania poleceń z dyktowaniem.
- Wygląd jasny, ciemny lub zgodny z systemem, cztery rozmiary czcionki, interfejs w 35 językach.
