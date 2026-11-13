# Frente 1: arquitetura de sincronização e servidor

> Pesquisa feita em 13/11/2026. Versões, licenças e datas de commit foram conferidas na API do GitHub e nas páginas oficiais no mesmo dia. O que não consegui confirmar está marcado com ⚠️.

> A recomendação consolidada está no fim do documento.

---

## (a) Posição de leitura por formato

| Formato | Posição "nativa" | Campos que vamos gravar | Observações |
|---|---|---|---|
| PDF | página | `page` (0-based), `page_count`, `percent` | Exata entre dispositivos se o arquivo for o mesmo. Zoom/scroll dentro da página é opcional (`fragments: ["page=12","viewrect=…"]`, sintaxe do Readium). |
| CBZ/CBR/CB7/CBT | índice da imagem | `page`, `page_count`, `percent` | A ordem das páginas **tem que ser determinística**: ordenação natural dos nomes, ignorando `__MACOSX` e afins. Em página dupla, grava a primeira. |
| EPUB | CFI / Locator | `href`, `progression`, `totalProgression`, `position`, `cfi`, `text{before,highlight,after}`, `percent` | Modelo da spec Readium Locator. O Komga já armazena esse formato (`R2Progression` = `modified` + `device` + `locator`). |
| MOBI/AZW3/FB2 | depende do motor | Locator, se o motor converter para HTML; senão só `percent` | O KOReader usa XPointer do CREngine, não CFI. |

**Por que não ficar só no CFI:** o CFI aponta para um índice no DOM, e cada motor (foliate-js, Readium, epub.js, CREngine) normaliza o DOM de um jeito. Um CFI gerado num motor pode cair em outro lugar noutro. O Readest converte o XPointer do KOReader em CFI local justamente porque o `percentage` do KOReader **não é comparável** com o seu.

**Ordem de resolução no dispositivo que recebe** (do mais preciso para o fallback):
1. Mesmo arquivo e mesmo motor: `cfi` (EPUB) ou `page` (paginado).
2. Mesmo arquivo, outro motor: `href` + busca do trecho de `text` dentro do capítulo, depois `href` + `progression`.
3. Arquivo diferente da mesma obra (outra edição): `totalProgression` ou `percent`. A posição fica aproximada e a UI avisa.
4. PDF/CBZ com `page_count` diferente (outro scan): `round(percent × page_count_local)`.

**O servidor não interpreta a posição:** guarda o JSON como veio, mais `percent` numérico. A tradução fica toda no cliente.

## (b) Identificar o mesmo livro em vários dispositivos

O **partial MD5 do KOReader** (`util.partialMD5`, em `frontend/util.lua`) lê blocos de 1 KiB nos offsets `1024 << 2i` para i = −1…10, o que dá 0, 1K, 4K, 16K, 64K, 256K, 1M, 4M, 16M, 64M, 256M e 1G, e calcula o MD5 desses blocos. O Readest tem a mesma função em TypeScript (`partialMD5`), e existem vetores de teste públicos em `pid1/kosync-conformance/vectors`.

