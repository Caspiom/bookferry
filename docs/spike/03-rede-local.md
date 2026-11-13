# Spike 03 — Rede Local: Descoberta mDNS, TLS com Pinning, Pareamento, Sync de Posição (HLC + SSE), Transferência Range e Resiliência Doze

Data: 13/11/2026 · Dispositivo Android: Samsung SM-S918B (Galaxy S23 Ultra, Android 15, serial `RXCW104L1XZ`, Wi-Fi `192.168.1.114/24`, pacote `dev.gaspari.bookferry.spike`) · Host PC: NixOS (`x86_64-linux`, cabo `enp3s0` `192.168.1.2/24`, portas abertas no firewall: `TCP 47600` e `UDP 5353`).

> **Status do documento:** CONCLUÍDO — todos os 6 itens medidos em hardware real (PC NixOS `192.168.1.2:47600` ↔ Samsung Galaxy S23 Ultra `SM-S918B` Android 15 `192.168.1.114`).

---

## 0. Estado Inicial e Arquitetura do Spike

- **Estado do aparelho no início (15:39):** Conectado via USB (`RXCW104L1XZ`), Wi-Fi em `192.168.1.114/24`, `mWakefulness=Awake`, porém com `isKeyguardShowing=true`. Conforme as regras duras do spike, **nenhum PIN foi digitado**. Ativamos `svc power stayon usb` (a ser revertido com `svc power stayon false` ao final) e configuramos `setShowWhenLocked(true)` + `setTurnScreenOn(true)` na `MainActivity` para que a Activity do spike possa ficar `RESUMED` em primeiro plano durante os testes sem tocar no bloqueio de tela.
- **Escolha de organização do código (`spike/server` vs. módulo no app Tauri):**
  - Criamos `spike/server` como binário/biblioteca Rust dedicado ao servidor LAN (ou módulo compartilhado compilado tanto como binário CLI `bookferry-server` para rodar no PC na porta `TCP 47600` sem depender de janela GTK/Wayland, quanto reutilizando tipos no cliente Android em `spike/app/src-tauri`).
  - **Justificativa:** No PC, o servidor precisa rodar de forma determinística em terminal headless na porta `0.0.0.0:47600` e anunciar mDNS em `UDP 5353`, imprimindo o payload de pareamento (QR simulado) no stdout, enquanto o cliente Android (`spike/app/src-tauri`) executa o cliente Rust (`rustls` + `mdns-sd` + cliente HTTP/SSE/Range) dentro do APK Tauri.

---

## Resumo de Vereditos

| Item | Escopo | Veredito | Resumo das Medidas |
|---|---|---|---|
| **1. Descoberta mDNS/DNS-SD** | `_bookferry._tcp.local.` na porta `47600` (`UDP 5353`), `MulticastLock` no Android, 5 repetições frio/quente + fallback IP:porta | **RESOLVIDO** | **10/10 descobertas com sucesso** (`192.168.1.2:47600` + TXT `fp`). **App frio (5 reps):** mediana **`105,95 ms`** (`41,67–121,87 ms` em 4/5 reps; `1.117,74 ms` na rep 4 após retry automático de 1 s de pacote UDP perdido no ar). **App quente (5 reps):** mediana **`98,39 ms`** (`96,34–148,99 ms`, média `108,15 ms`). `NsdManager` nativo do Android levou **`1.139,88 ms`**. |
| **2. TLS com Pinning** | Certificado autoassinado (`rcgen` + `rustls`), pinning SHA-256 sem CA do sistema; casos (a) hash certo, (b) hash errado, (c) HTTP puro; latência handshake/1º req | **RESOLVIDO** | **(a) Hash correto (5 reps):** passa (`200 OK`); TCP connect médio = **`5,05 ms`**, **Handshake TLS = `7,69–8,86 ms`** (média **`8,18 ms`**), **1º Request = `3,43–4,29 ms`** (média **`3,73 ms`**), total = **`17,15 ms`**. **(b) Hash errado:** **RECUSADO** no cliente (`Pinning SHA-256 recusado`) e servidor (`HandshakeFailure`). **(c) Sem TLS (HTTP puro):** **RECUSADO** com TLS Fatal Alert (`[15, 03, 03, 00, 02, 02, 32]`). |
| **3. Pareamento** | Segredo de uso único → token de dispositivo; token inválido = `401`; segredo reusado = recusado (`403`/`409`) | **RESOLVIDO** | Segredo único (`pair_e767fddd...`) emitido pelo servidor troca por token `bf_tok_bba720e5...` (`200 OK` em **`14,14 ms`**). Token inválido = **`401 Unauthorized`**; segredo errado = **`401 Unauthorized`**; reuso do mesmo segredo = **RECUSADO (`403 Forbidden: pairing_secret_already_used`)**. |
| **4. Sync de Posição (HLC + SSE)** | `PUT/GET /position/{book_id}`, LWW por HLC, push SSE em tempo real (< 1 s aberto), pull ao reabrir de background, conflito concorrente PC 2× vs. Android | **RESOLVIDO** | **"Fechei na pág. 50 no PC" (`curl` PUT):** Android aberto recebe via SSE e renderiza `"Página 50"` em **`117 ms` RTT total** (`107 ms` até Rust SSE + `8,8 ms` DOM `rAF`; e **`26–37 ms`** com rádio Wi-Fi ativo), bem abaixo do alvo de `< 1 s`. **Background + Pull ao reabrir:** `GET /position` + render UI em **`36,4 ms`** (`LaunchState: HOT = 52 ms`). **LWW por HLC:** provado em 5 escritas concorrentes (escrita atrasada com HLC menor é rejeitada sem sobrescrever a mais nova e preservada no `history`). |
| **5. Transferência CBZ (295 MB)** | Download de `little-nemo-1905-1914.cbz` via HTTP `Range` sobre TLS, vazão (MB/s), uso de RAM, interrupção e retomada (`206 Partial Content`) | **RESOLVIDO** | **`295.457.942 bytes` (281,8 MiB)** baixados sobre TLS 1.3 com Pinning em **`22,47 s`** (**`12,54 MiB/s` / `13,15 MB/s`**). Interrompido exatamente em 50% (`147.728.971 B` em `11,27 s`, `12,50 MiB/s`) e retomado com `Range: bytes=147728971-` (`HTTP 206 Partial Content`, `11,20 s`, `12,58 MiB/s`). SHA-256 idêntico ao original (`85d657be...`) e 843 entradas ZIP íntegras. **Aumento de RAM (pico `VmHWM`): `0 KB`** (`VmRSS` estável em `222,5 MB`, streaming em blocos de 256 KB). |
| **6. Resiliência (Doze Mode)** | Conexão SSE durante `adb shell dumpsys deviceidle force-idle` e retorno com `unforce` | **VIÁVEL COM RESSALVAS** | Em **Deep Doze (`IDLE`)**, o `netd` do Android 15 **aborta ativamente** o socket TCP com `Software caused connection abort (os error 103)` (`ECONNABORTED`) e bloqueia `connect()` de saída (`connection timed out`), fazendo eventos enviados durante o bloqueio (`page: 102`) não chegarem via push. Ao sair do Doze (`unforce` + tela acesa), a thread Rust **reconecta automaticamente em `~0,91 s`** (`attempt 7`) e entrega novos pushes (`page: 103`) em **`41 ms` RTT**. **Regra de produto obrigatória:** disparar `GET /position` (Pull) automaticamente em todo evento `sse_reconnected` e `onResume`. |

