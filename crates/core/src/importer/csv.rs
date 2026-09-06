//! Lecture d'un export CSV.
//!
//! Il n'existe pas de format standard : chaque banque produit son CSV, avec son
//! séparateur, ses en-têtes et parfois un préambule. Plutôt que de coder un
//! format par banque, on détecte la structure à la lecture.

use anyhow::{Result, bail};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::model::Transaction;

/// Séparateurs testés, du plus probable au moins probable en France.
const DELIMITERS: [char; 3] = [';', '\t', ','];

/// En-têtes reconnus pour chaque rôle de colonne, en minuscules et sans accent.
const DATE_HEADERS: [&str; 8] = [
    "date",
    "date operation",
    "date de l'operation",
    "date comptable",
    "date de comptabilisation",
    "date valeur",
    "date de valeur",
    "booking date",
];
const LABEL_HEADERS: [&str; 8] = [
    "libelle",
    "libelle operation",
    "description",
    "nature",
    "nature de l'operation",
    "intitule",
    "motif",
    "details",
];
const AMOUNT_HEADERS: [&str; 4] = ["montant", "amount", "montant operation", "valeur"];
const DEBIT_HEADERS: [&str; 3] = ["debit", "debit euro", "retrait"];
const CREDIT_HEADERS: [&str; 3] = ["credit", "credit euro", "depot"];

/// Structure repérée dans le fichier.
#[derive(Debug)]
struct Layout {
    delimiter: char,
    /// Index de la ligne d'en-tête, les lignes précédentes étant un préambule.
    header_row: usize,
    date: usize,
    label: usize,
    /// Colonne de montant signé, quand la banque n'en utilise qu'une.
    amount: Option<usize>,
    /// Colonnes débit et crédit séparées, l'autre convention courante.
    debit: Option<usize>,
    credit: Option<usize>,
}

/// Lit un relevé et en tire des opérations rattachées au compte donné.
pub fn parse(content: &str, account_id: &str, currency: &str) -> Result<Vec<Transaction>> {
    if content.trim_start().starts_with("OFXHEADER")
        || content.trim_start().starts_with("<?OFX")
        || content.contains("<STMTTRN>")
    {
        bail!(
            "ce fichier est au format OFX, que l'import ne lit pas encore. \
             Réexporte en CSV depuis ton espace bancaire."
        );
    }

    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        bail!("fichier vide");
    }

    let layout = detect_layout(&lines)?;
    let mut transactions = Vec::new();
    let mut ignored = 0usize;

    for line in lines.iter().skip(layout.header_row + 1) {
        let fields = split_line(line, layout.delimiter);
        match row_to_transaction(&fields, &layout, account_id, currency) {
            Ok(Some(tx)) => transactions.push(tx),
            // Une ligne de total ou de solde en fin de fichier n'est pas une
            // erreur : on la passe sans faire échouer tout l'import.
            Ok(None) => ignored += 1,
            Err(_) => ignored += 1,
        }
    }

    if transactions.is_empty() {
        bail!(
            "aucune opération lisible dans ce fichier ({ignored} lignes ignorées). \
             Vérifie qu'il s'agit bien d'un export d'opérations."
        );
    }

    Ok(transactions)
}

/// Repère séparateur, ligne d'en-tête et rôle des colonnes.
fn detect_layout(lines: &[&str]) -> Result<Layout> {
    // Les exports bancaires commencent souvent par des lignes d'identité du
    // compte : on cherche l'en-tête dans les premières lignes, pas seulement
    // dans la première.
    for (row, line) in lines.iter().enumerate().take(15) {
        for delimiter in DELIMITERS {
            let headers: Vec<String> = split_line(line, delimiter)
                .iter()
                .map(|h| normalize_header(h))
                .collect();
            if headers.len() < 2 {
                continue;
            }

            let date = find_header(&headers, &DATE_HEADERS);
            let amount = find_header(&headers, &AMOUNT_HEADERS);
            let debit = find_header(&headers, &DEBIT_HEADERS);
            let credit = find_header(&headers, &CREDIT_HEADERS);
            let label = find_header(&headers, &LABEL_HEADERS);

            // Il faut au minimum une date et de quoi reconstituer un montant.
            let has_amount = amount.is_some() || (debit.is_some() && credit.is_some());
            if let (Some(date), true) = (date, has_amount) {
                return Ok(Layout {
                    delimiter,
                    header_row: row,
                    date,
                    // Sans colonne de libellé identifiable, on prend la première
                    // colonne qui n'a pas déjà un rôle.
                    label: label.unwrap_or_else(|| {
                        (0..headers.len())
                            .find(|i| Some(*i) != Some(date) && Some(*i) != amount)
                            .unwrap_or(0)
                    }),
                    amount,
                    debit,
                    credit,
                });
            }
        }
    }

    bail!(
        "structure du fichier non reconnue : aucune ligne d'en-tête avec une \
         colonne de date et une colonne de montant.\n\
         En-têtes attendus, à l'accent et à la casse près : « Date » et \
         « Montant », ou « Date », « Débit » et « Crédit »."
    )
}

