use bookferry_core::{Hlc, PositionRecord, DEFAULT_MAX_FUTURE_DRIFT_MS};
use serde::{Deserialize, Serialize};

/// Limiar padrao (em pontos percentuais, 0..=100) acima do qual uma posicao remota
/// mais nova em outro dispositivo solicita confirmacao ao usuario (`ResumeDecision::Ask`).
pub const DEFAULT_RESUME_ASK_THRESHOLD_PERCENT: f64 = 2.0;

/// Decisao pura de retomada na abertura de um livro.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResumeDecision {
    /// Nenhuma posicao conhecida local nem remotamente.
    Nothing,
    /// Abrir diretamente na posicao local.
    UseLocal,
    /// Abrir diretamente na posicao remota (remoto mais novo, de outro dispositivo,
    /// e sem divergencia brusca com uma posicao local anterior).
    UseRemote,
    /// Remoto e mais novo (por HLC) e vem de OUTRO dispositivo, mas a diferenca de
    /// `percent` em relacao a posicao local excede o limiar configuravel (ex.: > 2 pontos
    /// percentuais). A UI deve exibir: "Continuar de onde parou no <device>?".
    Ask {
        remote: PositionRecord,
        local: PositionRecord,
    },
}

/// Avalia a decisao de retomada na abertura de um livro usando o limiar padrao
/// (`DEFAULT_RESUME_ASK_THRESHOLD_PERCENT = 2.0` pontos percentuais).
///
/// Regras:
/// 1. Sem posicao local nem remota -> `ResumeDecision::Nothing`.
/// 2. Apenas posicao local presente -> `ResumeDecision::UseLocal`.
/// 3. Apenas posicao remota presente -> `ResumeDecision::UseRemote` (desde que o HLC remoto
///    nao esteja em deriva futura absurda em relacao a `now_ms`; caso contrario, `Nothing`).
/// 4. Ambas presentes (`local` e `remote`):
///    - Se `remote` tem HLC invalido ou em deriva futura absurda (`> now_ms + 24h`) -> `UseLocal`.
///    - Se `remote.hlc <= local.hlc` (local mais novo ou igual por HLC) -> `UseLocal`.
///    - Se `remote.device_id == local.device_id` (mesmo dispositivo) -> `UseLocal`.
///    - Se `remote.locator == local.locator` e `(remote.percent - local.percent).abs() <= 1e-9` -> `UseLocal`.
///    - Se `remote.hlc > local.hlc` e `remote.device_id != local.device_id`:
///      - Se `(remote.percent - local.percent).abs() > DEFAULT_RESUME_ASK_THRESHOLD_PERCENT` ->
///        `ResumeDecision::Ask { remote, local }`.
///      - Caso contrario (`<= limiar`) -> `ResumeDecision::UseRemote`.
pub fn decide_resume(
    local: Option<&PositionRecord>,
    remote: Option<&PositionRecord>,
    now_ms: u64,
) -> ResumeDecision {
    decide_resume_with_threshold(local, remote, now_ms, DEFAULT_RESUME_ASK_THRESHOLD_PERCENT)
}

