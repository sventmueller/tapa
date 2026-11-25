Tapa 1.0.2 usa menos memoria cuando hay pestañas en segundo plano.

## Mejoras
- Las pestañas ocultas del terminal liberan sus búferes de Metal y las de los agentes liberan las vistas de conversación. Se conservan las sesiones de terminal, el historial de desplazamiento y las conversaciones.
- Cambiar de pestaña, desplazarse y redimensionar la ventana requieren menos trabajo del hilo principal. La interfaz sigue respondiendo al restaurar un espacio de trabajo.
- Claude y Codex gestionan las solicitudes en cola y las respuestas con mayor fiabilidad.
