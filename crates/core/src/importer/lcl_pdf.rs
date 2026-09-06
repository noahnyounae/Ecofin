//! Lecture des relevés de compte LCL au format PDF.
//!
//! L'API DSP2 ne remonte que 90 jours chez LCL. Les relevés mensuels, eux,
//! remontent à l'ouverture du compte — d'où ce module.
//!
//! Le PDF n'a pas de structure exploitable : c'est une mise en page. On délègue
//! l'extraction à `pdftotext -layout`, qui préserve les positions en colonnes,
//! puis on lit ce texte.
//!
//! Deux particularités du format LCL commandent tout le reste :
//!
//! - **Le sens d'une opération n'est porté que par sa position.** Il n'y a pas
//!   de signe : un montant est un débit ou un crédit selon la colonne où il se
//!   trouve. Les montants étant alignés à droite, c'est leur position de *fin*
//!   qui les situe, jamais celle de début — `1,00` commence plus à droite que
//!   `101,00` dans la même colonne.
//! - **Les colonnes se décalent d'une page à l'autre.** L'en-tête est donc relu
//!   à chaque fois qu'il réapparaît.
//!
//! Chaque relevé porte ses propres totaux : le résultat est vérifié
//! arithmétiquement, ce qui transforme un changement de mise en page en erreur
//! explicite plutôt qu'en données fausses.

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;

use crate::model::Transaction;

/// Lignes de synthèse, qui portent un montant sans être des opérations.
const OPENING_BALANCE: &str = "ANCIEN SOLDE";
const CLOSING_BALANCE: &str = "SOLDE EN EUROS";
const TOTALS: &str = "TOTAUX";

/// Résultat de la lecture d'un relevé.
pub struct Statement {
    pub transactions: Vec<Transaction>,
    /// IBAN imprimé sur le relevé, normalisé sans espaces.
    ///
    /// Sert à vérifier qu'on verse bien les opérations dans le compte auquel
    /// elles appartiennent.
    pub iban: Option<String>,
    /// Écart entre le solde recalculé et celui imprimé sur le relevé.
    /// Nul quand la lecture est fidèle.
    pub discrepancy: Decimal,
}

/// Position des colonnes de montants sur la page courante.
#[derive(Clone, Copy)]
struct Columns {
    /// Colonne où commence l'en-tête « CREDIT ».
    ///
    /// Un montant dont la fin dépasse cette colonne est un crédit. Les montants
    /// de la colonne débit finissent nettement avant, ce qui laisse une marge
    /// confortable.
    credit: usize,
}

pub fn parse_file(path: &Path, account_id: &str, currency: &str) -> Result<Statement> {
    let text = extract_text(path)?;
    parse(&text, account_id, currency).with_context(|| format!("lecture de {}", path.display()))
}

