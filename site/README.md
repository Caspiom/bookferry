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

## Publicar

Qualquer hospedagem de arquivos estáticos serve (GitHub Pages, Cloudflare Pages, um Nginx). A pasta
inteira é o site; não há passo de build.
