Tapa 1.0.3 ouvre des espaces de travail sur d'autres ordinateurs via SSH et suggère des commandes pendant la saisie.

## Nouveautés
- Espaces de travail distants : « Nouvel espace de travail » propose un dossier sur un autre ordinateur : un hôte de `~/.ssh/config` ou une adresse, le port, l'utilisateur et le mot de passe si nécessaire, et le dossier là-bas. Les terminaux et les agents qui y sont installés s'exécutent sur cet ordinateur via une seule connexion SSH. Le mot de passe peut être conservé dans le trousseau, et une clé d'hôte inconnue n'est jamais acceptée sans confirmation. La liste des fichiers, le panneau Git, l'éditeur et les aperçus n'y sont pas disponibles.
- La barre latérale affiche la connexion d'un espace de travail distant avec « Reconnecter », « Se déconnecter » et « Modifier la connexion ». Quand une connexion perdue revient, les terminaux redémarrent et les onglets d'agent poursuivent leur conversation.
- Suggestions pendant la saisie : dès le deuxième caractère, la suite d'une commande suggérée apparaît en grisé après le curseur, et → l'accepte. Tab accepte une suggestion sûre ou ouvre un menu de complétion qui décrit chaque candidat, avec ou sans le champ de saisie.
- Couleurs des commandes : les commandes, sous-commandes, options, variables et opérateurs prennent les couleurs du thème, et une commande que le shell ne peut pas exécuter est soulignée d'un trait rouge en pointillés. « Réglages › Saisie » désactive les suggestions et les couleurs.
- Dans un onglet d'agent, les boutons des sous-agents et des processus en arrière-plan ainsi que /tasks ouvrent une vue d'activité avec l'état et la durée d'exécution de chaque élément, avec « Afficher » et « Arrêter ».

## Améliorations
- « Réglages › Général › Fenêtre » permet de désactiver le renommage d'un espace de travail par un clic sur son nom.
- La barre latérale affiche le nombre de fichiers modifiés, le bouton Git et le bouton du dossier dans cet ordre, et le bouton Git uniquement dans un dépôt Git.
