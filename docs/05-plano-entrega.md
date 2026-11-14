# Plano de entrega: Windows, macOS, Linux e Android

Estado: **plano, nada construído**. Escrito em 14/11/2026. Decisões em aberto estão na seção 2 e
bloqueiam a Fase 1 em diante. Preços e regras de lojas/assinatura foram conferidos na documentação
em 11/2026: reconferir antes de pagar qualquer coisa.

## 1. Ponto de partida (verificado no repositório)

| Item | Hoje |
|---|---|
| Linux | só o pacote Nix (`nixos/bookferry.nix`); sem `.deb`, `.rpm`, AppImage ou Flatpak |
| Android | APK **debug**, só `arm64-v8a`, `minSdk 24`, `targetSdk 37`, `versionCode 1000`; sem assinatura de release |
| Windows e macOS | nunca compilaram: os caminhos `cfg(not(unix))` em `db.rs` e `tls.rs` existem, mas ninguém os rodou |
| CI | não existe (`.github/` ausente no repo do app) |
| `tauri.conf.json` | só `bundle.active` e ícones: sem alvos, assinatura, atualizador |
| Versão | 0.1.0 em tudo; licença do workspace declarada `MIT OR Apache-2.0` |
| Papel de cada SO | desktop = servidor embutido (porta fixa 47600, mDNS) + cliente; Android = só cliente |

Consequência de produto: o instalador do **desktop** precisa lidar com porta de entrada e descoberta
na rede em cada SO. O Android não abre porta nenhuma.

## 2. Decisões do Lucas (bloqueiam a execução)

| ID | Decisão | Por que importa | Recomendação |
|---|---|---|---|
| D1 | **Modelo de venda e licença**: (a) código fechado, licença/chave por compra; (b) código aberto, cobra pela conveniência (binários, lojas, suporte); (c) só pelas lojas | muda o repo (público ou privado), a licença no `Cargo.toml`, se Flathub/AUR são possíveis e o que o botão "Código" do site aponta | (a) para começar: é o que "landing paga" sugere. Trocar o campo `license` para proprietário |
| D2 | **Visibilidade do repo do app** | repo público: CI grátis, inclusive ARM; privado: ~US$ 12/mês de Actions (cerca de 193 min de macOS) | privado + macOS só em tag de release |
| D3 | **Canais**: site direto, lojas (Play, Microsoft Store, Mac App Store) ou os dois | lojas dão assinatura e atualização, mas têm taxa, revisão e regras; Play exige teste fechado com 12 pessoas por 14 dias em conta pessoal nova | site direto primeiro; Play depois |
| D4 | **Aparelhos de teste**: há Windows? há Mac? | sem Mac não dá para testar o app no macOS; sem Windows, só teste em CI | decidir antes de vender para cada SO |
| D5 | **Orçamento de assinatura** | Windows: certificado OV ~US$ 150–300/ano, ou Azure Artifact Signing (~US$ 10/mês, mas pessoa física só nos EUA/Canadá); macOS: Apple Developer US$ 99/ano; Android: US$ 25 uma vez (Play) | macOS e Windows assinados só quando for cobrar; antes disso, avisar no site |
| D6 | **Cobrança**: quem processa pagamento e entrega a licença | landing diz "[a definir]"; precisa de checkout, recibo, reembolso, imposto | pesquisar na hora (Brasil: meio de pagamento e nota fiscal); não é técnico |
| D7 | **Android x86_64/armv7**: incluir? | hoje só arm64; cobre quase todos os celulares atuais, mas exclui aparelhos antigos de 32 bits e emuladores | só arm64 no início; adicionar `x86_64` se houver demanda |

## 3. Plano por plataforma

### 3.1 Linux ("gama boa de distros")

| Público | Formato | Observação |
|---|---|---|
| Ubuntu, Debian, Mint, Pop!_OS | `.deb` | exige `libwebkit2gtk-4.1`; Ubuntu 22.04+ e Debian 12+ têm. Ubuntu 20.04/Debian 11 ficam de fora (a confirmar na prática) |
| Fedora, openSUSE, RHEL-like | `.rpm` | idem, pacote do webkit2gtk 4.1 da distro |
| Arch, Manjaro, outras | AppImage; AUR só se o código for aberto (D1) | AppImage roda em qualquer distro, mas é maior |
| Qualquer uma | AppImage | formato universal único; o mais simples de distribuir pelo site |
| NixOS | pacote Nix (já existe) | manter; não é produto vendido |
| Flatpak | opcional, só depois | Flathub não é lugar natural para app pago |

- Constrói em runner `ubuntu-22.04` (base antiga = compatibilidade com mais distros).
- **Testar de verdade sem hardware:** subir containers Docker (Ubuntu 22.04/24.04, Debian 12, Fedora)
  no PC, instalar o pacote e abrir sob Xvfb, com o mesmo `ping`/E2E de hoje.
- **Firewall:** `ufw` e `firewalld` bloqueiam a porta 47600 por padrão quando ativos. O pacote não
  deve mexer em firewall sozinho; o app deve **detectar que o celular não alcança o PC** e mostrar a
  instrução certa (comando `ufw`/`firewall-cmd`). Isso é trabalho de app, não de instalador.
- Assinatura: não há obrigatória; publicar `SHA256SUMS` (e assinatura GPG opcional).

### 3.2 Windows

- Formato: instalador NSIS `.exe` (instala o WebView2 se faltar). Constrói no runner Windows.
- **Firewall:** o instalador com privilégio pode criar a regra de entrada para a porta 47600; sem
  isso o Windows pergunta no primeiro uso. Decidir qual (a regra no instalador é mais limpa).
