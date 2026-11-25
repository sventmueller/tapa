O Tapa 1.0.2 usa menos memória quando há abas em segundo plano.

## Melhorias
- As abas ocultas do terminal liberam os buffers do Metal, e as abas ocultas dos agentes liberam as visualizações de conversa. As sessões do terminal, o histórico de rolagem e as conversas são preservados.
- Alternar abas, rolar e redimensionar a janela exige menos da thread principal. A interface continua respondendo durante a restauração de um espaço de trabalho.
- Claude e Codex processam solicitações na fila e respostas com mais confiabilidade.
