# Marca do Bookferry

**Conceito:** um livro aberto que vira barco. As páginas são o convés, a capa é o casco, o
marcador de página sobe como bandeira no mastro e as ondas levam a leitura de um aparelho para o
outro — o *ferry* que carrega o ponto onde você parou entre o PC e o celular.

## Cores

| Nome  | Hex       | Uso                                             |
| ----- | --------- | ----------------------------------------------- |
| Tinta | `#1B2A3A` | fundo do ícone, botões principais (tema claro)  |
| Papel | `#F4ECDD` | páginas, texto sobre tinta, botões (tema escuro) |
| Brasa | `#E8874A` | casco, bandeira, progresso de leitura           |
| Mar   | `#5FA7A3` | ondas, estado "conectado"                       |

Tipografia: **Literata** (OFL, em `app/src/fonts/`) para títulos e nomes de livros; fonte do
sistema para a interface.

## Arquivos

| Arquivo                   | Para quê                                                      |
| ------------------------- | ------------------------------------------------------------- |
| `icon.svg`                | ícone principal (cantos arredondados)                         |
| `icon-small.svg`          | versão sem ondas, para 16–32 px (favicon, bandeja, `.ico`)    |
| `icon-square.svg`         | sem transparência: iOS e tiles do Windows                     |
| `android-fg.svg`          | primeiro plano do ícone adaptativo do Android (zona de 66 dp) |
| `android-mono.svg`        | camada monocromática (ícones temáticos do Android 13+)        |
| `mark.svg`                | só o símbolo, sem fundo                                       |
| `logo-horizontal.svg`     | símbolo + nome (fundo claro); texto já convertido em curvas   |
| `logo-horizontal-dark.svg`| símbolo + nome (fundo escuro)                                 |
| `wordmark.svg`            | só o nome "Bookferry" em curvas (base para o kit)             |

## Kit completo (SVG, PDF, PNG, JPEG, WebP, ICO, favicons, redes sociais, guia)

```bash
node app/branding/export-kit.mjs
```

Gera `app/branding/kit/` com o símbolo, o logo horizontal, o empilhado e o nome em cinco
variações (colorido para fundo claro e escuro, uma cor em tinta, preto e branco), sete versões
do ícone, favicons com `site.webmanifest`, imagens para Open Graph/GitHub/Play Store/X, avatar,
papéis de parede e o `guia-de-marca.pdf`. Os vetores (`svg/`, `pdf/`), favicons e `.ico` ficam no
repositório; PNG/JPEG/WebP e `social/` são regerados pelo script.

## Regerar os ícones do app

Depois de editar os SVGs:

```bash
node app/branding/generate-icons.mjs
```

O script usa o Playwright/Chromium para rasterizar e escreve tudo em `app/src-tauri/icons/`
(PNG, `.ico`, `.icns`, iOS, Android) e em `app/src-tauri/gen/android/.../res/` (ícone adaptativo
com fundo `#1B2C3D` + camada monocromática).