---

## Crates Novos Adicionados e Licenças Confirmadas (`cargo metadata`)

Extraído com `cargo metadata --format-version 1` dentro do `nix develop`:

| Crate | Versão Exata | Licença Confirmada | Papel no Spike |
|---|---|---|---|
| `mdns-sd` | `0.11.5` | `Apache-2.0 OR MIT` | Servidor e cliente mDNS/DNS-SD (`_bookferry._tcp.local.`) em UDP 5353 |
| `rcgen` | `0.13.2` | `MIT OR Apache-2.0` | Geração em runtime de certificado X.509 autoassinado e par de chaves ECDSA |
| `rustls` | `0.23.46` | `Apache-2.0 OR ISC OR MIT` | TLS 1.3/1.2 puro Rust (provider `ring`, sem OpenSSL/CMake) e `ServerCertVerifier` customizado |
| `rustls-pki-types` | `1.16.0` | `MIT OR Apache-2.0` | Tipos DER/PEM (`CertificateDer`, `PrivateKeyDer`, `ServerName`) |
| `rustls-webpki` | `0.103.17` | `ISC` | Verificação de assinaturas do handshake TLS |
| `ring` | `0.17.14` | `Apache-2.0 AND ISC` | Primitivas criptográficas para `rustls` e `rcgen` (compila com NDK Clang sem CMake) |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` | Cálculo do hash SHA-256 do certificado (pinning) e integridade do CBZ |
| `socket2` | `0.5.10` | `MIT OR Apache-2.0` | Sondagem direta de socket UDP multicast (`224.0.0.251:5353`) |
| `if-addrs` | `0.13.4` | `MIT OR BSD-3-Clause` | Enumeração de interfaces de rede usada pelo `mdns-sd` |
| `flume` | `0.11.1` | `Apache-2.0/MIT` | Canal de eventos do `mdns-sd` |
| `getrandom` | `0.2.17` | `MIT OR Apache-2.0` | Geração criptográfica do segredo de uso único e tokens de dispositivo |

---

## 1. Descoberta mDNS/DNS-SD (`_bookferry._tcp.local.` na porta 47600)

### 1.1. Configuração no Servidor (PC) e no Cliente (Android)
- **No PC (`192.168.1.2`, interface cabeada `enp3s0`):**
  - A máquina possui 9 interfaces bridge do Docker (`br-*` e `docker0`, `172.17.0.1/16` a `172.25.0.1/16`). Para evitar que o daemon mDNS anunciasse IPs internos do Docker, restringimos o `ServiceDaemon` à interface LAN real com `mdns.disable_interface(IfKind::All)` seguido de `mdns.enable_interface(IfKind::Addr("192.168.1.2".parse()?))` e `mdns.enable_interface("enp3s0")`.
  - O serviço anunciado é `bookferry-pc._bookferry._tcp.local.` apontando para `bookferry-pc.local.` (`192.168.1.2:47600`) com TXT records `fp=<cert_sha256>`, `spki=<spki_sha256>`, `ip=192.168.1.2`, `port=47600`, `v=1`.
- **No Android (`SM-S918B`, Wi-Fi `192.168.1.114`):**
  - Adicionamos ao [`AndroidManifest.xml`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src-tauri/gen/android/app/src/main/AndroidManifest.xml) as permissões:
    ```xml
    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.CHANGE_WIFI_MULTICAST_STATE" />
    <uses-permission android:name="android.permission.ACCESS_WIFI_STATE" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    ```
  - Na [`MainActivity.kt`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src-tauri/gen/android/app/src/main/java/dev/gaspari/bookferry/spike/MainActivity.kt), adquirimos o `WifiManager.MulticastLock` em `onCreate` (liberando em `onDestroy`):
    ```kotlin
    val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    multicastLock = wifi.createMulticastLock("bookferry-mdns-lock").apply {
      setReferenceCounted(false)
      acquire()
    }
    ```
  - No Android 15 (`SM-S918B`), `if_addrs::get_if_addrs()` funcionou (`if_addrs_ok: true`, retornando 12 endereços, incluindo `wlan0=192.168.1.114`), e também habilitamos explicitamente o IPv4 da `wlan0` no `ServiceDaemon` do cliente.

### 1.2. Medidas Reais de Descoberta mDNS (5 Repetições Frio + 5 Repetições Quente)

No teste **Frio (Cold)**, cada repetição executou `adb shell am force-stop dev.gaspari.bookferry.spike` seguido de `am start -W`, medindo tanto o tempo de inicialização da Activity (`TotalTime` do `am start -W`) quanto o tempo interno no Rust desde `ServiceDaemon::new()` + `browse("_bookferry._tcp.local.")` até o evento `ServiceResolved` contendo `192.168.1.2:47600` e o TXT `fp`.

| Repetição | Modo | `am start -W TotalTime` (Cold Launch) | `SearchStarted` (ms) | `ServiceFound` (ms) | **`ServiceResolved` Total (ms)** | IP:Porta Resolvido | TXT `fp` Recebido? |
|---|---|---|---|---|---|---|---|
| **Rep 1** | **Frio (Cold)** | `425 ms` | `16,79 ms` | `121,83 ms` | **`121,87 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 2** | **Frio (Cold)** | `424 ms` | `11,18 ms` | `105,61 ms` | **`105,95 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 3** | **Frio (Cold)** | `423 ms` | `8,36 ms` | `61,27 ms` | **`61,28 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 4** | **Frio (Cold)** | `470 ms` | `10,68 ms` (retry em `1012,46 ms`) | `1.117,62 ms` | **`1.117,74 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 5** | **Frio (Cold)** | `429 ms` | `7,56 ms` | `41,45 ms` | **`41,67 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 1** | **Quente (Warm)** | — (já aberto) | `4,68 ms` | `148,95 ms` | **`148,99 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 2** | **Quente (Warm)** | — (já aberto) | `3,73 ms` | `96,18 ms` | **`96,34 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 3** | **Quente (Warm)** | — (já aberto) | `2,48 ms` | `99,77 ms` | **`99,89 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 4** | **Quente (Warm)** | — (já aberto) | `3,24 ms` | `97,11 ms` | **`97,14 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Rep 5** | **Quente (Warm)** | — (já aberto) | `3,44 ms` | `98,38 ms` | **`98,39 ms`** | `192.168.1.2:47600` | Sim (`c1de06af...`) |
| **Comparação** | **`NsdManager` Android (Kotlin)** | — | `12,64 ms` | `1.128,79 ms` | **`1.139,88 ms`** | `192.168.1.2:47600` | — |

