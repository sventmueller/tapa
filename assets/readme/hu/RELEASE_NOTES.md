A Tapa 1.0.6 véd a memória kimerítésére készített Markdown ellen, és megtartja a program utolsó kimenetét a terminálban.

## Javítások
- Egy több százezer üres táblázatcellát tartalmazó Markdown-dokumentum előnézetben vagy egy ügynök válaszában már nem visz el másodperceket és gigabájtnyi memóriát: 500 000 üres cella felett a táblázatai szövegként jelennek meg.
- A terminál már nem mulasztja el programja végét, és nem veszíti el annak utolsó kimenetét, ha a Mac terhelt.

## Fejlesztések
- Az ügynök előzményei kihagyják a hibátlanul lefutott vagy még futó hookokat; a sikertelen hook továbbra is megjelenik.
