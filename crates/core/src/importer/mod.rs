//! Import de relevés exportés depuis l'espace bancaire.
//!
//! L'API DSP2 ne remonte que ce que la banque accepte d'exposer — 90 jours chez
//! LCL. Les relevés téléchargés, eux, remontent à l'ouverture du compte. Ce
//! module les lit pour compléter l'historique par le passé.
//!
//! Le format est déduit de l'extension : PDF pour les relevés LCL, CSV pour les
//! exports tabulaires.

pub mod csv;
pub mod lcl_pdf;

use anyhow::{Context, Result, bail};
use rust_decimal::Decimal;
use std::path::{Path, PathBuf};

use crate::model::Transaction;

/// Ce qu'a produit la lecture d'un fichier.
#[derive(Debug)]
pub struct ParsedFile {
    pub transactions: Vec<Transaction>,
    /// IBAN lu sur le relevé, quand le format en porte un.
    pub iban: Option<String>,
    /// Écart entre le solde recalculé et celui imprimé, quand le format permet
    /// ce contrôle. Non nul, il signale une lecture douteuse.
    pub discrepancy: Decimal,
}

/// Lit un fichier, quel que soit son format.
pub fn parse_file(path: &Path, account_id: &str, currency: &str) -> Result<ParsedFile> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();

    match extension.as_str() {
        "pdf" => {
            let statement = lcl_pdf::parse_file(path, account_id, currency)?;
            Ok(ParsedFile {
                transactions: statement.transactions,
                iban: statement.iban,
                discrepancy: statement.discrepancy,
            })
        }
        "csv" | "txt" | "tsv" => {
            let bytes =
                std::fs::read(path).with_context(|| format!("lecture de {}", path.display()))?;
            // Les exports bancaires français sont souvent en Latin-1 : décodé
            // en UTF-8 strict, un « é » ferait échouer la lecture du fichier
            // entier.
            let content = match String::from_utf8(bytes.clone()) {
                Ok(text) => text,
                Err(_) => bytes.iter().map(|&b| b as char).collect(),
            };
            Ok(ParsedFile {
                transactions: csv::parse(&content, account_id, currency)?,
                // Un CSV n'identifie ni le compte ni un solde de contrôle : la
                // vérification d'appartenance n'est pas possible.
                iban: None,
                discrepancy: Decimal::ZERO,
            })
        }
        "" => bail!(
            "{} n'a pas d'extension : format indéterminable",
            path.display()
        ),
        other => {
            bail!("format « {other} » non pris en charge. Attendu : .pdf (relevé LCL) ou .csv")
        }
    }
}

/// Énumère les fichiers à importer, qu'on ait reçu un fichier ou un dossier.
///
/// Le tri chronologique n'est pas cosmétique : les relevés se recouvrent parfois
/// d'un jour, et les traiter dans l'ordre rend la déduplication déterministe.
pub fn collect_files(target: &Path) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    if !target.is_dir() {
        bail!("{} est introuvable", target.display());
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(target)
        .with_context(|| format!("lecture du dossier {}", target.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_lowercase)
                        .as_deref(),
                    Some("pdf" | "csv" | "txt" | "tsv")
                )
        })
        .collect();

    if files.is_empty() {
        bail!(
            "aucun relevé dans {} — extensions attendues : .pdf, .csv",
            target.display()
        );
    }

    // Les relevés LCL portent leur date dans le nom : le tri alphabétique du
    // nom de fichier est donc chronologique.
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_extensions() {
        let error = parse_file(Path::new("/tmp/x.docx"), "acc", "EUR")
            .unwrap_err()
            .to_string();
        assert!(error.contains("docx"), "message inattendu : {error}");
    }

    #[test]
    fn rejects_a_missing_target() {
        let error = collect_files(Path::new("/tmp/inexistant-ecofin"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("introuvable"), "message inattendu : {error}");
    }
}
