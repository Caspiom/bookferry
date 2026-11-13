# `bookferry-core`

Núcleo de tipos e lógica pura do **Bookferry** (sem I/O de rede, compartilhável entre desktop Linux e cliente Android):

- **Relógio Lógico Híbrido (`Hlc`, `HlcClock`, `Clock`)**: ordenação total determinística `(wall_ms, logical, node)`, envio (`now`) e recebimento (`receive`), codificação textual ordenável lexicograficamente (`{wall_ms:015}:{logical:06}:{node}`) e rejeição de deriva futura excessiva (`> 24h`).
- **Resolução de Conflitos LWW (`LwwDecision`, `LwwRegister`)**: último-escritor-vence por HLC total, preservando escritas atrasadas num histórico limitado por livro (padrão: 20 entradas).
- **Identidade de Livro (`BookId`, `BookFormat`, `compute_partial_md5`)**: partial MD5 estilo KOReader (12 amostras de 1 KiB em offsets `1024 << 2i` para `i = -1..=10`) combinado ao tamanho em bytes (`{partial_md5}-{size}`), com validação estrita contra *path traversal*.
- **Posição de Leitura (`PositionUpdate`, `PositionRecord`)**: validação de `locator`, `snippet` (`<= 120` chars Unicode) e `percent` (`0.0..=100.0`).
- **Pareamento e Segurança (`PairingQrPayload`, `constant_time_eq_str`, `sha256_hex`)**: payload versionado do QR code, redação de segredos em `fmt::Debug` e comparação em tempo constante via `subtle`.