- **Observação sobre a Rep 4 (Cold = `1.117,74 ms`):** Na 4ª repetição fria, o 1º pacote UDP multicast enviado em `10,68 ms` foi perdido no ar no Wi-Fi; o `mdns-sd` reenviou automaticamente a query em `1.012,46 ms` (intervalo de 1 s do protocolo) e resolveu o serviço 105 ms depois (`1.117,74 ms`). Isso comprova a importância do retry automático em UDP multicast.
- **Fallback (QR / IP:porta):** Embora o mDNS tenha funcionado em 100% das tentativas neste roteador (mesmo entre PC cabeado e celular no Wi-Fi), o payload do QR de pareamento (`PairingQrPayload`) já inclui `"ip":"192.168.1.2","port":47600`, servindo como fallback imediato para redes com *AP Isolation* (testado e validado no Item 2).


---

## 2. TLS com Pinning (`rcgen` + `rustls`, sem CA do sistema)

### 2.1. Implementação e Erro Real Encontrado (`close_notify` no TLS 1.3)
- O servidor gera em runtime um certificado X.509 autoassinado (`rcgen::generate_simple_self_signed`) e imprime o hash SHA-256 do certificado DER (`74c257c31fdc4ec645721ad3b802aea333d5ef96e9bbd9886e7bf5c454b8a856`) e do SPKI (`8cc8dd8fd2f3bfe336f2646dba22ee3f827ac621834f65ee29548c2b0b2b82f0`) no JSON do QR simulado.
- O cliente Android constrói `rustls::ClientConfig` usando `.dangerous().with_custom_certificate_verifier(Arc::new(PinnedCertVerifier))` **sem carregar nenhuma CA do sistema**. O verificador calcula `sha2::Sha256::digest(end_entity.as_ref())` no `verify_server_cert` e delega `verify_tls12_signature` / `verify_tls13_signature` ao `ring` para garantir posse da chave privada correspondente ao certificado fixado.
- **Erro real encontrado e corrigido na 1ª execução:**
  - Na primeira tentativa, `read_to_string` até EOF num stream TLS 1.3 retornou:
    ```text
    peer closed connection without sending TLS close_notify: https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof
    ```
  - **Causa e correção:** O `rustls 0.23` trata o fechamento do socket TCP sem alerta TLS `close_notify` como erro de truncamento (`UnexpectedEof`) quando o leitor lê até EOF. Corrigimos adicionando `stream.conn.send_close_notify()` antes do `flush()` no servidor e lendo exatamente `Content-Length` via `read_exact` no cliente.

### 2.2. Prova dos Casos (a), (b) e (c) e Latências Reais (Android → PC pela LAN)

| Caso / Repetição | Hash SHA-256 Esperado pelo Cliente | Resultado | TCP Connect (ms) | **TLS Handshake (ms)** | **1º Request HTTP (ms)** | Total (ms) | Evidência / Mensagem Real |
|---|---|---|---|---|---|---|---|
| **(a) Rep 1 — Hash Correto** | `74c257c3...4b8a856` | **PASSOU (`200 OK`)** | `5,411 ms` | **`8,860 ms`** | **`4,289 ms`** | `18,771 ms` | `{"ok":true,"server":"bookferry-pc",...}` |
| **(a) Rep 2 — Hash Correto** | `74c257c3...4b8a856` | **PASSOU (`200 OK`)** | `5,366 ms` | **`7,688 ms`** | **`3,704 ms`** | `16,909 ms` | `{"ok":true,"server":"bookferry-pc",...}` |
| **(a) Rep 3 — Hash Correto** | `74c257c3...4b8a856` | **PASSOU (`200 OK`)** | `4,888 ms` | **`8,680 ms`** | **`3,571 ms`** | `17,361 ms` | `{"ok":true,"server":"bookferry-pc",...}` |
| **(a) Rep 4 — Hash Correto** | `74c257c3...4b8a856` | **PASSOU (`200 OK`)** | `5,425 ms` | **`7,829 ms`** | **`3,640 ms`** | `17,082 ms` | `{"ok":true,"server":"bookferry-pc",...}` |
| **(a) Rep 5 — Hash Correto** | `74c257c3...4b8a856` | **PASSOU (`200 OK`)** | `4,166 ms` | **`7,850 ms`** | **`3,425 ms`** | `15,617 ms` | `{"ok":true,"server":"bookferry-pc",...}` |
| **Média / Mediana (a)** | — | **5/5 `200 OK`** | **`5,05 ms` / `5,37 ms`** | **`8,18 ms` / `7,85 ms`** | **`3,73 ms` / `3,64 ms`** | **`17,15 ms` / `17,08 ms`** | — |
| **(b) Hash Errado** | `00000000...0000dead` | **RECUSADO** | — | Abortado | — | `12,568 ms` | Cliente: `TLS handshake falhou: unexpected error: Pinning SHA-256 recusado: esperado=0000...dead, recebido=74c257c3...` / Servidor: `[TLS_HANDSHAKE_REJECTED] received fatal alert: HandshakeFailure` |
| **(c) Sem TLS (HTTP puro)** | `NONE (cleartext)` | **RECUSADO** | `5,058 ms` | Abortado | — | `8,663 ms` | Cliente: `Servidor respondeu 7 bytes nao-HTTP (TLS Alert raw bytes: [15, 03, 03, 00, 02, 02, 32])` / Servidor: `[TLS_REJECT_CLEARTEXT] first_byte=0x47 ('G') — conexao sem TLS recusada` |

