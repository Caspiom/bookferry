# Segurança do servidor e vários aparelhos

Escrito em 15/11/2026. Cada afirmação diz se foi **verificada** (teste ou execução) ou só **lida no código**.

## 1. Quantos aparelhos falam com um servidor?

O "servidor" é o PC que tem a pasta de livros (o app desktop com o servidor embutido).

| Pergunta | Resposta | Como sei |
|---|---|---|
| Vários celulares no mesmo servidor? | **Sim.** Cada um pareia com o próprio QR (o segredo vale uma vez). Não há limite de aparelhos pareados no código. | Verificado: `one_server_serves_one_pc_and_two_phones_sharing_library_and_position` (`bookferry-client/tests/integration.rs`) |
| Vários PCs no mesmo servidor? | **Sim**, como cliente: o segundo PC cola o JSON do pareamento na tela de pareamento e **não** clica em "Iniciar servidor". | Verificado no nível da biblioteca (mesmo teste, um PC + dois celulares). **Não** testei a tela do segundo PC de ponta a ponta; o caminho da interface foi lido no código. |
| Todos veem a mesma biblioteca? | Sim, os 3 livros do teste apareceram iguais nos 3 aparelhos. | Verificado, mesmo teste |
| A posição de leitura é compartilhada? | Sim, e é **uma por livro para o servidor inteiro**. Se o celular 1 lê até a página 70, o PC e o celular 2 recebem na hora. | Verificado (mesmo teste) e lido: `positions.book_id` é a chave primária |
| Duas pessoas no mesmo servidor? | **Não separa.** Não existe usuário: as duas pessoas sobrescreveriam a posição uma da outra. Para pessoas diferentes, cada uma roda o próprio Bookferry (decisão do projeto, `00-briefing.md` item 6). | Lido no schema |
| Um celular com dois servidores (PC de casa e do trabalho)? | **Não ao mesmo tempo.** Há um único pareamento por aparelho: parear com o servidor B **substitui** o A. | Verificado: `a_device_talks_to_one_server_at_a_time` |

Detalhes que surpreendem:

- Ao trocar de servidor, o servidor A **não é avisado**: o aparelho continua na lista dele até alguém revogar lá (Ajustes, lista de aparelhos).
- Não existe tela para ligar o "modo só cliente" (`client_only_mode` existe no código, sem botão). Um PC sem pasta de livros configurada já se comporta como cliente; basta não iniciar servidor nele.
- O teto prático não é de aparelhos e sim de conexões simultâneas (seção 2): 256 no total.

## 2. Limites de conexão (endurecimento do servidor)

Antes disto, o servidor aceitava qualquer número de conexões e deixava abertas as que ficavam mudas. Na rede de casa isso quase não importa; com o servidor alcançável por mais gente (Wi-Fi compartilhado ou um relay futuro) importa. Todos os valores são campos de `Config` (`bookferry-server/src/lib.rs`).

| Defesa | Padrão | O que faz |
|---|---|---|
| `max_connections` | 256 | Teto global. Acima disso a conexão é fechada **antes** do TLS (não gasta CPU com handshake). |
| `max_connections_per_ip` | 64 | Teto por IP de origem; um host sozinho não consome o teto global. O loopback é isento, porque é o próprio app desktop falando com o servidor embutido. |
| `idle_timeout` | 30 s | Fecha a conexão sem nenhum byte lido/escrito. Cobre a conexão muda logo após o TLS e a ociosa entre requisições. **Não** conta enquanto há requisição em andamento (um rescan longo não é cortado). O SSE manda keepalive a cada 5 s, então nunca fica ocioso. |
| `header_read_timeout` | 10 s | Tempo máximo para um cliente que já começou a enviar terminar os cabeçalhos (conexão que pinga um byte por vez). |

Já existiam e continuam: handshake TLS com timeout de 10 s, corpo JSON limitado a 64 KB, bloqueio do `/pair` (5 erros bloqueiam por 60 s), comparação em tempo constante do segredo, token guardado só como hash SHA-256, banco com permissão `0600`, certificado fixado por hash no celular.

**Por que o `idle_timeout` é necessário:** o `header_read_timeout` do hyper só começa a contar depois do primeiro byte. Testei: com ele em 1 s, uma conexão muda continuou aberta por mais de 5 s. Sem o `idle_timeout`, 64 conexões mudas de um IP encheriam o teto e travariam o servidor, ou seja, o próprio teto viraria a brecha. O cliente do Bookferry abre uma conexão TLS nova por requisição (`bookferry-client/src/tls.rs`), então fechar conexão ociosa não o afeta.

