# Frente 3 — Produto: features de leitura, "Ler a seguir" e capas

> Pesquisa de 2026-11-13. **Verificado** = lido na fonte primária ou testado por mim com requisições reais (amostra pequena). **Não verificado** = só busca secundária ou conhecimento prévio. Nenhum código de produto foi escrito.

## 1. Inventário de features (Moon+ Reader e YACReader)

Fontes: [moondownload.com](https://www.moondownload.com/) e [yacreader.com/features](https://www.yacreader.com/features) (YACReader **10.5.0, 09/11/2026**; desktop e servidor gratuitos e open source; apps iOS/Android com sync de progresso com a Library).

**Legenda:** MVP = sem isso o app não cumpre a ideia central · v1 = esperado por quem usa Moon+/YAC · Depois = diferencial ou custo alto.

| Área | Moon+ Reader (Android) | YACReader (PC/Android) | Classe |
|---|---|---|---|
| Formatos | txt, html, epub, pdf, djvu, mobi, azw/azw3, fb2, docx, rtf, cbr, cbz, md, OPDS… | CBR/CBZ/CB7/RAR/ZIP/7Z/TAR/PDF; EPUB só de imagens é "experimental" | MVP: EPUB, PDF, CBZ · v1: CBR, CB7 · Depois: MOBI/AZW3/FB2 (ver frente de formatos) |
| Modos de leitura | Rolagem suave real-time, paginação com 5 animações de virar página, auto-scroll (5 modos: persiana, por pixel, linha ou página) | Página a página, **double-page**, rolagem contínua (webtoon) | MVP: página única + rolagem · v1: double-page, auto-scroll · Depois: animações de página |
| Temas/brilho | 10+ temas, modo dia/noite, fundo customizado, AMOLED, filtro de luz azul, **brilho deslizando na borda esquerda**, fonte/entrelinha/justificado/hifenização | 12 temas + editor (UI), brilho/contraste/gama da imagem, escala Lanczos | MVP: claro/escuro/sépia + fonte/tamanho + brilho in-app · v1: gesto de brilho, tema customizável, ajustes de imagem · Depois: texturas/editor |
| Gestos / zonas de toque | Grade configurável: 24 operações × 15 eventos (toque, swipe, teclas de volume, fones, câmera) | Atalhos customizáveis, modos de mouse, pinch/touchpad zoom | MVP: toque esq/centro/dir + teclas de volume (Android) + setas (PC) · v1: zonas configuráveis · Depois: mapeamento total |
| Zoom / navegação em quadrinhos | PDF/CBZ básicos | **Lupa** (redonda/retangular, easing de borda), fit modes, **painel a painel** com detecção automática e **corte automático de margens** (mobile), go-to flow, salvar/extrair página | MVP: fit width/page + pinch zoom + duplo toque · v1: lupa, corte de margens · Depois: painel a painel |
| Manga / RTL | Via CBZ | **Modo manga completo**: páginas RTL + controles invertidos | MVP: flag RTL por livro (com leitura de `Manga` do ComicInfo) · v1: RTL em double-page |
| TTS | Sim (motor do sistema, rede), "agitar para falar" (Pro), fones/Bluetooth | Não | v1 (Android, só EPUB/texto) · PC: Depois |
| Anotações | Destaques em várias cores, sublinhado, notas, exportação, **sync de posição, favoritos, destaques e notas** via Dropbox/WebDAV | Favoritos/marcadores; sem destaques de texto (quadrinhos) | MVP: marcadores · v1: destaques+notas em EPUB (modelo sincronizável) · Depois: exportação |
| Dicionário / tradução | Dicionários offline (ColorDict etc.) e online, tradução, Wikipedia | Live Translation só no iOS | v1: lookup via intent/URL externa · Depois: dicionário embutido |
| Estatísticas | Tempo e velocidade de leitura por livro | — | Depois |
| Estante / biblioteca | Favoritos, Autores, Tags, Downloads, capa própria, widgets | **YACReaderLibrary**: pastas continuam no disco; vistas grade/flow/tabela; *Continue Reading*, *Recent*, marca "novo"; listas de leitura, **rótulos de cor**, favoritos, nota; *smart lists*; busca com sintaxe (`writer:Smith`, `read:false AND rating>=4`); edição em massa; capas customizadas; renomear/organizar arquivos por metadados; backup do BD; lido/não lido | MVP: grade com capa + progresso + lido/não lido + busca · v1: séries, tags/rótulos, favoritos, edição de metadados · Depois: smart lists, sintaxe de busca, organizar arquivos |
| Metadados | — | Scraper ComicVine + **import/export ComicInfo.xml**, escaneia pasta atrás de XML embutido | MVP: ler ComicInfo/OPF · v1: editar e escrever · Depois: scraper |
| Sync | Dropbox/WebDAV (posição, marcadores, destaques, arquivos) | Progresso bidirecional via servidor (Server API v3); servidor headless Docker + Web UI | MVP: posição + lista · v1: marcadores/destaques/estados |

**Leitura do inventário:** Moon+ é forte em *customização de texto e controles*; YACReader em *biblioteca/metadados e leitura de imagens*. O diferencial do produto (sync de posição + lista "Ler a seguir") não existe nos dois, então o MVP deve ser **pequeno** em leitura e caprichado em sync.

## 2. Fontes de capa e metadados (lista "Ler a seguir")

### 2.1 Comparação

Testes: busca real em Open Library (sem chave), AniList (sem chave); Google Books (ver nota).

| Fonte | Chave | Limite (fonte) | Busca por nome / pt-BR | Capas | Termos para cachear imagem |
|---|---|---|---|---|---|
| **Open Library** (`/search.json` + Covers) | Não | Covers por ID de capa/OLID: sem limite; **por ISBN: 100 req/IP/5 min**; "não faça crawl" (verificado) | Busca Solr em título/autor, aceitou typo leve (`harry poter` achou o livro; testado). Resultados misturam idiomas: `o hobbit` trouxe edição `por` (com capa), mas `senhor dos aneis a sociedade do anel` devolveu a edição pt **sem `cover_i`**, e o *Sapiens* pt idem. Cobertura pt-BR irregular | S/M/L (JPEG); blank ou 404 com `?default=false` | Guia pede *hotlink* para `covers.openlibrary.org` e link de volta; download em massa só via dumps do archive.org. **Não li** os termos formais de licença das imagens (muitas são capas de editoras) — cache pessoal por item selecionado é consistente com o uso pretendido, mas não é garantia legal |
| **Google Books** (`/volumes`) | Na prática **sim** | Cota diária por projeto; valor padrão reportado ≈1.000/dia (busca secundária, **não verificado**). Teste sem chave: HTTP 429 `quota_limit_value: 0` no projeto anônimo compartilhado | `langRestrict=pt`, busca por palavras (stem), melhor cobertura pt-BR e de edições brasileiras (não pude medir por causa da cota) | `imageLinks.thumbnail` (baixa resolução, zoom param) | ToS (verificado): **não cobrar dos usuários** sem acordo, remover conteúdo infringente. Regra de cache não encontrada na página lida; fontes secundárias sugerem não armazenar permanentemente — **tratar como restritivo**: exibir via URL, cache curto |
| **Hardcover** (GraphQL) | Sim (token gratuito, conta) — endpoint exige `Authorization` (verificado: erro sem header) | Reportado: 60 req/min, burst 10, 5.000/dia (**não verificado**; docs deram 403 para cliente automatizado) | Busca Typesense (tolerante a typo); catálogo comunitário, forte em inglês | URL de imagem por edição | **Não verificado.** Ler ToS antes |
| **ComicVine** | Sim (gratuita) | 200 req/recurso/hora + detecção de velocidade (verificado) | Bom para quadrinhos US; fraco para mangá/BR | Boas, por volume/edição | Verificado: **somente uso não comercial**, fazer cache de respostas, linkar a origem, "não redistribuir em outro meio" |
| **Metron** | Sim (conta/token) | Reportado 5.000/dia, ≥20/min (**não verificado**: o site exige desafio proof-of-work (Anubis) e **não o contornei**) | Quadrinhos US/DC/Marvel, com wrapper `mokkari` (GPL-3.0, reportado) | Capas por issue | Não verificado |
| **AniList** (GraphQL) | **Não** | 90 req/min (hoje degradado a **30/min**, aviso na doc — verificado) | Bom para mangá/anime (`berserk` ok); **sem tolerância a typo** (`bersek` → 0 resultados, testado); títulos romaji/inglês, pouco pt-BR | `coverImage.large` no CDN `s4.anilist.co`; filtrar `isAdult:false` | Verificado: gratuito não comercial (<US$150/mês); **proibido usar como backup/armazenamento e coleta em massa**; nome do app não pode conter "AniList" sem "unofficial" → *hotlink* + cache de curta duração |
| **Amazon** (PA-API/Creators API) | Sim, exige conta Associates com vendas qualificadas | Atrelado a vendas (**não verificado**) | Excelente catálogo/capas | Boas | Fontes secundárias: sem armazenamento permanente, refresh ~24h → **descartar** (inviável para uso pessoal sem afiliado) |
| **Scraping de sites de livrarias** | — | — | — | — | **Não recomendado** (ToS/CAPTCHAs/anti-bot; não implementar contorno) |

> **Sobre o "cachear a imagem":** nenhuma fonte verificada licencia explicitamente copiar a capa para armazenamento próprio por tempo indefinido. Três saídas honestas: (1) imagens que o **próprio usuário** escolhe/envia (sem risco de ToS de API); (2) **capa extraída do arquivo** do livro (EPUB/CBZ), que o usuário já possui; (3) cache local *do cliente* (como YACReader/Calibre fazem), tratado como cache privado, sem redistribuir para terceiros e sem servir publicamente. Para o modo multiusuário futuro, o servidor **não deve ser CDN de capas de terceiros**.

### 2.2 Fluxo proposto

```
digita nome → [debounce 400 ms] → cache de busca local (TTL 24h)
   → consulta em paralelo (timeout 4 s): Open Library + (Google Books se chave) + (AniList/ComicVine/Hardcover se habilitado e tipo = mangá/quadrinho)
   → normaliza em Candidato {titulo, autores, ano, idioma, isbn, capa_url, fonte}; deduplica por ISBN/título normalizado; ordena: pt primeiro, tem capa primeiro
   → usuário vê grade de 5–8 candidatos com capa → confirma um (ou "nenhum serve")
   → cliente baixa a capa 1×, reduz (~400 px, WebP/JPEG), calcula sha256 → salva local
   → grava item {id, titulo, autor, fonte, id_externo, capa_sha256, ordem}; sobe capa ao servidor como blob content-addressed (privado do usuário)
   → sync da lista para os outros dispositivos (que baixam o blob da capa do servidor, não da API externa)
```

**Fallbacks:** (a) sem rede/API fora → salvar só o texto, "capa pendente", repetir busca depois (retry exponencial); (b) sem resultado → busca com título "limpo" (sem subtítulo/volume) e por autor; (c) **edição manual sempre disponível**: colar URL, escolher imagem da galeria/câmera, ou capa gerada (cor + título); (d) o item vale com ou sem capa; (e) quando o livro é aberto de verdade, trocar pela capa embutida no arquivo.

**Boas práticas:** busca *debounced*, limite de ~5 req por digitação confirmada (nunca por tecla), `User-Agent` identificado, respeitar `429/Retry-After`. Com 1 usuário o volume é minúsculo; no multiusuário, centralizar buscas no servidor com cache compartilhado só de **metadados e URLs** (não de imagens de terceiros).

**Recomendação de fontes por fase:** MVP = Open Library (sem chave) + upload manual; v1 = Google Books (chave do próprio usuário/servidor) e AniList para mangá + capa extraída do arquivo; Depois = Hardcover/ComicVine/Metron opcionais, ativados por chave pessoal.

## 3. Metadados de biblioteca

| Formato | Campos úteis | Observações |
|---|---|---|
| **ComicInfo.xml** (CBZ; schema v2.1 verificado em [anansi-project](https://anansi-project.github.io/docs/comicinfo/schemas/v2.1)) | `Series`, `Number`, `Count`, `Volume`, `Title`, `Summary`, `Writer`…, `StoryArc`/`StoryArcNumber`, `SeriesGroup`, `LanguageISO`, `Manga` (Yes/YesAndRightToLeft/No), `AgeRating`, `Web`, `GTIN`, `PageCount`, `Pages` (tipo FrontCover etc.) | Padrão de fato (YACReader, Komga, Kavita, ComicTagger). CBR/CB7 também podem conter. **`Manga=YesAndRightToLeft` define RTL por padrão** |
| **OPF do EPUB** (conhecimento da especificação, não rebuscado) | `dc:title`, `dc:creator` (+`file-as`), `dc:language`, `dc:identifier` (ISBN/UUID), `dc:publisher`, `dc:date`, `dc:description`, `dc:subject`; série: EPUB3 `belongs-to-collection` + `group-position`, ou convenção Calibre `calibre:series`/`calibre:series_index`; capa: `properties="cover-image"` (EPUB3) ou `<meta name="cover">` (EPUB2) | Extrair no cliente na importação; muitos EPUBs não têm série → editável |
| **PDF** | Info dict (título/autor), frequentemente vazio | Usar nome do arquivo + 1ª página como capa |
| **Sem metadados** | Nome do arquivo/pasta | Heurística: `Série 003 (2019).cbz`, pasta = série |

**Modelo de agrupamento sugerido:** `Série` (nome, ordem, total opcional) 1—N `Livro` (volume/número); `Coleção/Lista` N—N `Livro` (manual, tags/rótulos); "Ler a seguir" é uma lista especial cujos itens podem ou não virar `Livro` (vínculo por ISBN/ID externo ou por hash de conteúdo quando o arquivo for importado). Prioridade da fonte: **edição do usuário > metadados embutidos > API externa > nome do arquivo**, guardando a origem de cada campo para não sobrescrever edições manuais. Isso também sustenta o multiusuário (toda entidade com `user_id`).

## 4. Histórias de usuário (priorizadas)

| # | História | Prio |
|---|---|---|
| 1 | Como leitor, abro EPUB/PDF/CBZ e o app lembra onde parei | MVP |
| 2 | Fecho na página 50 no PC e o Android abre na 50 (e vice-versa), mesmo após uso offline | MVP |
| 3 | Vejo minha biblioteca em grade com capa, progresso e lido/não lido; busco por título/autor | MVP |
| 4 | Digito um nome em "Ler a seguir", escolho a capa entre candidatos e o item aparece nos meus dispositivos | MVP |
| 5 | Se a capa não for encontrada, envio uma imagem ou uso capa gerada | MVP |
| 6 | Ajusto tema, fonte e brilho, e uso toque/teclas para virar páginas | MVP |
| 7 | Leio mangá em RTL e quadrinhos em double-page com zoom | v1 |
| 8 | Vejo quadrinhos agrupados por série (via ComicInfo/pasta) e continuo do próximo volume | v1 |
| 9 | Marco trechos/notas em EPUB e eles sincronizam | v1 |
| 10 | Marco um item de "Ler a seguir" como "começado" ao importar o arquivo correspondente | v1 |
| 11 | Ouço o livro em voz alta (TTS) e consulto palavras no dicionário | v1/Depois |
| 12 | Outra pessoa tem conta própria no meu servidor e não vê meus livros/listas | Multiusuário |

## 5. Roadmap

| Fase | Entrega | Critério de pronto |
|---|---|---|
| **MVP** (histórias 1–6) | Leitor EPUB/PDF/CBZ; sync de posição e lista "Ler a seguir"; biblioteca em grade; capas via Open Library + manual; servidor self-hosted com 1 usuário (mas `user_id` em todas as tabelas) | Fechar em um dispositivo e abrir em outro na mesma posição, com offline e conflito resolvido |
| **v1** (7–11) | RTL/double-page/lupa; séries via ComicInfo/OPF; marcadores/destaques; Google Books e AniList; CBR/CB7; TTS; zonas de toque configuráveis | Paridade percebida com o dia a dia no Moon+/YACReader |
| **Multiusuário** (12) | Registro/convites, isolamento por usuário, cotas, busca de capas centralizada no servidor com cache só de metadados; revisão de ToS (AniList/ComicVine/Google restringem uso comercial/redistribuição) | Dois usuários sem vazamento de dados; checklist legal |

## Recomendação

1. **MVP enxuto de leitura, forte em sync**: priorizar posição/lista; copiar de Moon+ apenas temas, brilho e zonas de toque básicas; de YACReader, a biblioteca com progresso, modo RTL (via `Manga` do ComicInfo) e double-page na v1.
2. **Capas: Open Library primeiro + upload manual + capa embutida no arquivo**; Google Books/AniList como complementos opcionais com chave/hotlink. **Não usar Amazon nem scraping.**
3. Tratar capa como **cache privado do cliente/servidor do usuário**, não como ativo redistribuível; guardar `fonte` e `id_externo` para poder re-baixar e creditar a origem.
4. Priorizar metadados nesta ordem: edição do usuário > ComicInfo/OPF > API > nome do arquivo.

## Riscos / Perguntas em aberto

- **Qualidade pt-BR das capas**: no teste, Open Library achou o texto mas faltou `cover_i` nas edições pt do *Senhor dos Anéis* e *Sapiens*. Preciso medir em ~30 títulos antes de decidir se Google Books (com chave) vira obrigatório. Amostra de teste foi pequena (4 títulos).
- **Google Books**: cota 1.000/dia e regras de cache **não verificadas**; sem chave a API respondeu 429. Quem fornece a chave (o usuário ou o servidor)? ToS proíbe cobrar pelo app sem acordo.
- **Hardcover e Metron** não verificados (403/proof-of-work); ler termos antes de adotar.
- **Licença das capas**: não há licença explícita; risco aceitável para uso pessoal, **precisa de revisão antes do multiusuário/público**.
- **AniList** está degradado (30 req/min) e proíbe armazenar dados em massa; manter só hotlink + cache curto.
- Perguntas ao usuário: (a) quadrinhos/mangá serão maioria? (decide AniList/ComicVine); (b) servidor guarda os arquivos ou só metadados (afeta capa embutida e deduplicação por hash)? (c) TTS/dicionário entram no v1 mesmo ou ficam para depois? (d) PC terá destaques de texto (escopo de UI)?
- Não verifiquei: documentação de OPF/EPUB3 nesta sessão (listada a partir do conhecimento da especificação), nem detalhes de gestos do Moon+ além do site oficial.

## Fontes

- Moon+ Reader (oficial): https://www.moondownload.com/
- YACReader (home e features, v10.5.0): https://www.yacreader.com/ · https://www.yacreader.com/features
- Open Library Covers API: https://openlibrary.org/dev/docs/api/covers · Search API: https://openlibrary.org/dev/docs/api/search
- Google Books Terms: https://developers.google.com/books/terms · Using the API: https://developers.google.com/books/docs/v1/using
- ComicVine API (termos e limites): https://comicvine.gamespot.com/api/
- AniList: https://docs.anilist.co/guide/rate-limiting · https://docs.anilist.co/guide/terms-of-use
- ComicInfo.xml v2.1: https://anansi-project.github.io/docs/comicinfo/schemas/v2.1
- Hardcover: https://docs.hardcover.app/api/getting-started/ (não acessível; só busca secundária) · Metron: https://metron.cloud/docs (bloqueado por proof-of-work; não acessado)
- Amazon PA-API/Creators API: apenas busca secundária (não verificado)