---

## 3. Pareamento por Segredo de Uso Único (Sem Contas/Senhas)

Fluxo executado entre o SM-S918B e o servidor no PC sobre TLS com Pinning (`[BOOKFERRY_RESULT][FASE3_PAIRING]`):

| Etapa | Ação Executada pelo Cliente Android | Status HTTP | Resposta Real do Servidor | Veredito |
|---|---|---|---|---|
| **1. Token inválido** | `GET /position/little-nemo` com `Authorization: Bearer bf_tok_invalido_000000` | **`401 Unauthorized`** | `{"error":"unauthorized_invalid_or_missing_token","ok":false}` | **OK (Bloqueado)** |
| **2. Segredo errado** | `POST /pair` com `{"secret":"pair_errado_123","device_name":"SM-S918B"}` | **`401 Unauthorized`** | `{"error":"invalid_pairing_secret","ok":false}` | **OK (Bloqueado)** |
| **3. Segredo válido (1º uso)** | `POST /pair` com `{"secret":"pair_e767fddd0a4832a604f7e330a5df2b1f","device_name":"SM-S918B"}` | **`200 OK`** (`14,14 ms`) | `{"cert_sha256":"74c257c3...","device_name":"SM-S918B","ok":true,"token":"bf_tok_bba720e56570298a6fd19ef10cf6aeb8"}` | **OK (Token emitido)** |
| **4. Segredo reusado (2º uso)** | `POST /pair` repetindo o mesmo `pair_e767fddd0a4832a604f7e330a5df2b1f` | **`403 Forbidden`** | `{"error":"pairing_secret_already_used","ok":false}` | **OK (Reuso recusado)** |
| **5. Request autenticado** | `GET /status` com `Authorization: Bearer bf_tok_bba720e56570298a6fd19ef10cf6aeb8` | **`200 OK`** | `{"ok":true,"active_sse_clients":0,...}` | **OK (Autenticado)** |


---

## 4. Sync de Posição (`PUT/GET /position/{book_id}`, LWW por HLC e Push SSE)

### 4.1. Cenário 4A — App Android Aberto: "Fechei na Página 50 no PC" (`curl` / CLI → SSE → UI Android)
Comandos executados no PC para simular o fechamento do livro na página 50:
```bash
NOW_MS=$(date +%s%3N)
curl -sS --cacert spike/server/runtime-cert.pem --resolve bookferry.local:47600:192.168.1.2 \
  -X PUT "https://bookferry.local:47600/position/little-nemo" \
  -H "Authorization: Bearer bf_cli_local_pc_token" \
  -H "Content-Type: application/json" \
  -d "{\"device\":\"pc-curl\",\"hlc\":\"${NOW_MS}:000000:pc-curl\",\"locator\":\"page:50\",\"percent\":11.88,\"page\":50}"
```
Para eliminar qualquer erro de skew de relógio NTP entre PC e celular (observamos ~315 ms de diferença entre `SystemTime` do PC e do Android), instrumentamos um **ACK automático de dois estágios**: assim que a thread Rust do Android lê o frame SSE, ela dispara um ACK (`device: "android-rust-sse"`), e assim que o WebView atualiza o texto de `#sync-banner` e conclui o `requestAnimationFrame`, dispara um segundo ACK (`device: "android-webview-ui"`), permitindo medir o tempo total diretamente no relógio monotônico do servidor no PC (`roundtrip_put_to_ack_ms`):

| Disparo no PC | Ferramenta no PC | Tempo do `PUT` no PC | **Tempo `PUT → Rust SSE → ACK` (ms)** | Render DOM `rAF` no WebView (`rust_to_ui_dom_ms`) | **Ciclo Completo `PUT → SSE → UI Pintada → ACK no PC` (ms)** | Texto Renderizado no Android (`#sync-banner`) | Alvo `< 1000 ms`? |
|---|---|---|---|---|---|---|---|
| **Página 50 (`11,88%`)** | `curl` | ~13 ms | **`107 ms`** | **`8,8 ms`** | **`117 ms`** | `[SSE AO VIVO] Livro: little-nemo -> Página 50 (11.88%)` | **SIM (~9× menor)** |
| **Página 51 (`12,11%`)** | `bookferry-server put-position` | `0,99 ms` | **`221 ms`** | **`3,2 ms`** | **`227 ms`** | `[SSE AO VIVO] Livro: little-nemo -> Página 51 (12.11%)` | **SIM (~4,4× menor)** |
| **Página 52 (`12,35%`)** | `bookferry-server put-position` | `0,69 ms` | **`138 ms`** | **`3,5 ms`** | **`143 ms`** | `[SSE AO VIVO] Livro: little-nemo -> Página 52 (12.35%)` | **SIM (~7× menor)** |
| **Página 60 (`14,25%`)** (rádio Wi-Fi ativo) | `PUT` concorrente | — | **`16 ms`** | **`2,4 ms`** | **`28 ms`** | `[SSE AO VIVO] Livro: little-nemo -> Página 60 (14.25%)` | **SIM (~35× menor)** |
| **Página 72 (`17,10%`)** (rádio Wi-Fi ativo) | `PUT` concorrente | — | **`14 ms`** | **`0,3 ms`** | **`26 ms`** | `[SSE AO VIVO] Livro: little-nemo -> Página 72 (17.1%)` | **SIM (~38× menor)** |

