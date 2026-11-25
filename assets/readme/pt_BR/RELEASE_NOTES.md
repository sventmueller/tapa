O Tapa 1.0.3 abre espaços de trabalho em outros computadores via SSH e sugere comandos enquanto você digita.

## Novidades
- Espaços de trabalho remotos: "Novo Espaço de Trabalho" oferece uma pasta em outro computador: um host do `~/.ssh/config` ou um endereço, porta, usuário e senha quando necessário, e a pasta lá. Os terminais e os agentes instalados lá são executados nesse computador por meio de uma única conexão SSH. A senha pode ficar guardada no chaveiro, e uma chave de host desconhecida nunca é aceita sem confirmação. A lista de arquivos, o painel do Git, o editor e as pré-visualizações não estão disponíveis lá.
- A barra lateral mostra a conexão de um espaço de trabalho remoto com "Reconectar", "Desconectar" e "Editar Conexão". Quando uma conexão perdida volta, os terminais reiniciam e as abas de agente continuam a conversa.
- Sugestões enquanto você digita: a partir do segundo caractere, o resto de um comando sugerido aparece esmaecido após o cursor, e → o aceita. Tab aceita uma sugestão certa ou abre um menu de preenchimento que descreve cada candidato, com ou sem o campo de entrada.
- Cores de comandos: comandos, subcomandos, opções, variáveis e operadores recebem as cores do tema, e um comando que o shell não consegue executar ganha um sublinhado vermelho tracejado. "Ajustes › Entrada" desativa sugestões e cores.
- Em uma aba de agente, os botões de subagentes e processos em segundo plano e /tasks abrem uma visualização de atividade com o estado e o tempo de execução de cada item, com "Mostrar" e "Parar".

## Melhorias
- "Ajustes › Geral › Janela" pode desativar a renomeação de um espaço de trabalho com um clique no nome.
- A barra lateral mostra o número de arquivos alterados, o botão do Git e o botão da pasta nesta ordem, e o botão do Git apenas dentro de um repositório Git.
