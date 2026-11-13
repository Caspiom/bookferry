# Leitura confortável (rodada 1 de 2)

Pesquisa de apps de leitura (Moon+ Reader, KOReader, ReadEra, Librera, Perfect Viewer, CDisplayEx) → 4 itens:
1. Painel "Aa" + preferências por livro + presets (EPUB) — **feito (fatia 1)**
2. Modo imersivo (EPUB/PDF/CBZ) — pendente (fatia 2)
3. Barra de progresso melhor — **feito (fatia 1)**
4. Quadrinhos: direção LTR/RTL, página dupla, margem zero, duplo toque, pré-carga — pendente (fatia 3)

Rodada 2 (depois): corte automático de margens no PDF e os itens "depois" (busca, marcadores, destaques, notas de rodapé, dicionário, rolagem contínua).

## Fatia 1 — o que existe

**Painel "Aa" (EPUB).** Botão na toolbar. Tamanho 12–36, família (livro/serifada/sem serifa/mono, só fontes do sistema), entrelinhas 1,2–2,2, margens 0–20 % (atributo `gap` do paginator do foliate), alinhamento, hifenização, tema (auto/claro/sépia/escuro/preto), itens do rodapé. Alterações valem ao vivo e são gravadas por livro (debounce 400 ms). Predefinições: 3 embutidas + até 12 do usuário (nome ≤ 24 caracteres). "Usar como padrão para novos livros" e "Restaurar padrão do livro". Esc fecha só o painel.

**Armazenamento.** `reader_prefs.json` no `data_dir`, escrita atômica (tmp + rename). Validação toda no Rust (`app/src-tauri/src/reader_prefs.rs`): enums e faixas numéricas; JSON corrompido ou campos desconhecidos caem nos padrões sem sobrescrever o arquivo. O JS (`state.js`) valida de novo e monta o CSS só a partir de enums/números. Não sincroniza com o servidor.

**Tema do "chrome".** `#reader-screen[data-reader-theme]` leva toolbar, rodapé e fundo para o tema do livro (preto puro sem moldura azul).

**Barra de progresso.** Trilho fino no rodapé; clicar/arrastar faz seek (EPUB: fração; PDF/CBZ: página). EPUB mostra capítulo, % do livro, % do capítulo e páginas restantes **no capítulo**, configuráveis. O "capítulo" é a seção (arquivo) do EPUB e as páginas vêm do paginator; EPUB de layout fixo omite esses itens em vez de inventar. PDF/CBZ: clicar no texto cicla `Pág. X / Y` → `N%` → `faltam K págs.` (lembrado).

**Posição sincronizada.** Mudar estilo não salva posição (`restyling` bloqueia o `relocate` durante o reflow) e o ponto de leitura é reposto com `view.goTo(cfi)`. Posição restaurada continua sendo sempre por CFI.

## Verificação

Comandos rodados (dentro de `cd spike && nix develop -c bash -c 'cd .. && …'`):

```
cargo test --workspace            # 0 falhas
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
node --test app/tests/logic.test.mjs   # 40 testes
cargo check --target aarch64-linux-android -p bookferry-app
bash app/tests/e2e/run-e2e.sh     # 3 execuções seguidas sem erro
```

Mutações feitas à mão (os testes têm que falhar): zona de toque 0,3→0,4; `totalChapters === 1`; clamp numérico; inversão RTL; cópia de preset; limite do nome; limite de presets; leitura de prefs por livro (Rust). Todas derrubaram algum teste, exceto `0.995→1.5` em `formatFractionPercent` (equivalente: `round(99,6)` já dá 100). No E2E, desligar a trava `restyling` derruba o roteiro; remover o `goTo(anchor)` **não** derruba (o `render()` do foliate já mantém o ponto), então esse `goTo` é redundante, mantido por segurança.

E2E (`xvfb`) confere no DOM do iframe do livro: tamanho, entrelinha, margem (`gap`), família, fundo sépia/preto, cor do chrome, persistência ao fechar/reabrir, livro sem prefs próprias usando o padrão, Esc fechando só o painel, texto do rodapé, seek pela barra e ciclo de formato no CBZ.

## Problemas conhecidos / não verificado

- **EPUB cortado à esquerda sob xvfb (aberto).** Com preferências padrão, depois de `next()` o screenshot `02-reader-epub.png` mostra a primeira letra de cada linha cortada e retângulos brancos. Não é artefato de pintura: o DOM confirma que os parágrafos visíveis começam em x=24 enquanto o recorte do paginator começa em x=35. O passo entre colunas medido nos parágrafos é 519 px, e o paginator avança 1081,8 px por página (colunas de 540 px) — dá ~44 px de deslocamento na 2ª página. Ocorre com DPR fracionário (1,0427) do xvfb; some quando o estilo muda (`render()` + `goTo(cfi)`), por isso os screenshots 10/11 saem corretos. `margin: 0` do html/body não é a causa (testado sem). Um segundo layout após abrir também não resolve. Não foi reproduzido no PC real (você viu normal); precisa ser conferido no S23 (Chromium) e no desktop com escala fracionária.
- Selects e checkbox do painel ficam com fundo mais claro que o resto no tema sépia (visual, sem perda de uso). O painel rola em janelas baixas (as ações "padrão/restaurar" ficam abaixo).
- Android (teclas de volume, barras do sistema, APK): ainda não tocado nesta fatia.
