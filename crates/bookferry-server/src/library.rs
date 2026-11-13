use crate::db::ScannedBook;
use crate::error::{ApiError, ServerError};
use bookferry_core::{BookFormat, BookId};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub scanned_books: Vec<ScannedBook>,
    pub duplicates_skipped: usize,
}

/// Varre `vault_dir` (somente leitura) recursivamente em ordem deterministica (lexicografica),
/// indexando apenas arquivos `.epub`, `.pdf` e `.cbz`.
///
/// Dois arquivos identicos produzem o mesmo `book_id` (`{partial_md5}-{size_bytes}`):
/// o primeiro caminho em ordem lexicografica relativa a `vault_dir` e mantido como
/// canonico e os demais sao contabilizados em `duplicates_skipped`.
pub fn scan_vault_dir(vault_dir: &Path) -> Result<ScanReport, ServerError> {
    if !vault_dir.exists() {
        return Err(ServerError::InvalidConfig(format!(
            "vault_dir nao existe: {}",
            vault_dir.display()
        )));
    }
    let canonical_vault = fs::canonicalize(vault_dir)?;
    let mut candidate_paths = Vec::new();
    collect_supported_files(&canonical_vault, &canonical_vault, &mut candidate_paths)?;

    // Ordena por caminho relativo para determinismo absoluto
    candidate_paths.sort_by(|a, b| a.0.cmp(&b.0));

    let mut seen_ids = HashSet::new();
    let mut report = ScanReport::default();

    for (rel_path, full_path, format) in candidate_paths {
        let meta = match fs::metadata(&full_path) {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let (book_id, partial_md5, size_bytes) = match BookId::from_file(&full_path) {
            Ok(res) => res,
            Err(err) => {
                tracing::warn!(path = %rel_path, error = %err, "falha ao ler livro durante scan");
                continue;
            }
        };

        if !seen_ids.insert(book_id.clone()) {
            tracing::debug!(
                book_id = %book_id,
                duplicate_rel_path = %rel_path,
                "arquivo identico duplicado ignorado (mesmo book_id)"
            );
            report.duplicates_skipped += 1;
            continue;
        }

        let title = full_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sem titulo")
            .to_string();

        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        report.scanned_books.push(ScannedBook {
            book_id,
            partial_md5,
            size_bytes,
            title,
            format,
            mtime_ms,
            rel_path,
        });
    }

    Ok(report)
}

fn collect_supported_files(
    canonical_vault: &Path,
    current_dir: &Path,
    out: &mut Vec<(String, PathBuf, BookFormat)>,
) -> Result<(), ServerError> {
    let mut entries: Vec<PathBuf> = fs::read_dir(current_dir)?
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();

    for path in entries {
        let Ok(canonical_entry) = fs::canonicalize(&path) else {
            continue;
        };
        // Nunca segue symlinks para fora de vault_dir
        if !canonical_entry.starts_with(canonical_vault) {
            continue;
        }
        let Ok(meta) = fs::metadata(&canonical_entry) else {
            continue;
        };
        if meta.is_dir() {
            collect_supported_files(canonical_vault, &canonical_entry, out)?;
        } else if meta.is_file() {
            let Some(format) = BookFormat::from_path(&canonical_entry) else {
                continue;
            };
            let Ok(rel) = canonical_entry.strip_prefix(canonical_vault) else {
                continue;
            };
            let rel_str = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel_str, canonical_entry, format));
        }
    }
    Ok(())
}

/// Resolve e valida que o arquivo do livro permanece estritamente dentro de `vault_dir`.
pub fn resolve_safe_vault_file(vault_dir: &Path, rel_path: &str) -> Result<PathBuf, ApiError> {
    let rel = Path::new(rel_path);
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            _ => {
                return Err(ApiError::bad_request(
                    "invalid_book_id",
                    "Caminho relativo de livro invalido",
                ));
            }
        }
    }

    let canonical_vault = fs::canonicalize(vault_dir)
        .map_err(|_| ApiError::internal("Diretorio da biblioteca indisponivel"))?;
    let candidate = canonical_vault.join(rel);
    let canonical_file = fs::canonicalize(&candidate)
        .map_err(|_| ApiError::not_found("book_not_found", "Arquivo do livro nao encontrado"))?;

    if !canonical_file.starts_with(&canonical_vault) {
        return Err(ApiError::bad_request(
            "invalid_book_id",
            "Acesso fora do vault_dir bloqueado",
        ));
    }

    Ok(canonical_file)
}