/// Avalia a decisao de retomada na abertura de um livro com limiar de divergencia
/// de percentual configuravel (`ask_threshold_percent`).
pub fn decide_resume_with_threshold(
    local: Option<&PositionRecord>,
    remote: Option<&PositionRecord>,
    now_ms: u64,
    ask_threshold_percent: f64,
) -> ResumeDecision {
    let threshold = if ask_threshold_percent.is_finite() && ask_threshold_percent >= 0.0 {
        ask_threshold_percent
    } else {
        DEFAULT_RESUME_ASK_THRESHOLD_PERCENT
    };

    match (local, remote) {
        (None, None) => ResumeDecision::Nothing,
        (Some(_), None) => ResumeDecision::UseLocal,
        (None, Some(rem)) => {
            if let Ok(rem_hlc) = Hlc::parse(&rem.hlc) {
                if rem_hlc
                    .validate_drift(now_ms, DEFAULT_MAX_FUTURE_DRIFT_MS)
                    .is_ok()
                {
                    ResumeDecision::UseRemote
                } else {
                    ResumeDecision::Nothing
                }
            } else {
                ResumeDecision::Nothing
            }
        }
        (Some(loc), Some(rem)) => {
            let Ok(rem_hlc) = Hlc::parse(&rem.hlc) else {
                return ResumeDecision::UseLocal;
            };
            if rem_hlc
                .validate_drift(now_ms, DEFAULT_MAX_FUTURE_DRIFT_MS)
                .is_err()
            {
                return ResumeDecision::UseLocal;
            }

            let Ok(loc_hlc) = Hlc::parse(&loc.hlc) else {
                return ResumeDecision::UseRemote;
            };

            // Se a posicao local e mais nova ou igual por HLC, ou se o remoto veio do mesmo device:
            if rem_hlc <= loc_hlc || rem.device_id == loc.device_id {
                return ResumeDecision::UseLocal;
            }

            let percent_diff = (rem.percent - loc.percent).abs();
            if rem.locator == loc.locator && percent_diff <= 1e-6 {
                return ResumeDecision::UseLocal;
            }

            if percent_diff > threshold {
                ResumeDecision::Ask {
                    remote: rem.clone(),
                    local: loc.clone(),
                }
            } else {
                ResumeDecision::UseRemote
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pos(device_id: &str, hlc: &str, locator: &str, percent: f64) -> PositionRecord {
        let parsed = Hlc::parse(hlc).unwrap();
        PositionRecord {
            book_id: "d41d8cd98f00b204e9800998ecf8427e-1000".to_string(),
            device_id: device_id.to_string(),
            hlc: parsed.to_canonical_string(),
            hlc_wall_ms: parsed.wall_ms,
            hlc_logical: parsed.logical,
            hlc_node: parsed.node,
            locator: locator.to_string(),
            snippet: None,
            percent,
            server_seq: 1,
            updated_at_ms: parsed.wall_ms,
        }
    }

    #[test]
    fn resume_decision_covers_all_variants() {
        let now = 1_700_000_010_000;

        // 1. Nenhuma posicao
        assert_eq!(decide_resume(None, None, now), ResumeDecision::Nothing);

        // 2. So local
        let local = sample_pos(
            "dev-android",
            "1700000001000:0:dev-android",
            "page:10",
            10.0,
        );
        assert_eq!(
            decide_resume(Some(&local), None, now),
            ResumeDecision::UseLocal
        );

        // 3. So remoto
        let remote_only = sample_pos("dev-pc", "1700000002000:0:dev-pc", "page:12", 12.0);
        assert_eq!(
            decide_resume(None, Some(&remote_only), now),
            ResumeDecision::UseRemote
        );

        // 4. Local mais novo que remoto -> UseLocal
        let remote_older = sample_pos("dev-pc", "1700000000500:0:dev-pc", "page:50", 50.0);
        assert_eq!(
            decide_resume(Some(&local), Some(&remote_older), now),
            ResumeDecision::UseLocal
        );

        // 5. Remoto mais novo, mas do MESMO device -> UseLocal
        let same_dev_newer = sample_pos(
            "dev-android",
            "1700000003000:0:dev-android",
            "page:40",
            40.0,
        );
        assert_eq!(
            decide_resume(Some(&local), Some(&same_dev_newer), now),
            ResumeDecision::UseLocal
        );

        // 6. Remoto mais novo, de OUTRO device, diferenca <= 2.0 pontos percentuais -> UseRemote
        let remote_small_step = sample_pos("dev-pc", "1700000002000:0:dev-pc", "page:11", 11.8);
        assert_eq!(
            decide_resume(Some(&local), Some(&remote_small_step), now),
            ResumeDecision::UseRemote
        );

        // 7. Remoto mais novo, de OUTRO device, diferenca > 2.0 pontos percentuais (avancou) -> Ask
        let remote_big_forward = sample_pos("dev-pc", "1700000002000:0:dev-pc", "page:50", 50.0);
        assert_eq!(
            decide_resume(Some(&local), Some(&remote_big_forward), now),
            ResumeDecision::Ask {
                remote: remote_big_forward.clone(),
                local: local.clone(),
            }
        );

        // 8. Remoto mais novo, de OUTRO device, diferenca > 2.0 pontos percentuais (voltou) -> Ask
        let remote_big_back = sample_pos("dev-pc", "1700000002000:0:dev-pc", "page:2", 2.0);
        assert_eq!(
            decide_resume(Some(&local), Some(&remote_big_back), now),
            ResumeDecision::Ask {
                remote: remote_big_back,
                local: local.clone(),
            }
        );

        // 9. Limiar customizado (ex.: 50.0%) faz com que diferenca de 40% vire UseRemote
        assert_eq!(
            decide_resume_with_threshold(Some(&local), Some(&remote_big_forward), now, 50.0),
            ResumeDecision::UseRemote
        );
    }
}
