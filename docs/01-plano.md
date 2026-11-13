# Plano consolidado — Bookferry (fase de planejamento)

> **Nome: Bookferry** (a balsa que leva sua leitura de um dispositivo ao outro). Livre no crates.io e no npm; 5 repositórios no GitHub com o nome, o maior com 1 estrela; sem produto na busca web. Descartados por conflito real: Bookshelf (VitalSource, MyBookshelf), Dogear (leitor de PDF, app iOS, plugin Obsidian), Pageloop (empresa), Bookloop (várias empresas de livros), TomeSync (plugin KOReader), PageSync (recurso Kindle/Sony). Falta checar domínio e marca. A pasta local continua `bookshelf/`.

Consolidação de 3 relatórios (`docs/pesquisa/`), gerados em 13/11/2026 por agentes do agy: Opus 5.5 (sync/servidor), Gemini 4 Argon (stack/renderização), Sonnet 5.5 (produto/capas). Fatos externos vêm dos relatórios; o que eles marcaram como não verificado continua não verificado aqui. **Os relatórios assumiam um servidor central; a decisão 5 abaixo muda a topologia** (ver "Topologia").

## Decisões do Lucas (13/11/2026)

1. Servidor em **Rust**.
2. Quadrinhos/mangá são só uma parcela da biblioteca: RTL/double-page na v1; AniList/ComicVine opcionais.
3. Destaques/notas e TTS **só depois** de o sync estar bom.
4. **Sem VPN nem serviços de terceiros** para o app funcionar.
5. **O "servidor" é a máquina de cada usuário** (o PC). Os livros e o banco ficam lá. O celular fala com o PC pela rede local, e fora de casa por um **relay opcional** em `bookferry.gaspari.dev`.
6. Arquivos: cofre no servidor (o PC), com "baixar neste dispositivo" sob demanda.
7. Nome: Bookferry.

## Topologia (híbrida)

```
 Android (Tauri) ──── mesma rede Wi-Fi: descoberta mDNS, TLS direto ────> PC (app Tauri + servidor Rust embutido)
        │                                                                       ▲   SQLite + cofre de livros
        └── fora de casa: conexão de saída ──> relay (VPS mínima) <── conexão de saída ┘
                                           só repassa bytes cifrados de ponta a ponta
```

- **O PC roda o servidor dentro do app desktop** (ou como serviço systemd/headless). Um único binário Rust.
- **Pareamento por QR code** no PC (nada de conta/senha): o QR leva a chave pública do PC e um segredo de uso único. O celular guarda a chave e **fixa o certificado do PC** (TLS com pinning), então LAN e relay usam a mesma identidade.
- **Relay sem estado**: PC e celular abrem conexão de saída até ele; ele casa as duas pontas pelo ID do PC e repassa bytes. Não guarda livros nem lê o conteúdo (cifrado ponta a ponta). Roda em VPS mínima (Lightsail US$ 5 por mês foi o único preço lido em fonte; alternativas não verificadas). É opcional: sem ele, o sync só acontece na mesma rede.
- **Multiusuário deixa de ser um servidor com várias contas**: cada pessoa roda o próprio Bookferry. O relay é compartilhável (roteia por ID de PC). `user_id` nas tabelas deixa de ser exigência; fica só um ID de instância.
- **Limitações assumidas:** com o **PC desligado**, o celular não sincroniza nem baixa livros; ele guarda as mudanças na fila (outbox) e envia depois. Livros já baixados no celular continuam abrindo. Isso é o preço de "o servidor é a minha máquina".

## Decisões técnicas (mantidas dos relatórios)

