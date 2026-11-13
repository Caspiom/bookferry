# Briefing — Lombada

## Ideia
Leitor de livros e quadrinhos para PC (Linux/NixOS, também ideal Windows/macOS) e Android, com sincronização
automática do progresso: se fechar um livro na página 50 no PC, o app Android abre na página 50 (e vice-versa).
Experiência de leitura inspirada em **Moon+ Reader** (Android) e **YACReader** (PC, quadrinhos) — o usuário gosta
das features e da usabilidade dos dois.

## Requisitos
1. Formatos: PDF, EPUB, e os populares de quadrinhos (CBZ, CBR, talvez CB7/CBT) e outros populares (MOBI/AZW3, FB2...) — avaliar o que é viável.
2. Sync de posição de leitura entre dispositivos (PC <-> Android), tolerante a offline e a conflitos; posição precisa
   em formatos reflowable (EPUB: CFI/locator) e paginados (PDF/CBZ: nº de página).
3. Lista "Ler a seguir": usuário digita o nome do livro; o app busca a capa na web e adiciona nome + capa à lista.
   A lista também deve sincronizar.
4. Biblioteca: organização, metadados, capas, progresso, coleções/séries (comum em quadrinhos).
5. Servidor de sync **self-hosted simples em Rust**, alcançável por HTTPS público, **sem depender de VPN ou serviços externos** (VPS barata). Preferência por IaC/terminal.
6. Uso pessoal agora (1 usuário, vários dispositivos), mas **com possibilidade de abrir para outros usuários depois** —
   não bloquear essa evolução (auth, isolamento por usuário no modelo de dados).
7. Stack de cliente: **em aberto** — comparar Flutter, Kotlin Multiplatform, Tauri + Android nativo, React Native etc.

## Restrições
- Sem custo recorrente pago além de infra mínima (VPS/AWS pequena). Sem APIs pagas obrigatórias. Orçamento: baixo.
- Os arquivos dos livros: decidir se o servidor guarda os arquivos ou só metadados/posição (comparar; identificação do livro entre
  dispositivos por hash de conteúdo é uma hipótese a avaliar).
- Atenção a direitos autorais/ToS das fontes de capa (ver licenças das APIs).

## Fase atual
**Planejamento.** Nenhum código ainda. Cada pesquisador entrega UM relatório em Markdown, em português (pt-BR), com
recomendações objetivas, tabelas de comparação, prós/contras, riscos e fontes (links). Seja específico e honesto sobre o que
não conseguiu verificar. Não invente números de versão, endpoints ou licenças: confirme na web.
