Tapa 1.0.3 otwiera obszary robocze na innych komputerach przez SSH i podpowiada polecenia podczas pisania.

## Nowości
- Zdalne obszary robocze: „Nowy obszar roboczy” oferuje folder na innym komputerze: hosta z `~/.ssh/config` lub adres, w razie potrzeby port, użytkownika i hasło oraz folder na tym komputerze. Terminale i zainstalowane tam agenty działają na tym komputerze przez jedno połączenie SSH. Hasło można zachować w pęku kluczy, a nieznany klucz hosta nigdy nie jest akceptowany bez potwierdzenia. Lista plików, panel Git, edytor i podglądy nie są tam dostępne.
- Pasek boczny pokazuje połączenie zdalnego obszaru roboczego z opcjami „Połącz ponownie”, „Rozłącz” i „Edytuj połączenie”. Gdy utracone połączenie wraca, terminale uruchamiają się ponownie, a karty agentów kontynuują rozmowę.
- Podpowiedzi podczas pisania: od drugiego znaku reszta podpowiadanego polecenia pojawia się blado za kursorem, a → ją przyjmuje. Tab przyjmuje pewną podpowiedź albo otwiera menu uzupełniania z opisem każdej możliwości, z polem wprowadzania lub bez niego.
- Kolory poleceń: polecenia, podpolecenia, opcje, zmienne i operatory przyjmują kolory motywu, a polecenie, którego powłoka nie może uruchomić, zostaje podkreślone czerwoną przerywaną linią. „Ustawienia › Wprowadzanie” wyłącza podpowiedzi i kolory.
- Na karcie agenta przyciski podagentów i procesów w tle oraz /tasks otwierają widok aktywności ze stanem i czasem działania każdej pozycji, z opcjami „Pokaż” i „Zatrzymaj”.

## Ulepszenia
- W „Ustawienia › Ogólne › Okno” można wyłączyć zmianę nazwy obszaru roboczego kliknięciem w nazwę.
- Pasek boczny pokazuje liczbę zmienionych plików, przycisk Git i przycisk folderu w tej kolejności, a przycisk Git tylko wewnątrz repozytorium Git.