| Tema | Decisão | Origem / motivo |
|---|---|---|
| Cliente | **Tauri v2** (Linux + Android) + núcleo em **Rust**; UI web | Frente 2. `foliate-js` (MIT) roda no WebView dos dois sistemas e resolve EPUB/MOBI/AZW3/FB2 com o mesmo motor nos dois lados (protege o CFI). Readest prova a arquitetura, mas é AGPL: não copiar código. Plano B: Flutter + pdfrx. |
| PDF | PDFium (`pdfium-render`) | BSD; MuPDF/Poppler descartados por licença. |
| Quadrinhos | `zip` (CBZ) + `libarchive` (CBR/RAR5, CB7) | BSD/MIT; evita UnRAR proprietário. |
| Identidade do arquivo | **partial MD5 do KOReader + tamanho** (SHA-256 em background) | Padrão de fato (KOSync, Kavita, Komga, Readest); a Frente 2 sugeriu BLAKE3 parcial, escolhi o partial MD5. |
| Posição de leitura | JSON versionado: paginado `page/page_count`; reflowable Locator Readium + CFI + trecho de texto; `percent` sempre | Frente 1. A tradução fica no cliente. |
| Conflitos | Estado por entidade, LWW com **HLC**, cursor `server_seq`, tombstones, outbox offline. Se a posição remota é mais nova mas está **atrás**, o app pergunta | Frente 1. Com a topologia nova, o PC é o árbitro do `server_seq`. |
| Tempo real | Push ao fechar/pausar + pull ao abrir; SSE (ou o canal do relay) para dispositivos abertos. Sem FCM/Google | Basta para "fecho no PC, abro no celular". |
| Compatibilidade KOReader | Adaptador `/kosync/*` na v1 | Frente 1; barato e dá teste de aceitação (`kosync-conformance`). |
| Capas ("Ler a seguir") | Open Library + upload manual + capa embutida no arquivo; Google Books (chave) e AniList depois; **sem Amazon e sem scraping** | Frente 3. Capa é cache privado, não ativo redistribuível. |
| Backup | Litestream não se aplica ao PC; usar `sqlite3 .backup` agendado para uma pasta | Ajuste meu. |

Modelo de dados e rotas detalhados: `docs/pesquisa/01-sync-e-servidor.md` (seções finais; ignore `users`, `invites` e o isolamento por usuário).

## MVP (critério de pronto: "fechei na pág. 50 no PC, o Android abre na 50", também após uso offline)

1. Ler EPUB, PDF e CBZ; lembrar onde parou.
2. Pareamento por QR e sync de posição PC <-> Android na mesma rede, com offline e conflito.
3. Biblioteca em grade (capa, progresso, lido/não lido, busca) e baixar livro do PC sob demanda.
4. "Ler a seguir": digita o nome, escolhe a capa entre candidatos, sincroniza. Fallback: upload manual ou capa gerada.
5. Tema claro/escuro/sépia, fonte, brilho, toque/teclas para virar página.

**v1:** relay para fora de casa, RTL/double-page/lupa, séries via ComicInfo/OPF, CBR/CB7, Google Books/AniList, zonas de toque configuráveis.
**Depois (só com o sync bom):** destaques/notas, TTS, painel-a-painel, smart lists.

## Riscos que o protótipo precisa matar primeiro

1. **Tauri Mobile no Android com arquivo grande via SAF** (descritor `fd` → Rust): CBZ/PDF de >500 MB.
2. **Sensação "Moon+/YAC" em WebView**: zoom/pan de quadrinho a 60 fps no Android. Se falhar, plano B (Flutter).
3. **CFI entre `webkit2gtk` (Linux) e Android WebView**: o mesmo EPUB deve cair no mesmo trecho.
4. **Servidor Rust dentro do app Tauri no Android/Linux e acesso pela LAN**: mDNS e TLS com pinning funcionando no Android (multicast pode ser bloqueado por alguns roteadores).
5. **Relay**: túnel cifrado de ponta a ponta com reconexão estável em rede móvel (o relay é a parte nova e sem referência nos relatórios).
6. **Ordenação de páginas de CBZ** igual nos dois lados (fixtures).
7. **Wayland/NVIDIA** (RTX 3060 híbrida): tela branca do WebKitGTK/DMABUF no niri.
8. Qualidade pt-BR das capas no Open Library: medir em ~30 títulos (amostra foi de 4).

## Pontos em que desconfio dos relatórios

- Versões da Frente 2 (Flutter 3.47, CMP 1.12, Electron 34 etc.) não foram conferidas.
- Números de desempenho (60–120 fps) são estimativas.
- A Frente 1 não mediu RAM de servidor nem preço de EC2/Hetzner. Nenhum relatório cobriu relay/NAT traversal: é design meu, sem pesquisa.

## Perguntas em aberto

1. DNS de `bookferry.gaspari.dev`: você cria o registro à mão ou eu preparo por terminal (Cloudflare/Route 53)? Só é preciso quando o relay entrar (v1).
2. Posso criar o repositório privado `Caspiom/bookferry` e dar push?
3. Próximo passo: spike dos riscos 1–4 em um projeto Tauri descartável. Posso rodar com 1 agente do agy?
