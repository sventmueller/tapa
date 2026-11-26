O Tapa 1.0.6 protege contra Markdown criado para esgotar a memória e mantém a última saída de um programa no terminal.

## Correções
- Um documento Markdown com centenas de milhares de células de tabela vazias, em uma pré-visualização ou na resposta de um agente, não leva mais segundos nem gigabytes de memória: acima de 500.000 células vazias, as tabelas aparecem como texto.
- Um terminal não perde mais o fim do seu programa nem a última saída dele quando o Mac está ocupado.

## Melhorias
- O histórico do agente omite hooks que foram executados sem erros ou que ainda estão em execução; um hook que falhou continua aparecendo.
