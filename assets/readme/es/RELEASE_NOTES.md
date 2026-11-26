Tapa 1.0.6 protege contra Markdown creado para agotar la memoria y conserva la última salida de un programa en el terminal.

## Correcciones
- Un documento Markdown con cientos de miles de celdas de tabla vacías, en una previsualización o en la respuesta de un agente, ya no cuesta segundos y gigabytes de memoria: a partir de 500.000 celdas vacías sus tablas se muestran como texto.
- Un terminal ya no pasa por alto el final de su programa ni pierde su última salida cuando el Mac está ocupado.

## Mejoras
- El historial del agente omite los hooks que se ejecutaron bien o que siguen ejecutándose; un hook que falló se sigue mostrando.
