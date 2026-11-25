O Tapa 1.0.0 é a primeira versão distribuída como imagem de disco: abra `Tapa-1.0.0.dmg` e arraste o Tapa para a pasta Aplicações. O Tapa 0.1.0 atualiza-se sozinho para esta versão, e o Homebrew instala a mesma imagem.

## Novidades
- Fechar a última janela do Tapa termina o Tapa e mantém tudo: espaços de trabalho, separadores, divisões, histórico e sessões de agente regressam no próximo arranque como os deixou. O Tapa só pergunta antes quando ainda corre um programa que não seja uma shell.
- Arraste os espaços de trabalho na barra lateral para qualquer ordem; o Terminal fica sempre no topo.
- Uma janela nova abre com o tamanho e a largura da barra lateral da última janela que redimensionou, também quando o Tapa não restaura nenhuma janela, e o Tapa memoriza se o campo de entrada está visível.
- Quando o Tapa arranca com a barra lateral recolhida, o botão Novo espaço de trabalho continua oculto.
- A saída que um programa escreve em rajada aparece quando o programa faz uma pausa, no máximo 8 ms depois, de modo que os ecrãs desenhados a meio já não piscam.
- Cada sessão corre no núcleo de terminal próprio do Tapa; a definição que mudava para o núcleo do Alacritty deixou de existir.
- Pasta e ramo acima do campo de entrada são texto cinzento simples, alinhado com o texto de espaço reservado.
- Num separador de agente, `/resume` lista as sessões anteriores do agente nesta pasta e continua uma delas num separador próprio.
- Uma lista longa na sobreposição de um separador de agente desloca-se em vez de aumentar a altura da janela.
- Um agente só é considerado concluído após três segundos de inatividade, pelo que o seu estado já não pisca entre os passos.
- Em ecrã completo, a barra lateral fica abaixo da barra de ferramentas e desce com ela quando a barra de menus aparece.

## O Tapa em resumo
- Espaços de trabalho numa barra lateral, cada um com os seus separadores, divisões, agentes, editores e separadores de navegador; as janelas e as sessões de agente regressam depois de um reinício.
- Separadores de agente para Claude, Codex, OpenCode e qualquer agente ACP do registo, instalados, atualizados e revertidos a partir das definições.
- Painéis de ficheiros e Git, pré-visualizações de Markdown e de imagens, um campo de entrada de comandos com ditado.
- Aspeto claro, escuro ou do sistema, quatro tamanhos de letra, 35 idiomas de interface.
