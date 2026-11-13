# Plano consolidado — Bookshelf (fase de planejamento)

Consolidação de 3 relatórios (`docs/pesquisa/`), gerados em 13/11/2026 por agentes do agy: Opus 5.5 (sync/servidor),
Gemini 4 Argon (stack/renderização), Sonnet 5.5 (produto/capas). Fatos externos (versões, preços, limites) vêm dos relatórios;
o que eles marcaram como não verificado continua não verificado aqui.

## Decisões propostas

| Tema | Decisão | Origem / motivo |
|---|---|---|
| Cliente | **Tauri v2** (Linux + Android) + núcleo em **Rust**; UI web | Frente 2. `foliate-js` (MIT) roda no WebView dos dois sistemas e resolve EPUB/MOBI/AZW3/FB2 com o **mesmo motor** nos dois lados, o que protege a precisão do CFI no sync. Readest prova a arquitetura (mas é AGPL: não copiar código). Plano B: Flutter + pdfrx. |
| PDF | PDFium (`pdfium-render`) | BSD; MuPDF/Poppler descartados por licença (AGPL/GPL). |
| Quadrinhos | `zip` (CBZ) + `libarchive` (CBR/RAR5, CB7) | BSD/MIT; evita UnRAR proprietário. |
| Servidor | Binário único + **SQLite**, API REST + SSE, adaptador **KOSync** | Frente 1. Linguagem **a confirmar: Rust ou Go** (ver pergunta 1). |
| Identidade do arquivo | **partial MD5 do KOReader + tamanho** (SHA-256 em background) | Frente 1 sugeriu partial MD5, Frente 2 sugeriu BLAKE3 parcial. **Escolho partial MD5**: é padrão de fato (KOSync, Kavita, Komga, Readest), com vetores de teste públicos. |
| Posição de leitura | JSON versionado: paginado `page/page_count`; reflowable Locator Readium + CFI + trecho de texto; `percent` sempre | Frente 1. O servidor não interpreta, só guarda. |
| Conflitos | Estado por entidade, LWW com **HLC**, cursor `server_seq`, tombstones, outbox offline. Se a posição remota é mais nova mas está **atrás**, o app pergunta | Frente 1 (lições dos bugs de relógio do Readest). |
| Tempo real | Push ao fechar/pausar + pull ao abrir; SSE para dispositivos abertos. Sem FCM na v1 | Basta para "fecho no PC, abro no celular". |
| Arquivos dos livros | **Fora do servidor na v1** (Syncthing/pasta/OPDS); cofre por SHA-256 reservado e desligado | Evita peso, e risco de direitos autorais ao abrir para outros. |
| Capas ("Ler a seguir") | Open Library + upload manual + capa embutida no arquivo; Google Books (chave) e AniList depois; **sem Amazon e sem scraping** | Frente 3. Capa tratada como cache privado, não como ativo redistribuível. |
| Multiusuário | `user_id` em toda tabela e consulta desde o dia 1, argon2id, tokens por dispositivo, cadastro só por convite | Frentes 1 e 3. |
| Hospedagem | Casa + Tailscale (US$ 0) para começar; plano B Lightsail US$ 5 com OpenTofu; backup Litestream; Docker Compose + módulo NixOS | Frente 1. Preços AWS/Hetzner: só o Lightsail foi lido na fonte. |

## Arquitetura (resumo)

```
 Tauri app (Linux / Android)                         Servidor (1 binário + SQLite)
 ┌───────────────────────────────┐   REST + SSE     ┌────────────────────────────┐
 │ UI web: biblioteca, lista,    │ <──────────────> │ /v1/sync/push|changes      │
 │ leitor foliate-js / canvas    │                  │ /v1/progress, /reading-list│
 │ Núcleo Rust: SQLite local,    │                  │ /v1/events (SSE)           │
 │ outbox+HLC, PDFium, zip/      │                  │ /kosync/* (compat. KOReader)│
 │ libarchive, hash, capas       │                  │ auth por usuário/dispositivo│
 └───────────────────────────────┘                  └────────────────────────────┘
```
Modelo de dados e rotas detalhados: `docs/pesquisa/01-sync-e-servidor.md` (seções finais).

## MVP (critério de pronto: "fechei na pág. 50 no PC, o Android abre na 50", também após uso offline)

1. Ler EPUB, PDF e CBZ; lembrar onde parou.
2. Sync de posição entre PC e Android com offline e conflito.
3. Biblioteca em grade (capa, progresso, lido/não lido, busca).
4. "Ler a seguir": digita o nome, escolhe a capa entre candidatos, sincroniza. Fallback: upload manual ou capa gerada.
5. Tema claro/escuro/sépia, fonte, brilho, toque/teclas para virar página.
6. Servidor self-hosted, 1 usuário, mas multiusuário-ready.

**v1:** RTL/double-page/lupa, séries via ComicInfo/OPF, CBR/CB7, marcadores/destaques, TTS, Google Books/AniList.
**Depois:** convites/cotas e revisão legal (multiusuário), cofre de arquivos, painel-a-painel, smart lists.

## Riscos que o protótipo precisa matar primeiro (spike antes de construir)

1. **Tauri Mobile no Android com arquivo grande via SAF** (descritor `fd` → Rust): abrir CBZ/PDF de >500 MB com fluidez.
2. **Sensação "Moon+/YAC" em WebView**: zoom/pan de quadrinho a 60 fps no Android. Se falhar, o plano B (Flutter) entra.
3. **CFI entre `webkit2gtk` (Linux) e Android WebView**: o mesmo EPUB deve cair no mesmo trecho.
4. **Ordenação de páginas de CBZ** igual nos dois lados (fixtures de teste).
5. **Wayland/NVIDIA** (sua RTX 3060 híbrida): tela branca do WebKitGTK/DMABUF no niri.
6. Qualidade pt-BR das capas no Open Library: medir em ~30 títulos (a amostra foi de 4).

## Pontos em que desconfio dos relatórios

- Versões citadas pela Frente 2 (Flutter 3.47, CMP 1.12, Electron 34 etc.) **não foram conferidas por mim**; confirmar na hora de decidir.
- Afirmações de desempenho (60–120 fps, KB/ms) são estimativas, não medições: só o spike valida.
- A Frente 1 não mediu RAM de Go/SQLite nem preço de EC2/Hetzner.

## Perguntas para você

1. **Linguagem do servidor:** Rust (compartilha tipos/HLC/posição com o app, ciclo de build mais lento) ou Go (mais simples)? Recomendo Rust pelo código compartilhado.
2. **Arquivos:** você já usa (ou aceita usar) Syncthing/pasta para levar os arquivos entre PC e celular, ou quer que o app baixe do servidor?
3. **Biblioteca:** quadrinhos/mangá são maioria? (decide AniList/ComicVine e o peso do modo RTL/double-page)
4. **Escopo v1:** destaques/notas de texto e TTS entram cedo ou só depois do sync estar redondo?
5. **Tailscale no Android:** você já usa outra VPN no celular? (o Android só aceita uma ativa)
6. **Nome** do projeto (hoje "Bookshelf" é provisório) e se crio repositório privado no GitHub (Caspiom).