### `/status` não revela mais metadados

Sem token válido, `GET /status` devolve só `{"status":"ok","version":"1"}`. Nome do servidor, quantidade de livros e tempo ligado só aparecem para um aparelho pareado. Com token inválido a resposta é a mesma enxuta (não vira 401, para o endpoint seguir útil como "o servidor está de pé?"). O nome do servidor continua visível no anúncio mDNS e no QR, na rede local, por design.

## 3. O que foi verificado

Todos em `cargo test --workspace`. Cada defesa foi **sabotada de propósito** (mutação manual) para ver o teste correspondente falhar; os arquivos foram restaurados depois.

| Teste | Sabotagem que ele pegou |
|---|---|
| `connection_cap_refuses_extra_connections_and_frees_slots` | teto global desligado |
| `per_ip_cap_applies_to_a_non_loopback_peer` (conecta ao IP da LAN da própria máquina, origem não-loopback) | teto por IP desligado |
| `loopback_is_exempt_from_per_ip_cap` | (garante que o app desktop não é barrado) |
| `silent_connection_is_closed_but_long_lived_sse_survives` | timeout de ociosidade removido; escrita deixando de contar como tráfego (o SSE cairia) |
| `limits::tests::idle_connection_expires_but_in_flight_request_and_traffic_keep_it_alive` | requisição em andamento deixando de contar |
| `status_hides_metadata_without_a_valid_token` | `/status` sempre completo |
| `one_server_serves_one_pc_and_two_phones_...` | servidor deixando de emitir o evento de posição |

## 4. O que NÃO está coberto

Isto importa **antes** de qualquer relay ou de abrir a porta na internet:

1. **Corpo de requisição lento:** o corpo JSON é limitado a 64 KB, mas não há timeout próprio para quem manda os cabeçalhos e depois pinga o corpo (um byte por intervalo menor que 30 s). O teto por IP limita o estrago a 64 conexões por origem.
2. **`/pair` bloqueia para todos:** o contador de erros é um só (não por IP). Quem alcança a porta consegue errar o segredo 5 vezes e impedir novos pareamentos por 60 s. Não vaza nada, só atrapalha. Adivinhar o segredo é inviável (256 bits aleatórios, uso único, expira em 5 min).
3. **Token errado não tem limite de tentativas.** O token tem 256 bits, então força bruta é inviável, mas cada tentativa custa uma consulta ao banco. O teto de conexões limita a vazão.
4. **Só IPv4:** o servidor escuta em `0.0.0.0`. IPv6 não foi testado.
5. **Por IP, atrás de um relay, todos parecem o mesmo IP.** O teto por IP só faz sentido na rede local; um relay precisaria de limites próprios por PC e autenticação de quem registra e de quem conecta.
6. **Testado só em loopback e no IP da própria máquina.** Não testei com um segundo aparelho real atacando, nem o comportamento do Android (Doze) com o `idle_timeout`. O SSE do celular reconecta sozinho, e o servidor manda keepalive a cada 5 s, bem abaixo dos 30 s.
7. **Não existe `cargo audit`/`cargo deny`** no ambiente: não conferi vulnerabilidades conhecidas nas dependências.

## 5. Para o relay (fase futura)

Resumo do que a análise de segurança concluiu, para quando o assunto voltar:

- Com o relay, qualquer pessoa na internet que conheça o identificador do PC alcança o servidor. Os itens 1, 2, 3 e 5 da seção 4 deixam de ser teóricos.
- O relay só deve repassar bytes e **nunca encerrar o TLS**: o celular continua fixando o certificado do PC e o relay não lê nem altera nada. Isso é uma promessa de projeto; ainda não existe código nem teste dela.
- O identificador do PC deve ser longo e aleatório (é a única chave de roteamento), e o relay não pode ser um proxy aberto.
- Mesmo sem ver o conteúdo, o relay vê IPs, horários e volume de tráfego: para público geral isso é dado pessoal (LGPD), então pede política de retenção e página de privacidade.
- Um servidor seu no meio contradiz o "sem conta e sem nuvem" do projeto; a proposta de valor precisaria ser reescrita.