fn row_to_transaction(
    fields: &[String],
    layout: &Layout,
    account_id: &str,
    currency: &str,
) -> Result<Option<Transaction>> {
    let Some(raw_date) = fields.get(layout.date) else {
        return Ok(None);
    };
    let Some(date) = parse_date(raw_date) else {
        return Ok(None);
    };

    let amount = match layout.amount {
        Some(index) => match fields.get(index).and_then(|v| parse_amount(v)) {
            Some(value) => value,
            None => return Ok(None),
        },
        None => {
            // Convention débit/crédit : une seule des deux colonnes est
            // remplie, et le débit est écrit sans signe.
            let debit = layout
                .debit
                .and_then(|i| fields.get(i))
                .and_then(|v| parse_amount(v));
            let credit = layout
                .credit
                .and_then(|i| fields.get(i))
                .and_then(|v| parse_amount(v));
            match (debit, credit) {
                (Some(d), _) if !d.is_zero() => -d.abs(),
                (_, Some(c)) if !c.is_zero() => c.abs(),
                _ => return Ok(None),
            }
        }
    };

    let description = fields
        .get(layout.label)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(sans libellé)".to_string());

    Ok(Some(Transaction {
        // Un export ne porte pas de référence d'écriture : on fabrique un
        // identifiant à partir du contenu. Le préfixe distingue la provenance,
        // et la déduplication par date et montant empêche les doublons avec ce
        // que l'API a déjà rapporté.
        id: format!("import:{date}:{amount}:{description}"),
        account_id: account_id.to_string(),
        amount,
        currency: currency.to_string(),
        booking_date: Some(date),
        value_date: None,
        description,
        counterparty: None,
        bank_category: None,
        // Un relevé exporté ne contient que des opérations comptabilisées.
        booked: true,
    }))
}

/// Découpe une ligne CSV en respectant les guillemets.
fn split_line(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                // Guillemet échappé par doublement, selon la RFC 4180.
                current.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => {
                fields.push(current.trim().to_string());
                current = String::new();
            }
            c => current.push(c),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

/// Ramène un en-tête à une forme comparable : minuscules, sans accent ni
/// ponctuation. « Date de l'opération » et « DATE DE L'OPERATION » se
/// rejoignent ainsi.
fn normalize_header(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '_' | '-' => ' ',
            c => c,
        })
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '\'')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cherche la colonne dont l'en-tête correspond le mieux.
///
/// L'égalité exacte l'emporte sur l'inclusion, pour qu'une colonne « Date »
/// ne soit pas supplantée par « Date de valeur » quand les deux existent.
fn find_header(headers: &[String], candidates: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| candidates.contains(&h.as_str()))
        .or_else(|| {
            headers
                .iter()
                .position(|h| candidates.iter().any(|c| h.starts_with(c)))
        })
}

/// Lit une date dans les formats que produisent les banques françaises.
fn parse_date(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();
    for format in ["%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y", "%d.%m.%Y", "%d/%m/%y"] {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
            return Some(date);
        }
    }
    None
}