| Método | Custo | Robustez | Prós | Contras |
|---|---|---|---|---|
| **Partial MD5 (KOReader)** | ~12 leituras de 1 KiB | Muda se os bytes amostrados mudarem | Instantâneo, mesmo em CBZ de 1 GB. É o **padrão de fato**: KOSync, Kavita (≥0.8.7), Komga (modo "Binary") e Readest usam. | Colisão teórica entre arquivos que só diferem fora das amostras (raro). Qualquer reescrita do arquivo (Calibre, edição de metadados) gera outro hash. |
| SHA-256 do arquivo inteiro | Lê o arquivo todo | Igual ao anterior, sem colisão prática | Integridade, deduplicação e chave natural para guardar blobs | Lento em arquivos grandes no Android. Tem que rodar em background, uma única vez. |
| MD5 do nome do arquivo (modo "Filename" do KOSync) | Zero | Quebra ao renomear | Sobrevive a mudanças no conteúdo | Colide fácil ("capitulo1.pdf") |
| ISBN / identificadores do OPF | Precisa fazer parse | Igual entre edições do mesmo ISBN | Junta arquivos diferentes da mesma obra | Quadrinhos e PDFs quase nunca têm. Às vezes o ISBN é do impresso. |
| Hash de metadados (o `meta_hash` do Readest: `md5(título\|autores\|ids)`) | Precisa fazer parse | Médio | Junta cópias diferentes | Metadados genéricos colidem. O Readest passou a acrescentar o nome do arquivo no caso de PDF depois de um bug (issue #5411). |

**Recomendação:** dois níveis de identidade.
- **`book_files`:** `partial_md5` + `size_bytes` + `format` (`sha256` calculado depois). Casa o progresso **exato**.
- **`books`** (obra lógica, por usuário): agrupa arquivos automaticamente pelo ISBN; se só título+autor batem, pergunta ao usuário.
- O progresso fica no **livro**, com o `file_id` de origem, para o cliente saber se a posição é exata ou aproximada.
- O `book_id` é **carimbado uma vez** e nunca recalculado (lição do Readest).

## (c) Modelo de sync e resolução de conflitos

| Opção | Complexidade | Serve para nós? |
|---|---|---|
| LWW com timestamp do cliente | Baixa | Não sozinho: um relógio adiantado "trava" o sync. O Readest teve isso nas issues #5661 (um relógio 1 h adiantado bloqueou o progresso por 1 h) e #6544 (uma nota apagada por mistura de relógio do servidor com relógio do cliente). |
| **LWW com HLC + cursor do servidor** | Baixa/média | **Sim.** O HLC é `max(relógio físico, último HLC visto + 1)`, então nunca anda para trás. O cursor `seq` do servidor nunca depende do relógio do cliente. |
| Vector clocks | Média | Desnecessário: são poucos dispositivos e quase todo conflito se resolve com "o mais recente vence" |
| CRDT (Automerge/Yjs) | Alta | Exagero. Talvez só para anotações no futuro. |
| Log de eventos (event sourcing) | Média | Útil como **histórico** (estatísticas, desfazer), não como fonte da verdade |

**Modelo proposto**
- **Estado por entidade** (progresso, item da lista, coleção), com `hlc`, `device_id`, `deleted_at`, `server_seq`.
- **Push:** mutações da outbox do cliente; o servidor aplica LWW por `hlc` (desempate por `device_id`) e devolve os vencedores.
- **Pull:** `GET /changes?since=<seq>`. **Toda** escrita, inclusive exclusão, recebe `server_seq` monotônico, o que evita filtros `OR deleted_at > since` (armadilha documentada pelo Readest).
- **Progresso tem uma regra a mais:** além do LWW, o cliente compara "mais recente" com "mais adiantado", como o KOReader faz (`sync_forward = PROMPT`, `sync_backward = DISABLE` por padrão). Se a posição remota for mais nova **mas estiver atrás** da local, a UI pergunta em vez de pular sozinha.
- **Histórico:** `progress_events` append-only, com retenção, para estatísticas e "voltar para onde eu estava". Não decide conflitos.
- **Offline-first:** SQLite local + outbox + retry. Sync ao abrir/fechar o livro, ao pausar o app e com debounce de ~25 s (valor do KOReader).
- **Lista "Ler a seguir":** `sort_key` fracionário, para não reescrever a lista a cada movimentação.

## (d) Soluções prontas vs servidor próprio

| Projeto | Última versão (data) | Licença | O que oferece | Reaproveitar? |
|---|---|---|---|---|
| **koreader-sync-server** | v2.1.1 (16/05/2026) | AGPL-3.0 | Rotas `/users/{create,auth,me,password}` e `/syncs/progress[/:document]`; OpenResty + Lua + Redis | **O protocolo, sim; o código, não.** Só tem progresso. Guarda `x-auth-key` (= `md5(senha)`) direto no Redis. Há uma spec CC0 com verificador (`pid1/kosync-conformance`) e ≥9 reimplementações. |
| **Kavita** | v0.9.1.4 (02/09/2026) | GPL-3.0 | Servidor de biblioteca (mangá, quadrinhos, EPUB, PDF), OPDS, KOReader sync, Kobo sync. Tem um plano pago opcional (Kavita+). | Integração opcional. Exige que o servidor tenha os arquivos. Pesado para o que precisamos. |
| **Komga** | 1.30.1 (10/11/2026) | MIT | Foco em quadrinhos, OPDS, Kobo sync, KOReader sync (API key, modo Binary). Progresso no formato Readium (`R2Progression`). | Boa **referência de modelo**. Limitação que a própria documentação admite: no EPUB, o progresso KOReader↔Komga fica **no início do capítulo**. |
| **Calibre-Web** | 0.6.27 (08/08/2026) | GPL-3.0 | Interface web para o banco do Calibre, Kobo sync | Não. O fork **Calibre-Web-Automated** (v4.0.8) tem endpoints KOSync, mas continua amarrado ao Calibre. |
| **Readest** (cliente + nuvem) | v0.12.20 (05/11/2026) | AGPL-3.0 | App Tauri/Next.js; sync de livros, progresso e notas sobre Supabase; fala KOSync; tem Docker self-hosted | **Referência de aprendizado** (notas `.claude/memory/*sync*`). Como backend, traria AGPL + Supabase/Postgres. |
| **Syncthing** | v2.1.7 (21/10/2026) | MPL-2.0 | Sincroniza arquivos P2P | Pode distribuir **os arquivos dos livros** entre dispositivos. Não serve para progresso (conflitos de arquivo). O app Android oficial foi descontinuado; o fork `researchxxl/syncthing-android` continua ativo. |

**Construir:** servidor em **Go + SQLite**. Um binário estático que cabe numa VPS de 512 MB (consumo de RAM ⚠️ não medido) e roda em ARM e x86. Rust (axum) também serve, com ciclo de desenvolvimento mais lento; Node pesa mais. O PocketBase (MIT) aparece como **arquivado** no GitHub ⚠️ (causa não investigada).

**Reaproveitar:** protocolo KOSync como adaptador (`/kosync/*`) e seu verificador, partial MD5 com vetores de teste, Locator do Readium, lições de sync do Readest.

## (e) Transporte

| Mecanismo | Uso | Observações |
|---|---|---|
| **REST + JSON** | Push, pull, CRUD | Base de tudo. Idempotente com `mutation_id` (UUID) gerado no cliente. |
| **SSE** (`GET /v1/events`) | Avisar "tem mudança, faça pull" | Mais simples que WebSocket, passa por proxy e pelo Cloudflare Tunnel. Precisa de *heartbeat* a cada 15–30 s para a conexão parada não ser derrubada. Só é útil com o app em primeiro plano. |
| WebSocket | Alternativa ao SSE | Só compensa se um dia houver tráfego bidirecional de verdade (ex.: leitura simultânea) |
| Polling | Fallback e background | Android: WorkManager (mínimo de 15 min para tarefa periódica). Desktop: timer. |
| FCM | Push com o app fechado | Gratuito, mas exige um projeto Firebase e Google Play Services, e quebra o "100% self-hosted". **Opcional.** Alternativa sem Google: **UnifiedPush** via ntfy (Apache-2.0, v2.37.2). |

**Conclusão:** para "fecha no PC, abre no Android", bastam **push ao fechar/pausar** e **pull ao abrir**. O SSE só melhora o caso de dois dispositivos abertos ao mesmo tempo.

## (f) O servidor guarda os arquivos?

| Opção | Prós | Contras |
|---|---|---|
| **Só metadados e progresso** (v1) | Disco e banda mínimos. Sem risco de direitos autorais ao abrir para outros usuários. Backup em KB/MB. | O usuário tem que copiar os arquivos para cada dispositivo (pasta local, Syncthing, OPDS). |
| Cofre de arquivos (blobs por SHA-256) | "Abre em qualquer lugar", restaura biblioteca | Disco/banda (bibliotecas de quadrinhos passam fácil de dezenas de GB). O **Cloudflare (planos Free/Pro) limita requisições a 100 MB** (CBZ grande precisa de upload em partes). Abrir para terceiros vira hospedagem de conteúdo de terceiros (DMCA/LGPD). |
| Delegar (Syncthing, OPDS de Kavita/Komga/Calibre) | Reaproveita o que o usuário já usa | Mais uma peça para configurar |

**Recomendação:** v1 só com metadados, mais importação via OPDS e compatibilidade com pastas do Syncthing. A rota `/v1/blobs/{sha256}` (upload em partes ≤ 50 MB, `HEAD` para deduplicar) fica reservada e **desligada por padrão**. Guardar capas em cache depende do ToS de cada fonte (outra frente).

## (g) Autenticação e modelo multiusuário

- **Desde o dia 1:** `user_id NOT NULL` em toda tabela de dados e em toda consulta, com testes de isolamento entre usuários.
- **Login:** usuário + senha com **argon2id**. Cada **dispositivo** recebe um *refresh token* opaco, guardado só como hash, revogável por dispositivo. O *access token* é curto (JWT de 15 min ou token opaco). O cadastro é **só por convite**, ou fica desligado (`REGISTRATION=invite|closed`).
- **Compatibilidade KOSync:** o KOReader envia `x-auth-user` e `x-auth-key = md5(senha)`. Em vez de reaproveitar a senha principal, gerar uma **"chave de sync" separada por usuário**, guardada com hash. Kavita e Komga fazem algo parecido com API key.
- **Depois:** OIDC, papéis (admin/user), cotas. TLS sempre, *rate limit* no login e no SSE.

## (h) Hospedagem e custo

| Opção | Custo/mês | Prós | Contras |
|---|---|---|---|
| **Home server + Tailscale** | US$ 0 (+ energia) | Plano Personal grátis: 6 usuários, dispositivos ilimitados, sem portas abertas, TLS via `tailscale serve` | Android precisa do app Tailscale ligado (ocupa o slot de VPN). Outros usuários entram no seu tailnet. Servidor precisa ficar ligado. |
| Home server + **Cloudflare Tunnel** | US$ 0 + domínio (~US$ 10/ano ⚠️) | URL pública, sem abrir portas, SSE/WebSocket funcionam | Limite de 100 MB por requisição. O tráfego passa pela Cloudflare (ela termina o TLS). |
| **AWS Lightsail** | **US$ 3,50** (512 MB, só IPv6) / **US$ 5** (512 MB, com IPv4) | Preço fixo, 1 TB de tráfego, snapshots | Na região **São Paulo, a franquia de tráfego é metade**. Só IPv6 pode dar problema em redes móveis/residenciais sem IPv6. |
| AWS EC2 t4g.nano | ⚠️ não verifiquei o preço atual | Ecossistema AWS completo | Cobrança por IPv4 público, EBS e tráfego. É mais fácil errar a conta do que no Lightsail. |
| Hetzner CX23 / CAX11 | ⚠️ ~€ 5,5–6,5 (subiu em 2026; confirmar no console) | 4 GB de RAM, 20 TB de tráfego | Só na Europa/EUA (latência ~200 ms do Brasil, irrelevante para sync) |
| **Oracle Always Free** | US$ 0 | A1 Flex: 1.500 OCPU-h e 9.000 GB-h por mês, equivalente a 2 OCPU / 12 GB | **Instâncias ociosas podem ser recuperadas** (p95 de CPU, rede e memória < 20% por 7 dias). Nosso servidor fica ocioso por natureza. Disponibilidade de capacidade incerta. |

**Deploy e IaC**
- **Docker:** imagem distroless com o binário; Compose com **Caddy** (TLS automático) ou sidecar `cloudflared`/`tailscale` (por *profile*).
- **NixOS:** módulo `services.bookshelf` num flake (systemd com `DynamicUser`).
- **VPS:** **OpenTofu**/Terraform (Lightsail ou Hetzner) + `cloud-init`.
- **Backup:** **Litestream** (Apache-2.0, v0.5.18) para S3, R2 ou B2, ou `sqlite3 .backup` diário.

---

## Esboço do modelo de dados (SQLite)

```text
users(id PK, username UNIQUE, password_hash, kosync_key_hash, role, created_at, disabled_at)
devices(id PK, user_id FK, name, platform, last_seen_at, created_at, revoked_at)
refresh_tokens(id PK, user_id FK, device_id FK, token_hash UNIQUE, expires_at, revoked_at)
invites(code_hash PK, created_by FK, expires_at, used_by FK NULL)

books(id PK uuid, user_id FK, title, authors_json, series_id FK NULL, series_index,
      isbn13 NULL, language, cover_blob_sha NULL, hlc, device_id, deleted_at, server_seq)
book_files(id PK uuid, user_id FK, book_id FK, partial_md5, size_bytes, format,
      sha256 NULL, page_count NULL, filename_hint, hlc, deleted_at, server_seq,
      UNIQUE(user_id, partial_md5, size_bytes))
series(id PK, user_id FK, name, hlc, deleted_at, server_seq)
collections(id PK, user_id FK, name, sort_key, hlc, deleted_at, server_seq)
collection_items(collection_id FK, book_id FK, user_id FK, sort_key, hlc, deleted_at, server_seq)

reading_progress(user_id FK, book_id FK, file_id FK, position_json, percent REAL,
      status ENUM(unread,reading,finished), hlc, device_id, updated_at_server, server_seq,
      PK(user_id, book_id))
progress_events(id PK, user_id, book_id, file_id, position_json, percent, device_id, hlc, created_at) -- append-only, com retenção

reading_list_items(id PK uuid, user_id FK, title, author NULL, cover_url NULL,
      cover_blob_sha NULL, cover_source, linked_book_id NULL, sort_key, hlc, device_id,
      deleted_at, server_seq)

kosync_documents(user_id, document_key, book_id NULL, progress_raw, percentage, device,
      device_id, timestamp, PK(user_id, document_key))   -- camada de compatibilidade
blobs(sha256 PK, size, mime, created_at)  +  user_blobs(user_id, sha256)  -- opcional
seq_counter(id=1, value)   -- fonte do server_seq monotônico
```

Formato de `position_json`:
```json
{"v":1,"kind":"paged","page":49,"page_count":212,"percent":0.2311}
{"v":1,"kind":"reflow","href":"OEBPS/ch05.xhtml","progression":0.42,"totalProgression":0.37,
 "position":118,"cfi":"epubcfi(/6/14!/4/2/10/3:15)","text":{"before":"…","highlight":"…"},
 "engine":"readium-kotlin/3.4.0","percent":0.37}
```

## Esboço da API (v1)

| Método e rota | Função |
|---|---|
| `POST /v1/auth/register` (com convite), `POST /v1/auth/login`, `POST /v1/auth/refresh`, `POST /v1/auth/logout` | Sessão por dispositivo |
| `GET/DELETE /v1/devices[/{id}]` | Listar e revogar dispositivos |
| `GET /v1/sync/changes?since={seq}&limit=500` | Pull incremental de todas as entidades. Devolve `{changes[], next_seq, has_more}`. |
| `POST /v1/sync/push` | Lote de mutações `{mutation_id, entity, id, hlc, op, data}`. Devolve vencedores e conflitos. |
| `POST /v1/books/resolve` | Recebe `{partial_md5, size, isbn?, title?, authors?}` e devolve `book_id` existente ou candidatos |
| `GET /v1/progress/{book_id}` / `PUT /v1/progress/{book_id}` | Atalho para o caminho crítico (abrir/fechar livro) |
| `GET /v1/progress/by-file/{partial_md5}` | Lookup rápido sem `book_id` |
| `GET/POST/PATCH/DELETE /v1/reading-list[/{id}]` | Lista "Ler a seguir" (também passa pelo `/sync`) |
| `GET /v1/events` (SSE) | Eventos `changed{seq}` e heartbeat |
| `HEAD/PUT/GET /v1/blobs/{sha256}` | Opcional: capas e cofre de arquivos |
| `/kosync/users/auth`, `/kosync/users/create`, `PUT /kosync/syncs/progress`, `GET /kosync/syncs/progress/{doc}` | Compatibilidade com KOReader. Responder **200 sem `percentage`** para documento desconhecido (requisito K-GET-3 da spec de conformidade). |
| `GET /healthz`, `GET /v1/version` | Operação |

---

## Recomendação

1. **Servidor próprio em Go + SQLite**, um único binário, com a API acima e o adaptador **KOSync** desde a v1. Isso dá compatibilidade de graça com KOReader, Readest, Kavita e Komga, e o `kosync-conformance` funciona como teste de aceitação.
2. **Identidade:** partial MD5 + tamanho para o arquivo, `book_id` lógico com ISBN ou título+autor para agrupar, SHA-256 calculado em background.
3. **Posição:** JSON versionado. Paginados com `page`/`page_count`, reflowable com Locator Readium + CFI + trecho de texto, `percent` sempre. Tradução só no cliente.
4. **Sync:** estado + LWW com **HLC**, `server_seq` do servidor como cursor, tombstones, outbox no cliente. Para progresso, perguntar ao usuário quando a posição recebida está atrás da local.
5. **Transporte:** REST + SSE. Sem FCM na v1.
6. **Arquivos:** fora do servidor na v1 (Syncthing ou OPDS). Cofre por SHA-256 como opção futura.
7. **Infra:** começar em casa com **Tailscale**, empacotado em **Docker Compose + módulo NixOS**. Plano B: **Lightsail US$ 5** (IPv4) com OpenTofu + cloud-init. Backup com **Litestream**.

## Riscos e perguntas em aberto

- **Motores diferentes:** com Readium no Android e outro motor no desktop, o CFI nem sempre casa. **O protótipo precisa testar** a resolução por trecho de texto (depende da frente de stack de cliente).
- **Relógio:** o HLC não protege a primeira escrita de um relógio muito errado. O servidor deve recusar ou ajustar `hlc` mais de 5 min no futuro.
- **CBZ/CBR:** os dois clientes precisam da **mesma** ordenação natural de páginas, testada com fixtures.
- **Mesmo livro em arquivos diferentes:** quanto de agrupamento automático é aceitável? Erro aqui mistura progresso de livros distintos.
- **Tailscale no Android** ocupa o slot de VPN. O usuário já usa outra VPN?
- **Abrir para outros usuários com cofre de arquivos** traz responsabilidade legal (direitos autorais e LGPD). Decidir antes de ligar esse recurso.
- **AGPL:** não copiar código do koreader-sync-server, do KOReader nem do Readest. Reimplementar a partir da spec CC0 e da descrição do algoritmo.
- ⚠️ **Não verificado:** preços atuais da Hetzner (houve reajustes em 2026), preço do EC2 t4g.nano, custo de domínio, consumo real de RAM do servidor em Go, e o motivo do arquivamento do repositório do PocketBase.

## Fontes

- KOReader `util.partialMD5`: https://github.com/koreader/koreader/blob/master/frontend/util.lua
- KOReader plugin kosync (debounce de 25 s, estratégias forward/backward, `userkey = md5(senha)`): https://github.com/koreader/koreader/blob/master/plugins/kosync.koplugin/main.lua
- koreader-sync-server (rotas, Redis, AGPL, v2.1.1): https://github.com/koreader/koreader-sync-server ([routes.lua](https://github.com/koreader/koreader-sync-server/blob/master/config/routes.lua))
- Spec e verificador KOSync (CC0/BSD-3): https://github.com/pid1/kosync-conformance
- Readium Locator: https://readium.org/architecture/models/locators/ · Kotlin toolkit 3.4.0 (BSD-3): https://github.com/readium/kotlin-toolkit · TS toolkit: https://github.com/readium/ts-toolkit
- EPUB CFI (IDPF): https://idpf.org/epub/linking/cfi/
- Komga (MIT, 1.30.1) e KOReader sync: https://komga.org/docs/guides/koreader · `R2Progression`: https://github.com/gotson/komga/blob/master/komga/src/main/kotlin/org/gotson/komga/domain/model/R2Progression.kt
- Kavita (GPL-3.0, v0.9.1.4): https://github.com/Kareadita/Kavita · https://wiki.kavitareader.com
- Calibre-Web (0.6.27): https://github.com/janeczku/calibre-web · Calibre-Web-Automated (v4.0.8): https://github.com/crocodilestick/Calibre-Web-Automated
- Readest (AGPL-3.0, v0.12.20) e notas de sync: https://github.com/readest/readest · `md5.ts`: https://github.com/readest/readest/blob/main/apps/readest-app/src/utils/md5.ts · https://github.com/readest/readest/tree/main/apps/readest-app/.claude/memory
- Syncthing (MPL-2.0, v2.1.7): https://github.com/syncthing/syncthing · fork Android: https://github.com/researchxxl/syncthing-android
- Litestream (v0.5.18): https://github.com/benbjohnson/litestream · ntfy (v2.37.2): https://github.com/binwiederhier/ntfy · UnifiedPush: https://unifiedpush.org
- Tailscale (Personal: 6 usuários, dispositivos ilimitados): https://tailscale.com/pricing
- Cloudflare (limite de 100 MB no Free/Pro): https://developers.cloudflare.com/fundamentals/reference/connection-limits/ (confirmado só via busca)
- AWS Lightsail (US$ 3,50 só IPv6 / US$ 5 com IPv4; metade da franquia em São Paulo): https://aws.amazon.com/lightsail/pricing/
- Oracle Always Free (A1 e política de instâncias ociosas): https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm
- Hetzner (reajustes de 2026, ⚠️ valores não confirmados na fonte primária): https://docs.hetzner.com/general/infrastructure-and-availability/price-adjustment/
- Versões, licenças e datas: API do GitHub (`/repos/{owner}/{repo}` e `/releases/latest`), consultada em 13/11/2026.