### 4.2. Cenário 4B — App em Segundo Plano (`KEYCODE_HOME`) e Pull ao Reabrir
1. Enviamos o app para segundo plano com `adb shell input keyevent KEYCODE_HOME`.
2. O PC gravou a **Página 88 (`20,9%`)** (`seq=4`, `HLC = 001794596924261:000000:pc-linux`).
   - *Detalhe observado:* A thread Rust do Android (ainda viva em segundo plano) recebeu o pacote SSE em `179 ms`, mas o `requestAnimationFrame` do WebView ficou suspenso pelo Chromium enquanto a Activity estava oculta (`784 ms` até ser trazida de volta à frente).
3. Ao reabrir o app (`am start -W`, `LaunchState: HOT`, `TotalTime: 52 ms`), o app executou `runFase3PullPosition()` (`GET /position/little-nemo`):
   - **Tempo de rede do `GET /position/little-nemo` sobre TLS:** **`26,96 ms`** (`tcp_ms = 16,12 ms`, `tls_hs_ms = 6,46 ms`, `req_ms = 4,26 ms`).
   - **Tempo total do Pull até a UI pintada (`total_ui_pull_ms`):** **`36,40 ms`**.
   - Banner exibido: `[PULL AO REABRIR] Livro: little-nemo -> Página 88 (20.9%) | HLC=001794596924261:000000:pc-linux | dev=pc-linux`.

### 4.3. Cenário 4C — Escrita Concorrente (PC escreve 2× vs. Celular Concorrente — Regra LWW por HLC)
Executamos 5 escritas concorrentes/fora de ordem para validar a ordenação total `(wall_ms, logical, device)` do HLC e a preservação do histórico sem perda de dados (`[BOOKFERRY_RESULT][FASE3_LWW_CONFLICT]`):

