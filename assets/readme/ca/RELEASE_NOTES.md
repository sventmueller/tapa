El Tapa 1.0.0 és la primera versió que es distribueix com a imatge de disc: obre `Tapa-1.0.0.dmg` i arrossega el Tapa a la carpeta Aplicacions. El Tapa 0.1.0 s’hi actualitza sol, i Homebrew instal·la la mateixa imatge.

## Novetats
- Tancar l’última finestra del Tapa tanca el Tapa i ho conserva tot: els espais de treball, les pestanyes, les divisions, l’historial i les sessions d’agent tornen a l’inici següent tal com els vas deixar. El Tapa només pregunta abans si encara s’executa un programa que no sigui un shell.
- Arrossega els espais de treball de la barra lateral a l’ordre que vulguis; el Terminal sempre queda a dalt de tot.
- Una finestra nova s’obre amb la mida i l’amplada de la barra lateral de l’última finestra que has redimensionat, també quan el Tapa no restaura cap finestra, i el Tapa recorda si el camp d’entrada es mostra.
- Quan el Tapa s’inicia amb la barra lateral plegada, el botó Espai de treball nou queda amagat.
- La sortida que un programa escriu d’un cop es mostra quan el programa fa una pausa, com a molt 8 ms després, de manera que les pantalles dibuixades a mitges ja no parpellegen.
- Cada sessió s’executa amb el nucli de terminal propi del Tapa; la configuració que canviava al nucli d’Alacritty ja no hi és.
- La carpeta i la branca sobre el camp d’entrada són text gris senzill, alineat amb el marcador de posició.
- En una pestanya d’agent, /resume llista les sessions anteriors de l’agent en aquesta carpeta i en continua una en una pestanya pròpia.
- Una llista llarga a la superposició d’una pestanya d’agent es desplaça en lloc de fer la finestra més alta.
- Un agent només compta com a acabat després de tres segons d’inactivitat, de manera que el seu estat ja no parpelleja entre passos.
- A pantalla completa la barra lateral es manté sota la barra d’eines i baixa amb ella quan apareix la barra de menús.

## El Tapa d’un cop d’ull
- Espais de treball en una barra lateral, cadascun amb les seves pestanyes, divisions, agents, editors i pestanyes de navegador; les finestres i les sessions d’agent tornen després d’un reinici.
- Pestanyes d’agent per a Claude, Codex, OpenCode i qualsevol agent ACP del registre, que s’instal·len, s’actualitzen i es reverteixen des de la configuració.
- Panells de fitxers i de Git, previsualitzacions de Markdown i d’imatges, un camp d’entrada d’ordres amb dictat.
- Aparença clara, fosca o la del sistema, quatre mides de lletra, 35 idiomes d’interfície.