- **SmartScreen:** sem assinatura aparece "Windows protegeu seu PC". Com certificado OV a reputação
  ainda demora a vir; certificado EV elimina o aviso, mas custa mais. Ver D5.
- Risco técnico alto: o caminho não-Unix de permissão de arquivos (segredos com permissão restrita)
  nunca rodou; no Windows é por ACL. **Revisar antes de prometer a segurança descrita no `SOFTWARE.md`.**
- Teste: smoke em CI (instalar, abrir, `ping`); teste humano em máquina real, se houver (D4).

### 3.3 macOS

- Formato: `.dmg`, um por arquitetura (`aarch64-apple-darwin` e `x86_64-apple-darwin`), ou universal.
- Assinatura e **notarização** exigem Apple Developer (US$ 99/ano). Sem isso o Gatekeeper bloqueia o
  app e o usuário precisa contornar manualmente. Assinatura ad-hoc (`"-"`) não resolve isso (não verificado).
- **Rede local:** o macOS pede permissão de "Rede local" para mDNS e o firewall do sistema pergunta
  sobre conexões de entrada. Precisa de `NSLocalNetworkUsageDescription` e `NSBonjourServices` no
  `Info.plist`, e o texto de ajuda no app.
- Só dá para construir e assinar em runner macOS (caro em repo privado: tag de release apenas).
- Sem um Mac para teste (D4) o risco é alto: não vender macOS sem ter rodado o app em um.

### 3.4 Android

- **APK de release assinado** (`cargo tauri android build --apk`) para distribuir pelo site;
  **AAB** (`--aab`) para a Play Store.
- **Keystore de release própria**, fora do repo, com backup em dois lugares. Se perder, não dá para
  atualizar o app já instalado. No CI, `keystore.properties` é gerado a partir de segredos.
- `versionCode` precisa crescer a cada release (hoje 1000, vindo de `tauri.properties`); definir regra
  (ex.: `major*10000 + minor*100 + patch`).
- Play Store: conta pessoal nova exige teste fechado (12 testadores, 14 dias seguidos) antes de pedir
  produção; o primeiro upload é manual. Política de dados e "Segurança dos dados" precisam estar preenchidos
  (o app não coleta nada, mas usa rede local e busca de capas na internet).
- Sideload pelo site: o usuário precisa liberar "instalar apps desconhecidos"; documentar na página.
- Android não vira servidor: continua só cliente.

### 3.5 O que vale para todas

- **Atualização:** `tauri-plugin-updater` em todos os SOs, com chave própria de assinatura de update
  (`tauri signer generate`, guardar com a mesma disciplina da keystore). No Android da Play Store, quem
  atualiza é a loja.
- **Versionamento:** uma versão só para o produto (hoje 0.1.0 no workspace); tag `vX.Y.Z` dispara o release.
- **Release por tag:** rascunho de release com todos os instaladores e `SHA256SUMS`; a página de download
  da landing aponta para os arquivos. Não colocar link de download antes do instalador ter sido testado.
- **Honestidade do site:** o site e o `SOFTWARE.md` só podem afirmar um SO depois de ele ter sido
  construído **e** aberto em um aparelho real ou em CI com smoke test. Hoje o site diz "a confirmar".

## 4. Fases (em ordem)

| Fase | Entrega | Pronto quando | Depende de |
|---|---|---|---|
| 0 | Decisões D1–D7 | respostas anotadas aqui | Lucas |
| 1 | **CI base** em branch separada: compilar + `cargo test` + `node --test` em Linux, Windows e macOS, sem instalador | os 3 SOs verdes (corrigindo o que o Windows e o macOS acharem) | D2 |
| 2 | **Linux**: `.deb`, `.rpm`, AppImage; teste em containers das 4 distros | instala e abre em Ubuntu 22.04/24.04, Debian 12 e Fedora | Fase 1 |
| 3 | **Android release**: keystore, APK assinado, regra de `versionCode` | `adb install` do APK de release e pareamento com o desktop | D3 |
| 4 | **Windows**: NSIS, regra de firewall, smoke test | instala, abre, o celular pareia | Fase 1, D4, D5 |
| 5 | **macOS**: `.dmg`, permissões de rede local, notarização | abre num Mac real sem contornar o Gatekeeper | Fase 1, D4, D5 |
| 6 | **Assinatura** (Windows e macOS) e Play | sem aviso de segurança no primeiro uso | D5 |
| 7 | **Pipeline de release por tag**, atualizador, página de download | tag gera release completo; app instalado se atualiza | fases 2–5 |
| 8 | **Cobrança e entrega de licença** | uma compra de ponta a ponta com reembolso testado | D1, D6 |

Linux e Android são as primeiras porque já rodam hoje: é o menor caminho até algo vendável.
Windows e macOS dependem de achar bugs que ainda não vimos.

## 5. Riscos

1. **Windows/macOS nunca compilaram.** A Fase 1 pode revelar de uma correção pequena até retrabalho
   em rede/permissões. Tratar como descoberta, não como tarefa fechada.
2. **Segurança de segredos fora do Unix:** permissões `0600` não existem no Windows; sem revisão, o
   texto de privacidade do site seria falso naquela plataforma.
3. **Firewall e rede local** são o maior ponto de suporte (porta 47600 e mDNS em cada SO, além de
   Wi-Fi com isolamento de clientes, que nenhum instalador resolve).
4. **Custo de CI em repo privado**, especialmente macOS.
5. **Perda de chaves** (keystore Android, chave do atualizador, certificados): sem backup, o produto
   não consegue mais atualizar quem já instalou.
6. **Licença do código** ainda declara MIT/Apache-2.0 no `Cargo.toml` (D1).
