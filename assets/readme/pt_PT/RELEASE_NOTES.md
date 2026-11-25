O Tapa 1.0.3 abre espaços de trabalho noutros computadores através de SSH e sugere comandos enquanto escreve.

## Novidades
- Espaços de trabalho remotos: "Novo espaço de trabalho" oferece uma pasta noutro computador: um anfitrião do `~/.ssh/config` ou um endereço, porta, utilizador e palavra-passe quando necessário, e a pasta lá. Os terminais e os agentes lá instalados são executados nesse computador através de uma única ligação SSH. A palavra-passe pode ficar guardada no porta-chaves, e uma chave de anfitrião desconhecida nunca é aceite sem confirmação. A lista de ficheiros, o painel do Git, o editor e as pré-visualizações não estão disponíveis lá.
- A barra lateral mostra a ligação de um espaço de trabalho remoto com "Voltar a ligar", "Desligar" e "Editar ligação". Quando uma ligação perdida regressa, os terminais reiniciam e os separadores de agente continuam a conversa.
- Sugestões enquanto escreve: a partir do segundo carácter, o resto de um comando sugerido aparece esbatido após o cursor, e → aceita-o. Tab aceita uma sugestão certa ou abre um menu de conclusão que descreve cada candidato, com ou sem o campo de entrada.
- Cores dos comandos: comandos, subcomandos, opções, variáveis e operadores recebem as cores do tema, e um comando que a shell não consegue executar fica com um sublinhado vermelho tracejado. "Definições › Entrada" desativa as sugestões e as cores.
- Num separador de agente, os botões de subagentes e processos em segundo plano e /tasks abrem uma vista de atividade com o estado e o tempo de execução de cada item, com "Mostrar" e "Parar".

## Melhorias
- "Definições › Geral › Janela" permite desativar a mudança de nome de um espaço de trabalho com um clique no nome.
- A barra lateral mostra o número de ficheiros alterados, o botão do Git e o botão da pasta por esta ordem, e o botão do Git apenas dentro de um repositório Git.