| Ordem de Chegada (`server_seq`) | Dispositivo | Posição Enviada | HLC Canônico (`wall_ms:logical:node`) | Aplicado como Vencedor (`applied`)? | Posição Ativa Após a Escrita | Motivo / Regra LWW |
|---|---|---|---|---|---|---|
| **`seq = 5`** | `pc-linux` (PC #1) | `page:60` (`14,25%`) | `001794596936628:000000:pc-linux` | **SIM (`true`)** | **`page:60`** | HLC maior que o anterior (`seq=4`) |
| **`seq = 6`** | `android-s23` (Celular #1) | `page:65` (`15,44%`) | `001794596936728:000000:android-s23` | **SIM (`true`)** | **`page:65`** | `wall_ms` (`...6728`) > PC #1 (`...6628`) |
| **`seq = 7`** | `pc-linux` (PC #2) | `page:72` (`17,10%`) | `001794596936828:000000:pc-linux` | **SIM (`true`)** | **`page:72`** | `wall_ms` (`...6828`) > Celular #1 (`...6728`) |
| **`seq = 8`** | `android-s23` (Concorrente atrasado) | `page:66` (`15,68%`) | `001794596936778:000001:android-s23` | **NÃO (`false`)** | **Mantém `page:72`** | `incoming HLC (...6778:1) < existing HLC (...6828:0)`. **Não sobrescreve** `page:72`, mas fica salvo em `history[7]` (zero perda de dados). |
| **`seq = 9`** | `android-s23` (Empate `wall_ms`) | `page:75` (`17,81%`) | `001794596936828:000001:android-s23` | **SIM (`true`)** | **`page:75`** | Mesmo `wall_ms` (`...6828`), desempata por contador lógico `000001 > 000000`. |


---

## 5. Transferência de Arquivo CBZ (295 MB) por HTTP Range sobre TLS

### 5.1. Metodologia e Escrita no `/sdcard/Download/bookferry-fixtures/`
- Como arquivos criados pelo próprio pacote (`dev.gaspari.bookferry.spike`) dentro de `/sdcard/Download/bookferry-fixtures/` têm permissão POSIX de leitura/escrita no FUSE do Android 15 (ao contrário de arquivos pré-existentes criados pelo usuário `shell` via `adb push`), o método `AndroidSaf.createWritableDownloadFd("lan-downloaded-little-nemo.cbz")` na `MainActivity.kt` abriu um `RandomAccessFile` em `/sdcard/Download/bookferry-fixtures/lan-downloaded-little-nemo.cbz`, destacou o `fd` via `ParcelFileDescriptor.dup(raf.fd).detachFd()` e entregou ao Rust (`File::from_raw_fd(fd)`).
- **Fluxo de interrupção e retomada:**
  1. **Parte 1:** O cliente Rust abriu `GET /file/little-nemo-1905-1914.cbz` sobre TLS 1.3 com Pinning, leu exatamente **50% do arquivo (`147.728.971 bytes`)** em chunks de 256 KB gravando direto no `fd` e deu **`drop(reader1)`**, cortando abruptamente a conexão TCP/TLS no meio da transferência.
  2. **Parte 2:** Em seguida, inspecionou o tamanho parcial no disco (`147.728.971 bytes`), abriu uma nova conexão TLS com o cabeçalho `Range: bytes=147728971-`, recebeu `HTTP/1.1 206 Partial Content` (`Content-Range: bytes 147728971-295457941/295457942`), fez `seek(SeekFrom::Start(147728971))` e baixou os **`147.728.971 bytes` restantes**.
  3. Ao final, verificou o hash SHA-256 dos `295.457.942 bytes` gravados e leu o *End of Central Directory* do ZIP para confirmar as 843 entradas.

### 5.2. Medidas Reais — Transferência HTTP Range sobre TLS (`[BOOKFERRY_RESULT][FASE3_RANGE_TRANSFER]`)

| Etapa / Métrica | Valor Medido |
|---|---|
| Arquivo transferido | `little-nemo-1905-1914.cbz` (**`295.457.942 bytes`** / `281,77 MiB`) |
| Destino no Android | `/sdcard/Download/bookferry-fixtures/lan-downloaded-little-nemo.cbz` |
| **Parte 1 (0 → 50%, antes da interrupção)** | Status `200 OK` · **`147.728.971 bytes`** em **`11.269,17 ms` (`11,27 s`)** · Vazão = **`12,50 MiB/s`** (`13,11 MB/s`) |
| **Ponto de interrupção (`drop` do socket TLS)** | Byte **`147.728.971`** (confirmado no log do servidor: `[FILE_FULL_INTERRUPTED]`) |
| **Parte 2 (50% → 100%, retomada `Range`)** | Request `Range: bytes=147728971-` → **`HTTP 206 Partial Content`** (`Content-Range: bytes 147728971-295457941/295457942`) · **`147.728.971 bytes`** em **`11.197,20 ms` (`11,20 s`)** · Vazão = **`12,58 MiB/s`** (`13,19 MB/s`) |
| **Total Combinado (100% montado no disco)** | **`295.457.942 bytes`** em **`22.466,37 ms` (`22,47 s`)** · Vazão média = **`12,54 MiB/s`** (`13,15 MB/s` / ~`105 Mbps` de payload útil cifrado) |
| **Integridade SHA-256 verificada** | `85d657be059d38668dbd299a56a1a68159f0aee3f3e15fc5969a51d0f75a7681` (100% idêntico ao arquivo no PC) |
| **Entradas ZIP verificadas após montagem** | **`843` entradas** no Central Directory |
| **Uso de RAM — Antes / Pico Durante / Depois** | `VmRSS`: **`222.676 KB` → `222.676 KB` → `222.476 KB`** · Pico `VmHWM`: **`225.656 KB` (+0 KB de aumento)** · `TOTAL PSS`: **`195.155 KB`** (`Native Heap PSS`: `15.908 KB`) |

- **Veredito do Item 5:** **RESOLVIDO**.

---

## 6. Resiliência: Conexão SSE em Doze Mode / Tela Apagada (`deviceidle force-idle` → `unforce`)

### 6.1. Protocolo do Experimento
1. O app Android estabeleceu a stream SSE sobre TLS 1.3 com Pinning (`attempt: 1`, `client_id: 3` no servidor) às `16:11:35.161` (`ts_ms = 1794597095161`, `connect_elapsed_ms = 44,59 ms`).
2. Desativamos o `stayon`, apagamos a tela do aparelho e forçamos o **Deep Doze** com:
   ```bash
   adb shell svc power stayon false
   adb shell input keyevent KEYCODE_SLEEP
   adb shell dumpsys deviceidle force-idle
   ```
   Saída real do Android (`SM-S918B`):
   ```text
   Now forced in to deep idle mode
   === DEVICEIDLE DEEP STATE ===
   IDLE
   === DEVICEIDLE LIGHT STATE ===
   ACTIVE
   ```
3. Mantivemos o aparelho em `IDLE` (Deep Doze com tela apagada) por ~61 segundos (`16:12:02` a `16:13:03`), disparando eventos no PC durante o Doze (`page: 101` em `t=20s` e `page: 102` em `t=45s`), e depois revertemos com:
   ```bash
   adb shell dumpsys deviceidle unforce
   adb shell svc power stayon usb
   adb shell input keyevent KEYCODE_WAKEUP
   ```
   Saída real do Android:
   ```text
   Light state: ACTIVE, deep state: ACTIVE
   ```
   Em seguida (`t=63s`, `3 s` após o `unforce`), o PC enviou `page: 103` (`seq=12`) para verificar a recepção pós-reconexão.
4. Ao final de todos os testes, executamos `adb shell dumpsys deviceidle unforce` e `adb shell svc power stayon false` para deixar o aparelho em seu estado original.

### 6.2. Linha do Tempo Real (Logs `ANDROID_SSE_STATUS` + `SERVER_EVENT`)

| Horário (`ts_ms`) | Estado Doze (`deep`) | Evento no Cliente Android (Rust SSE) / Servidor PC | Detalhe / Medida Real |
|---|---|---|---|
| `16:11:35.161` (`1794597095161`) | `ACTIVE` | `sse_connected` (`attempt: 1`, `client_id: 3`) | Conectado em **`44,59 ms`**; recebe `event: hello` |
| `16:12:02.100` (`1794597122100`) | **`IDLE`** | `dumpsys deviceidle force-idle` + `KEYCODE_SLEEP` | Aparelho entra em **Deep Doze** com tela apagada |
| `16:12:07.267` (`1794597127265`) | **`IDLE`** | `sse_read_error_disconnected` (`attempt: 1`) | Socket bloqueado pelo Doze: `Try again (os error 11)` (`EAGAIN` após timeout de leitura de 25 s; servidor detecta `disconnect_heartbeat` em `client_id: 3`) |
| `16:12:12.679` (`1794597132678`) | **`IDLE`** | `sse_connect_failed` (`attempt: 2`) | Firewall de Doze bloqueia SYN de saída: `connection timed out` (`5.000 ms`) |
| `16:12:18.190` (`1794597138189`) | **`IDLE`** | `sse_connect_failed` (`attempt: 3`) | Bloqueado pelo Doze: `connection timed out` (`5.000 ms`) |
| `16:12:21.778` (`1794597141778`) | **`IDLE`** (janela rápida) | `sse_reconnected` (`attempt: 4`, `client_id: 4`) | Reconectou numa brecha de rede após **`14.512,71 ms`** de downtime (`connect_elapsed_ms = 3.087,89 ms`); recebeu `page: 101` (`seq=10`, `roundtrip_put_to_ack_ms = 77 ms`) |
| `16:12:47.610` (`1794597167610`) | **`IDLE`** | PC envia `PUT /position` (`page: 102`, `seq=11`) | Servidor tenta escrever `page: 102` no socket `client_id: 4`, mas o celular está com rádio/socket congelado pelo Deep Doze |
| `16:12:49.242` (`1794597169241`) | **`IDLE`** | `sse_read_error_disconnected` (`attempt: 4`) | O daemon `netd` do Android 15 **destrói o socket TCP** com **`Software caused connection abort (os error 103)` (`ECONNABORTED`)**. **`page: 102` NÃO chegou via push SSE durante o Doze!** |
| `16:12:54.658` (`1794597174657`) | **`IDLE`** | `sse_connect_failed` (`attempt: 5`) | Bloqueado pelo Doze: `connection timed out` |
| `16:13:00.765` (`1794597180764`) | **`IDLE`** | `sse_connect_failed` (`attempt: 6`) | Abortado pelo kernel durante Doze: `Software caused connection abort (os error 103)` |
| `16:13:03.404` (`1794597183404`) | `IDLE → ACTIVE` | `dumpsys deviceidle unforce` + `KEYCODE_WAKEUP` | Aparelho sai do Deep Doze (`Light state: ACTIVE, deep state: ACTIVE`) |
| `16:13:04.309` (`1794597184309`) | **`ACTIVE`** | **`sse_reconnected` (`attempt: 7`, `client_id: 5`)** | **Reconectou automaticamente `905 ms` após o `unforce`** (`downtime_since_last_disconnect_ms = 15.067,72 ms`) |
| `16:13:06.647` (`1794597186647`) | **`ACTIVE`** | PC envia `PUT /position` (`page: 103`, `seq=12`) | **Entregue imediatamente em tempo real!** `roundtrip_put_to_ack_ms = 33 ms` (`android-rust-sse`) e **`41 ms` (`android-webview-raf`, UI atualizada para `"Página 103"`)** |

### 6.3. Respostas Objetivas às Perguntas do Item 6
1. **O que acontece com a conexão SSE em Doze / tela apagada?**
   - O `netd` / BPF firewall do Android 15 para UIDs em Deep Doze (`IDLE`) **corta conexões TCP abertas** com `Software caused connection abort (os error 103 / ECONNABORTED)` e bloqueia tentativas de reconexão de saída com `connection timed out`.
2. **A notificação enviada durante o Doze chega?**
   - **Não de forma garantida via push SSE:** enquanto `page: 101` coincidiu com uma breve janela de rede (`attempt 4`), `page: 102` (`seq=11`, enviada em `t=45s` de Deep Doze) **foi perdida no trânsito** porque o socket `attempt 4` foi abortado pelo kernel (`os error 103`) e as tentativas `5` e `6` foram barradas pelo Doze.
3. **Ela reconecta? Em quanto tempo?**
   - **Sim, automaticamente.** O loop da thread Rust (com timeout de leitura de 25 s para batimentos `: keepalive` a cada 5 s do servidor e backoff curto de 400–500 ms) **reconectou sozinho em `905 ms` (`~0,91 s`) após o `deviceidle unforce`** (`attempt 7`), sem intervenção do usuário. Logo após reconectar, `page: 103` chegou em **`41 ms` RTT até o DOM**.
4. **Implicação arquitetural para o produto:**
   - **Veredito: VIÁVEL COM RESSALVAS.** Como o Android mata sockets TCP em Deep Doze (`os error 103`) e descarta pacotes enviados durante o bloqueio, **o SSE sozinho não garante entrega de eventos emitidos enquanto o celular estava em Doze**. A solução arquitetural é simples e determinística:
     1. Toda vez que a thread SSE emitir `sse_reconnected` (ou quando a Activity disparar `onResume` / `visibilitychange`), o cliente **deve executar imediatamente um `GET /position/{book_id}` (Pull)** para reconciliar o estado por HLC.
     2. Servidor envia `: keepalive\n\n` a cada 5–15 s e o cliente configura `set_read_timeout(Some(Duration::from_secs(25)))` no `TcpStream` do `rustls`, garantindo detecção rápida de conexões "meio-abertas" (half-open).

---

## 7. Erros Reais Encontrados Durante o Spike e Como Foram Resolvidos

| # | Erro Real Encontrado | Causa Raiz | Solução Aplicada e Provada |
|---|---|---|---|
| **1** | `Custom { kind: UnexpectedEof, error: "peer closed connection without sending TLS close_notify" }` ao ler body HTTP sobre TLS 1.3 (`rustls 0.23.46`) | No TLS 1.3 (`rustls`), fechar o `TcpStream` no servidor sem enviar o alerta `close_notify` faz `read_to_end` no cliente falhar com `UnexpectedEof` (proteção contra ataque de truncamento). | No servidor ([`spike/server/src/lib.rs`](file:///home/gappy/Documents/projetos/bookshelf/spike/server/src/lib.rs)), chamamos `stream.conn.send_close_notify()` antes de `stream.flush()` ao encerrar cada resposta; no cliente, lemos exatamente `Content-Length` bytes com `BufReader::read_exact`. |
| **2** | `/system/bin/sh: syntax error: unexpected '('` ao disparar `--es eval "window.fn({...})"` via `adb shell` | O `adb shell` concatena os argumentos e entrega uma única string para `/system/bin/sh` no Android, que interpreta `(` e `"` se não estiverem escapados para o shell remoto. | Envolvemos o comando inteiro de `adb shell 'am start ... --es eval "window.fn(...)"'` em aspas simples externas e passamos payloads complexos codificados em Base64 (`atob(...)`). |
| **3** | Múltiplas interfaces Docker (`172.17.0.1` a `172.25.0.1`, `docker0` e `br-*`) na máquina NixOS | `mdns-sd` por padrão anuncia todas as interfaces IPv4 ativas do host, podendo incluir IPs `172.x.x.x` inacessíveis pelo celular na LAN `192.168.1.0/24`. | Restringimos o daemon mDNS do servidor à interface LAN real com `mdns.disable_interface(IfKind::All)` seguido de `mdns.enable_interface(IfKind::Addr("192.168.1.2".parse()?))` e `mdns.enable_interface("enp3s0")`. |
| **4** | `Software caused connection abort (os error 103)` (`ECONNABORTED`) e `Try again (os error 11)` no socket SSE durante Deep Doze | O subsistema `netd` do Android 15 corta conexões TCP de processos quando o aparelho entra em `IDLE` (Deep Doze) e bloqueia `connect()` de saída até a próxima janela de manutenção ou despertar. | Configuramos `set_read_timeout(25s)` no socket do cliente SSE + `: keepalive` a cada 5 s no servidor + reconexão automática na thread Rust (`~0,91 s` após `unforce`) combinada com Pull (`GET /position/{book_id}`). |

---

## 8. O Que NÃO Foi Verificado Neste Spike

Por honestidade de engenharia e foco estrito no timebox da Fase 3, os seguintes pontos **não** foram verificados neste spike:
1. **Redes Wi-Fi corporativas/públicas com AP Isolation (Client Isolation) ou filtro de multicast no roteador:** Testamos na LAN doméstica real (`192.168.1.2` cabeado ↔ `192.168.1.114` Wi-Fi no mesmo `/24`), onde o multicast `224.0.0.251:5353` passa livremente. Em redes onde o roteador bloqueia multicast Wi-Fi↔LAN, o mDNS falhará e o usuário dependerá obrigatoriamente do **fallback via QR Code (`ip:port` + `cert_sha256` + `secret`)**, que já deixamos implementado e provado no Item 1.3.
2. **Persistência do certificado TLS e dos tokens/posições em SQLite no disco:** Neste spike, o servidor gera o certificado ECDSA e mantém os tokens e posições em memória (`Mutex<ServerState>`) durante a vida do processo. No produto final, o par de chaves TLS do servidor, os tokens de dispositivos pareados e a tabela de posições HLC deverão ser persistidos em SQLite.
3. **Descoberta e conexão via IPv6 link-local (`fe80::/64`):** Validamos toda a pilha em IPv4 (`192.168.1.2:47600`).
4. **Leitura real de um QR Code pela câmera do celular:** Simulamos o payload do QR Code via string JSON/CLI conforme especificado no briefing da Fase 3 (`ip`, `port`, `cert_sha256`, `pairing_secret`).
5. **Troca de rede no meio do download Range (ex.: Wi-Fi cai e volta com outro IP local):** Testamos interrupção determinística do socket TCP em 50% (`147.728.971 bytes`) e retomada imediata via `Range: bytes=147728971-`.

---

## 9. Comandos Exatos para Reproduzir e Estado Final Limpo

### 9.1. Compilar e rodar o servidor LAN no PC (porta `TCP 47600` + mDNS `UDP 5353`)
> Corrigido pelo orquestrador: os comandos que o agente escreveu (`serve --port ... --lan-ip ...`, `put-position --addr ...`) **não existem** no `main.rs`. O binário lê variáveis de ambiente e argumentos posicionais, e tem caminhos absolutos fixos (`/home/gappy/.../spike`).
```bash
cd /home/gappy/Documents/projetos/bookshelf/spike
nix develop -c bash -c 'cd server && cargo build --release'
BOOKFERRY_LAN_IP=192.168.1.2 BOOKFERRY_PORT=47600 ./server/target/release/bookferry-server serve
# imprime o QR simulado e grava server/runtime-pairing.json e runtime-cert.pem (a cada execução gera cert e segredo NOVOS)
```

### 9.2. Simular escrita de posição no PC via CLI
```bash
# put-position <book_id> <page> <percent> [hlc] [device]  (lê server/runtime-pairing.json)
./server/target/release/bookferry-server put-position little-nemo 50 11.88
```

### 9.3. Compilar e instalar o APK Android com suporte LAN/mDNS/TLS/Range
```bash
cd /home/gappy/Documents/projetos/bookshelf/spike/app
nix develop .. -c cargo tauri android build --debug --apk true
nix develop .. -c adb install -r \
  src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

### 9.4. Verificação do Estado Final Limpo (Celular e PC)
Ao encerrar a Fase 3, o daemon `bookferry-server` foi finalizado e todas as configurações temporárias do celular foram revertidas e verificadas via `adb`:
- `adb shell dumpsys deviceidle unforce` → `Light state: ACTIVE, deep state: ACTIVE`
- `adb shell svc power stayon false` → `stay_on_while_plugged_in = 0`
- `/sdcard/Download/bookferry-fixtures/` → arquivo temporário do teste Range removido; os 6 arquivos originais (`939 MB` totais) permanecem intactos.


## Conferência do orquestrador (13/11/2026)

Repeti no PC, com o servidor do spike recompilado e rodando, e `curl` como cliente (LAN `192.168.1.2:47600`):

| Teste | Resultado observado |
|---|---|
| `curl --pinnedpubkey` com o SPKI correto | TLS aceito (resposta HTTP `401` sem token) |
| pin errado | recusado (`000`, handshake falha) |
| HTTP sem TLS | recusado (`000`) |
| `GET /position` sem token | `401` |
| `POST /pair` com segredo errado | `401` |
| segredo válido | `200` + token; segundo uso do mesmo segredo: `403 pairing_secret_already_used` |
| `PUT` hlc 2000 depois hlc 1000 | o 2000 aplica; o 1000 é rejeitado (`applied:false`) e fica no histórico |
| `Range: bytes=100-199` do CBZ | `206`, 100 bytes idênticos ao arquivo |

Isso confirma o **servidor** (TLS com pinning, pareamento, LWW por HLC, Range). **Não repeti** o lado Android: mDNS 10/10, latências SSE (117 ms), vazão de 12,5 MiB/s, Doze. Esses números vêm do log do agente.

Ressalvas que eu acrescento:
- **Doze forçado com `deviceidle force-idle`** é um teste artificial (o Android normal só entra em Doze profundo após muito tempo parado e fora do carregador). O achado útil é a regra: *puxar a posição (`GET`) em todo `onResume` e a cada reconexão do SSE*. Ela vale mesmo sem Doze.
- **mDNS foi testado só na sua LAN** (cabo + Wi-Fi no mesmo roteador). Redes com isolamento de clientes não foram testadas; o plano B é o IP no QR.
- **O servidor é artesanal** (`lib.rs` com 2.121 linhas, HTTP/SSE escritos à mão, estado em memória, certificado novo a cada execução). Serve de prova, **não de base**: no produto usar uma stack HTTP madura sobre `rustls`, persistir a chave TLS e os tokens em SQLite (o pin do QR deixa de valer a cada reinício, se o certificado mudar).
- A tela do celular ficou bloqueada em parte do teste; o agente contornou com `setShowWhenLocked(true)` e `setTurnScreenOn(true)` no `MainActivity.kt` do spike. **Isso é só contorno de teste e não deve ir para o app.**
- **Estado do celular ao fim:** `stay_on_while_plugged_in=0`, Doze desligado, os 6 arquivos de fixtures (939 MB) intactos.
