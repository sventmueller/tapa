O Tapa 1.0.0 é a primeira versão distribuída como imagem de disco: abra `Tapa-1.0.0.dmg` e arraste o Tapa para a pasta Aplicativos. O Tapa 0.1.0 se atualiza sozinho para ela, e o Homebrew instala a mesma imagem.

## Novidades
- Fechar a última janela do Tapa encerra o Tapa e mantém tudo: espaços de trabalho, abas, divisões, histórico e sessões de agente voltam na próxima inicialização como você os deixou. O Tapa só pergunta antes quando ainda roda um programa que não seja um shell.
- Arraste os espaços de trabalho na barra lateral para qualquer ordem; o Terminal sempre fica no topo.
- Uma janela nova abre com o tamanho e a largura da barra lateral da última janela que você redimensionou, também quando o Tapa não restaura nenhuma janela, e o Tapa lembra se o campo de entrada está visível.
- Quando o Tapa inicia com a barra lateral recolhida, o botão Novo Espaço de Trabalho continua oculto.
- A saída que um programa escreve em rajada aparece quando o programa faz uma pausa, no máximo 8 ms depois, de modo que telas desenhadas pela metade não piscam mais.
- Toda sessão roda no núcleo de terminal próprio do Tapa; o ajuste que trocava para o núcleo do Alacritty não existe mais.
- Pasta e branch acima do campo de entrada são texto cinza simples, alinhado com o texto de espaço reservado.
- Em uma aba de agente, `/resume` lista as sessões anteriores do agente nesta pasta e continua uma delas em uma aba própria.
- Uma lista longa na sobreposição de uma aba de agente rola em vez de deixar a janela mais alta.
- Um agente só é considerado concluído após três segundos de inatividade, então seu status não pisca mais entre as etapas.
- Em tela cheia, a barra lateral fica abaixo da barra de ferramentas e desce junto com ela quando a barra de menus aparece.

## O Tapa em resumo
- Espaços de trabalho em uma barra lateral, cada um com suas abas, divisões, agentes, editores e abas de navegador; janelas e sessões de agente voltam depois de reiniciar.
- Abas de agente para Claude, Codex, OpenCode e qualquer agente ACP do registro, instalados, atualizados e revertidos pelos ajustes.
- Painéis de arquivos e Git, pré-visualizações de Markdown e de imagens, um campo de entrada de comandos com ditado.
- Aparência clara, escura ou do sistema, quatro tamanhos de fonte, 35 idiomas de interface.
