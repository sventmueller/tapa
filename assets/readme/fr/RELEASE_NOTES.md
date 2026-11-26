Tapa 1.0.6 protège contre le Markdown conçu pour épuiser la mémoire et conserve la dernière sortie d'un programme dans le terminal.

## Corrections
- Un document Markdown comportant des centaines de milliers de cellules de tableau vides, dans un aperçu ou dans la réponse d'un agent, ne coûte plus des secondes et des gigaoctets de mémoire : au-delà de 500 000 cellules vides, ses tableaux s'affichent en texte.
- Un terminal ne manque plus la fin de son programme et ne perd plus sa dernière sortie lorsque le Mac est occupé.

## Améliorations
- L'historique de l'agent omet les hooks qui se sont bien déroulés ou qui sont encore en cours ; un hook qui a échoué reste affiché.
