# Site do Bookferry (`site/`)

Landing page estática, sem build: um `index.html` com CSS e JS embutidos, a Literata local em
`fonts/` e os favicons do kit de marca em `img/`.

## Ver localmente

```bash
python3 -m http.server -d site 8080
```

Abra `http://localhost:8080`.

## Como foi pensada

- **Mesma linguagem do app** ("papel e tinta"): os tokens de cor vêm de `app/src/styles.css` e da
  marca (`app/branding/README.md`): Tinta `#1B2A3A`, Papel `#F4ECDD`, Brasa `#E8874A`, Mar `#5FA7A3`.
  Literata nos títulos e no texto de leitura, fonte do sistema na interface, mono para dados.
- **Tema claro e escuro** seguindo o sistema (`prefers-color-scheme`) ou `data-theme` no `<html>`.
  A faixa de Tinta do topo e do rodapé é igual nos dois.
- **A demonstração do topo** é o único momento animado: virar a página no PC ou no celular (borda
  direita/esquerda da tela, botões ‹ › ou setas do teclado) faz a balsa levar a posição ao outro
  aparelho. A faixa do meio da tela do celular liga o modo imersivo, como no app (zonas 30/40/30).
  Com `prefers-reduced-motion`, nada se move.
- **Conteúdo verdadeiro**: os textos de exemplo são de *Dom Casmurro* (domínio público), os
  recursos listados são os que o app já tem, e a seção Plataformas diz o que ainda não existe.
  Ao mudar o produto, atualize essa lista.

## Idiomas (PT / EN)

O botão PT/EN fica na barra do topo. O português está escrito no HTML (é o que buscadores e quem
abre sem JS veem); o inglês fica no objeto `EN` do script, no fim do `index.html`.

- Cada texto traduzível tem `data-i18n="chave"` (pode ter marcação inline, como `<em>` ou `<code>`)
  ou `data-i18n-aria="chave"` (para `aria-label`).
- **Ao mudar um texto em português, atualize a mesma chave em `EN`.** Ao criar um texto novo, dê
  uma chave nova e coloque a tradução.
- O livro da demonstração muda junto: *Dom Casmurro* em PT, *Moby-Dick* em EN (objeto `DEMO`).
- O idioma vem, nesta ordem: `?lang=en` ou `?lang=pt` na URL, a última escolha (guardada no
  navegador) e o idioma do navegador (português → PT, qualquer outro → EN).

## Publicar no GitHub Pages

O workflow `.github/workflows/pages.yml` publica a pasta `site/` a cada push na `main` que altere o
site (ou pelo botão "Run workflow" na aba Actions).

1. Em **Settings → Pages → Build and deployment**, escolha **Source: GitHub Actions**.
2. Faça merge na `main`. O endereço aparece no resumo do workflow:
   `https://caspiom.github.io/bookferry/`.

O GitHub Pages em repositório **privado** só funciona nos planos pagos (Pro, Team, Enterprise). No
plano gratuito, o repositório precisa ser público.

Domínio próprio (ex.: `bookferry.gaspari.dev`): crie um registro `CNAME` apontando para
`caspiom.github.io`, informe o domínio em Settings → Pages e marque "Enforce HTTPS".

A página usa só caminhos relativos, então funciona tanto em `/bookferry/` quanto na raiz de um
domínio.