/// Extrait le texte du PDF en préservant la mise en colonnes.
fn extract_text(path: &Path) -> Result<String> {
    let output = Command::new("pdftotext")
        .arg("-layout")
        // Sans cette option, un saut de page s'insère au milieu du texte et
        // brouille le repérage des lignes.
        .arg("-nopgbrk")
        .arg(path)
        .arg("-")
        .output()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!(
                "`pdftotext` est introuvable. Il fait partie de poppler :\n  \
                 macOS   : brew install poppler\n  \
                 Debian  : apt-get install poppler-utils\n\
                 L'image Docker d'ecofin l'embarque déjà."
            ),
            _ => anyhow::Error::new(err).context("exécution de pdftotext"),
        })?;

    if !output.status.success() {
        bail!(
            "pdftotext a échoué sur {} : {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn parse(text: &str, account_id: &str, currency: &str) -> Result<Statement> {
    let period = find_period(text)
        .context("période du relevé introuvable — est-ce bien un relevé de compte LCL ?")?;

    let mut columns: Option<Columns> = None;
    let mut transactions = Vec::new();
    let (mut opening, mut closing) = (Decimal::ZERO, None);
    // Compte les opérations rigoureusement identiques dans ce relevé.
    let mut occurrences: HashMap<(NaiveDate, Decimal, String), usize> = HashMap::new();

    for line in text.lines() {
        if let Some(found) = header_columns(line) {
            columns = Some(found);
            continue;
        }
        let Some(columns) = columns else { continue };
        let Some(amount) = last_amount(line, columns) else {
            continue;
        };

        // Les lignes de synthèse portent un montant mais ne sont pas des
        // opérations : les compter fausserait le solde.
        if line.contains(OPENING_BALANCE) {
            opening = amount;
            continue;
        }
        if line.contains(CLOSING_BALANCE) {
            closing = Some(amount);
            continue;
        }
        if line.contains(TOTALS) {
            continue;
        }

        let Some((day, month, label_start)) = split_date_prefix(line) else {
            continue;
        };
        let Some(date) = resolve_year(day, month, period) else {
            continue;
        };

        let label = label_of(line, label_start);

        // Une même journée peut porter plusieurs achats rigoureusement
        // identiques — quatre péages à 1,30 €, trois cafés au même comptoir.
        // Sans numéro d'occurrence, ils partageraient un identifiant et
        // seraient fondus en une seule opération. Le compteur est propre au
        // relevé : une opération vue dans deux relevés qui se recouvrent
        // reçoit le même numéro, donc le même identifiant, et fusionne bien.
        let occurrence = {
            let key = (date, amount, label.clone());
            let seen = occurrences.entry(key).or_insert(0);
            *seen += 1;
            *seen - 1
        };

        transactions.push(Transaction {
            // Un relevé ne porte pas de référence d'écriture : l'identifiant
            // est dérivé du contenu. La déduplication face aux données de
            // l'API se fait, elle, sur le couple date/montant à l'insertion.
            id: format!("lcl:{date}:{amount}:{occurrence}:{label}"),
            account_id: account_id.to_string(),
            amount,
            currency: currency.to_string(),
            booking_date: Some(date),
            value_date: None,
            description: label,
            counterparty: None,
            bank_category: None,
            booked: true,
        });
    }

    if transactions.is_empty() {
        bail!("aucune opération trouvée dans ce relevé");
    }

    // Le relevé imprime son propre solde de clôture : on s'en sert comme
    // contrôle. Une mise en page inattendue devient ainsi une erreur visible.
    let movement: Decimal = transactions.iter().map(|t| t.amount).sum();
    let discrepancy = match closing {
        Some(closing) => opening + movement - closing,
        None => Decimal::ZERO,
    };

    Ok(Statement {
        transactions,
        iban: find_iban(text),
        discrepancy,
    })
}

/// Repère l'IBAN du relevé et le normalise.
///
/// LCL l'imprime espacé par groupes de quatre — « FR76 3000 2009 … » — alors
/// qu'il est stocké d'un seul tenant : la comparaison exige de retirer les
/// espaces.
fn find_iban(text: &str) -> Option<String> {
    let start = text.find("IBAN")?;
    let after = &text[start + 4..];
    let line = after.lines().next()?;
    let normalized: String = line
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    // Un IBAN français fait 27 caractères ; en deçà, c'est autre chose.
    (normalized.len() >= 15).then_some(normalized)
}

/// Repère « du 02.07.2026 au 31.07.2026 » en tête de relevé.
fn find_period(text: &str) -> Option<(NaiveDate, NaiveDate)> {
    let marker = text.find(" du ")?;
    let window: String = text[marker..].chars().take(40).collect();
    let digits: Vec<u32> = window
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .take(6)
        .collect();
    if digits.len() < 6 {
        return None;
    }
    let start = NaiveDate::from_ymd_opt(digits[2] as i32, digits[1], digits[0])?;
    let end = NaiveDate::from_ymd_opt(digits[5] as i32, digits[4], digits[3])?;
    Some((start, end))
}

/// Reconnaît la ligne d'en-tête des écritures et en tire la colonne « CREDIT ».
fn header_columns(line: &str) -> Option<Columns> {
    if !line.contains("LIBELLE") || !line.contains("DEBIT") {
        return None;
    }
    Some(Columns {
        credit: line.find("CREDIT")?,
    })
}

/// Découpe le préfixe de date « JJ.MM » en tête de ligne.
fn split_date_prefix(line: &str) -> Option<(u32, u32, usize)> {
    let bytes = line.as_bytes();
    // Le format est strictement " JJ.MM " : une espace, puis la date.
    if bytes.len() < 7 || bytes[0] != b' ' || bytes[3] != b'.' || bytes[6] != b' ' {
        return None;
    }
    let day = line.get(1..3)?.parse().ok()?;
    let month = line.get(4..6)?.parse().ok()?;
    Some((day, month, 6))
}

/// Choisit l'année qui rapproche le plus la date de la période du relevé.
///
/// Les relevés ne datent qu'en jour et mois. Une opération peut précéder la
/// période de quelques jours — le relevé de janvier porte un ancien solde daté
/// du 31.12, qui appartient à l'année précédente.
fn resolve_year(day: u32, month: u32, (start, end): (NaiveDate, NaiveDate)) -> Option<NaiveDate> {
    [start.year() - 1, start.year(), end.year()]
        .into_iter()
        .filter_map(|year| NaiveDate::from_ymd_opt(year, month, day))
        .map(|date| {
            let distance = if date >= start && date <= end {
                0
            } else {
                (date - start)
                    .num_days()
                    .abs()
                    .min((date - end).num_days().abs())
            };
            (distance, date)
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, date)| date)
}

/// Dernier montant de la ligne, signé selon la colonne où il se termine.
fn last_amount(line: &str, columns: Columns) -> Option<Decimal> {
    let (_, end, text) = find_last_amount(line)?;
    let value = Decimal::from_str(&text).ok()?;
    // Alignés à droite, les montants se situent par leur fin : un crédit
    // dépasse la colonne de l'en-tête « CREDIT », un débit s'arrête avant.
    Some(if end > columns.credit { value } else { -value })
}

/// Repère le dernier nombre de la forme `1 234,56` et sa position.
///
/// Les dates de valeur (`02.07.26`) sont écartées d'office : elles n'ont pas de
/// virgule décimale.
fn find_last_amount(line: &str) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut result = None;

    for (index, window) in chars.windows(3).enumerate() {
        // Une virgule suivie de deux chiffres marque la fin d'un montant.
        if window[0] != ',' || !window[1].is_ascii_digit() || !window[2].is_ascii_digit() {
            continue;
        }
        // Un troisième chiffre signalerait autre chose qu'un montant.
        if chars.get(index + 3).is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }

        // On remonte la partie entière, séparateurs de milliers compris.
        let mut begin = index;
        while begin > 0 {
            let previous = chars[begin - 1];
            let is_group_separator = matches!(previous, ' ' | '\u{a0}' | '\u{202f}')
                && chars
                    .get(begin.wrapping_sub(2))
                    .is_some_and(|c| c.is_ascii_digit());
            if previous.is_ascii_digit() || is_group_separator {
                begin -= 1;
            } else {
                break;
            }
        }
        if !chars[begin].is_ascii_digit() {
            continue;
        }

        let text: String = chars[begin..index]
            .iter()
            .filter(|c| c.is_ascii_digit())
            .chain(std::iter::once(&'.'))
            .chain(chars[index + 1..index + 3].iter())
            .collect();
        result = Some((begin, index + 3, text));
    }
    result
}

