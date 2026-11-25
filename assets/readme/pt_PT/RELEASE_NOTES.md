O Tapa 1.0.2 utiliza menos memória quando há separadores em segundo plano.

## Melhorias
- Os separadores de terminal ocultos libertam os buffers do Metal e os separadores de agentes ocultos libertam as vistas das conversas. As sessões de terminal, o histórico de deslocamento e as conversas são preservados.
- Mudar de separador, deslocar o conteúdo e redimensionar a janela reduz a carga na thread principal. A interface continua a responder durante o restauro de um espaço de trabalho.
- O Claude e o Codex processam pedidos em fila e respostas de forma mais fiável.
