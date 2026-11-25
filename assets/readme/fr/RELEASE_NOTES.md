Tapa 1.0.0 est la première version distribuée sous forme d’image disque : ouvrez `Tapa-1.0.0.dmg` et faites glisser Tapa sur le dossier Applications. Tapa 0.1.0 se met à jour tout seul vers cette version, et Homebrew installe la même image.

## Nouveautés
- Fermer la dernière fenêtre de Tapa quitte Tapa et conserve tout : les espaces de travail, les onglets, les divisions, l’historique et les sessions d’agent reviennent au prochain démarrage tels que vous les avez laissés. Tapa ne demande confirmation que si un programme autre qu’un shell tourne encore.
- Faites glisser les espaces de travail de la barre latérale dans l’ordre de votre choix ; « Terminal » reste toujours tout en haut.
- Une nouvelle fenêtre s’ouvre avec la taille et la largeur de barre latérale de la dernière fenêtre redimensionnée, même quand Tapa ne restaure aucune fenêtre, et Tapa retient si le champ de saisie est affiché.
- Quand Tapa démarre avec la barre latérale repliée, le bouton Nouvel espace de travail reste masqué.
- La sortie qu’un programme écrit par rafales s’affiche dès que le programme marque une pause, au plus 8 ms plus tard, si bien que les écrans à moitié dessinés ne clignotent plus.
- Chaque session tourne sur le cœur de terminal propre à Tapa ; le réglage qui basculait vers le cœur d’Alacritty n’existe plus.
- Le dossier et la branche au-dessus du champ de saisie sont du simple texte gris, aligné sur le texte indicatif.
- Dans un onglet d’agent, /resume liste les sessions passées de l’agent dans ce dossier et en poursuit une dans un onglet à part.
- Une longue liste dans la surcouche d’un onglet d’agent défile au lieu d’agrandir la fenêtre en hauteur.
- Un agent n’est considéré comme terminé qu’après trois secondes d’inactivité, si bien que son état ne clignote plus entre deux étapes.
- En plein écran, la barre latérale reste sous la barre d’outils et descend avec elle quand la barre des menus s’affiche.

## Tapa en bref
- Des espaces de travail dans une barre latérale, chacun avec ses onglets, ses divisions, ses agents, ses éditeurs et ses onglets de navigateur ; les fenêtres et les sessions d’agent reviennent après un redémarrage.
- Des onglets d’agent pour Claude, Codex, OpenCode et tout agent ACP du registre, installés, mis à jour et ramenés à une version antérieure depuis les réglages.
- Des panneaux Fichiers et Git, des aperçus Markdown et d’images, un champ de saisie de commandes avec dictée.
- Apparence claire, sombre ou celle du système, quatre tailles de police, 35 langues d’interface.