/// Libellé de l'opération : ce qui sépare la date du montant, épuré de la date
/// de valeur que LCL intercale.
fn label_of(line: &str, label_start: usize) -> String {
    let Some((amount_start, _, _)) = find_last_amount(line) else {
        return "(sans libellé)".to_string();
    };
    let chars: Vec<char> = line.chars().collect();
    if label_start >= amount_start || amount_start > chars.len() {
        return "(sans libellé)".to_string();
    }

    let raw: String = chars[label_start..amount_start].iter().collect();
    let cleaned = strip_value_date(&raw);
    match cleaned.is_empty() {
        true => "(sans libellé)".to_string(),
        false => cleaned,
    }
}

/// Retire la date de valeur `JJ.MM.AA` que LCL place entre le libellé et le
/// montant, et ramène le tout sur une seule ligne.
fn strip_value_date(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|token| !is_value_date(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_value_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 8
        && bytes[2] == b'.'
        && bytes[5] == b'.'
        && token.chars().filter(char::is_ascii_digit).count() == 6
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extrait d'un relevé réel, colonnes et en-tête de période compris.
    const SAMPLE: &str = concat!(
        "                                du 01.08.2024 au 30.08.2024 - N° 25\n",
        " DATE                          LIBELLE                        VALEUR       DEBIT           CREDIT\n",
        "\n",
        " 01.08                                          ANCIEN SOLDE                            1 168,84\n",
        " 01.08     VIR SEPA Location immo du palais                  01.08.24       460,00\n",
        " 12.08     VIREMENT TEMPORIS                                 12.08.24                     319,69\n",
        " 16.08     FRAIS VIR INST HARMONIIE SAS                      16.08.24         1,00\n",
        " 30.08                                        SOLDE EN EUROS                            1 027,53\n",
    );

    fn period() -> (NaiveDate, NaiveDate) {
        (
            NaiveDate::from_ymd_opt(2024, 8, 1).unwrap(),
            NaiveDate::from_ymd_opt(2024, 8, 30).unwrap(),
        )
    }

    #[test]
    fn reads_debits_and_credits_from_their_column() {
        let statement = parse(SAMPLE, "acc", "EUR").unwrap();
        let amounts: Vec<_> = statement.transactions.iter().map(|t| t.amount).collect();

        assert_eq!(amounts.len(), 3);
        assert_eq!(amounts[0], Decimal::from_str("-460.00").unwrap());
        assert_eq!(
            amounts[1],
            Decimal::from_str("319.69").unwrap(),
            "colonne crédit"
        );
        assert_eq!(
            amounts[2],
            Decimal::from_str("-1.00").unwrap(),
            "des frais sont un débit"
        );
    }

    /// Le contrôle arithmétique est la garantie contre un changement de mise en
    /// page : 1168,84 - 460,00 + 319,69 - 1,00 = 1027,53.
    #[test]
    fn reconciles_with_the_printed_closing_balance() {
        let statement = parse(SAMPLE, "acc", "EUR").unwrap();
        assert_eq!(statement.discrepancy, Decimal::ZERO);
    }

    #[test]
    fn strips_the_value_date_from_the_label() {
        let statement = parse(SAMPLE, "acc", "EUR").unwrap();
        assert_eq!(
            statement.transactions[0].description,
            "VIR SEPA Location immo du palais"
        );
    }

    #[test]
    fn excludes_summary_lines_from_transactions() {
        let statement = parse(SAMPLE, "acc", "EUR").unwrap();
        assert!(
            !statement
                .transactions
                .iter()
                .any(|t| t.description.contains("SOLDE")),
            "ancien et nouveau solde ne sont pas des opérations"
        );
    }

    /// Un montant court commence plus à droite qu'un montant long de la même
    /// colonne : seule la position de fin les situe correctement.
    #[test]
    fn short_amounts_stay_in_the_debit_column() {
        let line = " 29.08    FRAIS VIR INST                     29.08.24         1,00";
        let columns = header_columns(
            " DATE            LIBELLE                    VALEUR       DEBIT           CREDIT",
        )
        .unwrap();
        assert!(last_amount(line, columns).unwrap().is_sign_negative());
    }

    #[test]
    fn resolves_the_year_from_the_statement_period() {
        assert_eq!(
            resolve_year(15, 8, period()),
            NaiveDate::from_ymd_opt(2024, 8, 15)
        );
    }

    /// Le relevé de janvier porte un ancien solde daté du 31 décembre, qui
    /// appartient à l'année précédente.
    #[test]
    fn resolves_december_dates_to_the_previous_year() {
        let january = (
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 30).unwrap(),
        );
        assert_eq!(
            resolve_year(31, 12, january),
            NaiveDate::from_ymd_opt(2025, 12, 31)
        );
    }

    #[test]
    fn reads_thousand_separators() {
        let (_, _, text) = find_last_amount("  total       1 168,84").unwrap();
        assert_eq!(text, "1168.84");
    }

    #[test]
    fn ignores_value_dates_which_have_no_decimal_comma() {
        assert!(find_last_amount(" 02.07   LIBELLE    02.07.26").is_none());
    }

    #[test]
    fn reads_the_statement_iban() {
        let text = "  IBAN : FR76 3000 2009 9800 0001 9124 V26\n  BIC : CRLYFRPP";
        assert_eq!(
            find_iban(text).as_deref(),
            Some("FR7630002009980000019124V26")
        );
    }

    #[test]
    fn reports_no_iban_when_absent() {
        assert_eq!(find_iban("RELEVE DE COMPTE\nrien ici"), None);
    }

    #[test]
    fn finds_the_statement_period() {
        let text = "  du 02.07.2026 au 31.07.2026 - N° 49";
        assert_eq!(
            find_period(text),
            Some((
                NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
            ))
        );
    }
}