/// Lit un montant à la française : virgule décimale, espaces de milliers,
/// éventuel symbole monétaire, signe parfois placé en fin.
fn parse_amount(raw: &str) -> Option<Decimal> {
    let cleaned: String = raw
        .trim()
        // Espaces insécables et fines, employées comme séparateur de milliers.
        .replace(['\u{a0}', '\u{202f}', ' '], "")
        .replace(['€', '$', '£'], "");
    if cleaned.is_empty() {
        return None;
    }

    // Certains exports écrivent « 12,50- » pour un débit.
    let (body, trailing_negative) = match cleaned.strip_suffix('-') {
        Some(rest) => (rest.to_string(), true),
        None => (cleaned, false),
    };

    // Une virgule sert de séparateur décimal ; un point ne sert de séparateur
    // de milliers que si une virgule est également présente.
    let normalized = if body.contains(',') {
        body.replace('.', "").replace(',', ".")
    } else {
        body
    };

    let value = Decimal::from_str(&normalized).ok()?;
    Some(if trailing_negative { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_semicolon_export_with_signed_amount() {
        let csv = "Date;Libellé;Montant\n\
                   29/08/2026;CB KEBAB CORNER;-12,50\n\
                   28/08/2026;VIREMENT SALAIRE;2 450,00\n";
        let txs = parse(csv, "acc", "EUR").unwrap();

        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].amount, Decimal::from_str("-12.50").unwrap());
        assert_eq!(txs[0].description, "CB KEBAB CORNER");
        assert_eq!(txs[1].amount, Decimal::from_str("2450.00").unwrap());
        assert_eq!(
            txs[1].booking_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 28).unwrap())
        );
    }

    #[test]
    fn reads_separate_debit_and_credit_columns() {
        let csv = "Date;Libellé;Débit;Crédit\n\
                   29/08/2026;ACHAT;12,50;\n\
                   28/08/2026;SALAIRE;;2450,00\n";
        let txs = parse(csv, "acc", "EUR").unwrap();

        assert_eq!(txs.len(), 2);
        assert!(
            txs[0].is_debit(),
            "une colonne Débit donne un montant négatif"
        );
        assert_eq!(txs[0].amount, Decimal::from_str("-12.50").unwrap());
        assert!(!txs[1].is_debit());
    }

    /// Les exports commencent souvent par l'identité du compte.
    #[test]
    fn skips_preamble_before_the_header() {
        let csv = "Compte;12345678901\n\
                   Titulaire;M NOAH NYOUNAE\n\
                   \n\
                   Date;Libellé;Montant\n\
                   29/08/2026;ACHAT;-12,50\n";
        let txs = parse(csv, "acc", "EUR").unwrap();
        assert_eq!(txs.len(), 1);
    }

    #[test]
    fn ignores_trailing_total_rows() {
        let csv = "Date;Libellé;Montant\n\
                   29/08/2026;ACHAT;-12,50\n\
                   ;TOTAL;-12,50\n";
        let txs = parse(csv, "acc", "EUR").unwrap();
        assert_eq!(txs.len(), 1, "la ligne de total n'est pas une opération");
    }

    #[test]
    fn respects_quoted_fields_containing_the_delimiter() {
        let csv = "Date;Libellé;Montant\n\
                   29/08/2026;\"ACHAT; BOUTIQUE\";-12,50\n";
        let txs = parse(csv, "acc", "EUR").unwrap();
        assert_eq!(txs[0].description, "ACHAT; BOUTIQUE");
    }

    #[test]
    fn reads_comma_separated_iso_dates() {
        let csv = "Date,Description,Amount\n\
                   2026-08-29,COFFEE,-3.50\n";
        let txs = parse(csv, "acc", "EUR").unwrap();
        assert_eq!(txs[0].amount, Decimal::from_str("-3.50").unwrap());
    }

    #[test]
    fn parses_french_amount_conventions() {
        assert_eq!(
            parse_amount("1 234,56"),
            Some(Decimal::from_str("1234.56").unwrap())
        );
        assert_eq!(
            parse_amount("-12,50 €"),
            Some(Decimal::from_str("-12.50").unwrap())
        );
        assert_eq!(
            parse_amount("12,50-"),
            Some(Decimal::from_str("-12.50").unwrap())
        );
        assert_eq!(
            parse_amount("1.234,56"),
            Some(Decimal::from_str("1234.56").unwrap())
        );
        assert_eq!(
            parse_amount("3.50"),
            Some(Decimal::from_str("3.50").unwrap())
        );
        assert_eq!(parse_amount(""), None);
        assert_eq!(parse_amount("n/a"), None);
    }

    #[test]
    fn normalizes_accented_headers() {
        assert_eq!(
            normalize_header("Date de l'opération"),
            "date de l'operation"
        );
        assert_eq!(normalize_header("  MONTANT  "), "montant");
        assert_eq!(normalize_header("Débit"), "debit");
    }

    #[test]
    fn rejects_ofx_with_a_useful_message() {
        let error = parse("OFXHEADER:100\nDATA:OFSGML\n", "acc", "EUR")
            .unwrap_err()
            .to_string();
        assert!(error.contains("OFX"), "message inattendu : {error}");
    }

    #[test]
    fn rejects_unrecognised_structure() {
        let error = parse("a;b;c\n1;2;3\n", "acc", "EUR")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("non reconnue"),
            "message inattendu : {error}"
        );
    }
}
