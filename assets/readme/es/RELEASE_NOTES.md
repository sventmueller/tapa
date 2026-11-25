Tapa 1.0.3 abre espacios de trabajo en otros ordenadores por SSH y sugiere comandos mientras escribes.

## Novedades
- Espacios de trabajo remotos: «Nuevo espacio de trabajo» ofrece una carpeta en otro ordenador: un host de `~/.ssh/config` o una dirección, el puerto, el usuario y la contraseña si hace falta, y la carpeta allí. Los terminales y los agentes instalados allí se ejecutan en ese ordenador a través de una sola conexión SSH. La contraseña puede guardarse en el llavero, y una clave de host desconocida nunca se acepta sin confirmación. La lista de archivos, el panel de Git, el editor y las previsualizaciones no están disponibles allí.
- La barra lateral muestra la conexión de un espacio de trabajo remoto con «Volver a conectar», «Desconectar» y «Editar conexión». Cuando vuelve una conexión perdida, los terminales se inician de nuevo y las pestañas de agente continúan su conversación.
- Sugerencias mientras escribes: desde el segundo carácter, el resto de un comando sugerido aparece atenuado tras el cursor, y → lo acepta. Tab acepta una sugerencia segura o abre un menú de compleción que describe cada candidato, con o sin el campo de entrada.
- Colores de comandos: los comandos, subcomandos, opciones, variables y operadores toman los colores del tema, y un comando que la shell no puede ejecutar aparece subrayado con una línea roja discontinua. «Ajustes › Entrada» desactiva las sugerencias y los colores.
- En una pestaña de agente, los botones de subagentes y procesos en segundo plano y /tasks abren una vista de actividad con el estado y el tiempo de ejecución de cada elemento, con «Mostrar» y «Detener».

## Mejoras
- «Ajustes › General › Ventana» permite desactivar el cambio de nombre de un espacio de trabajo al hacer clic en su nombre.
- La barra lateral muestra el número de archivos modificados, el botón de Git y el botón de carpeta en este orden, y el botón de Git solo dentro de un repositorio Git.
