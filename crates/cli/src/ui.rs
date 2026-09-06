//! Mise en forme pour le terminal : tableaux et montants.

use comfy_table::{Cell, CellAlignment, ContentArrangement, Table, presets};
use rust_decimal::Decimal;

/// Tableau simple, toutes les colonnes alignées à gauche.
pub fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    table_right_aligned(headers, rows, &[]);
}

/// Tableau dont les colonnes listées dans `right` sont alignées à droite,
/// ce qu'on veut pour les montants.
pub fn table_right_aligned(headers: &[&str], rows: Vec<Vec<String>>, right: &[usize]) {
    let mut table = Table::new();
    table
        .load_style(presets::UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers.iter().map(Cell::new));

    for row in rows {
        table.add_row(row.into_iter().enumerate().map(|(index, value)| {
            let cell = Cell::new(value);
            if right.contains(&index) {
                cell.set_alignment(CellAlignment::Right)
            } else {
                cell
            }
        }));
    }

    println!("{table}");
}

/// Formate un montant à la française : séparateur de milliers, virgule
/// décimale, symbole après le nombre.
pub fn money(amount: Decimal, currency: &str) -> String {
    let rounded = amount.round_dp(2);
    let negative = rounded.is_sign_negative();
    let text = rounded.abs().to_string();

    let (whole, decimals) = match text.split_once('.') {
        Some((w, d)) => (w.to_string(), format!("{d:0<2}")),
        None => (text, "00".to_string()),
    };

    let symbol = match currency {
        "EUR" => "€",
        "GBP" => "£",
        "USD" => "$",
        other => other,
    };

    format!(
        "{}{},{} {}",
        if negative { "-" } else { "" },
        group_thousands(&whole),
        decimals,
        symbol
    )
}

/// Insère une espace fine tous les trois chiffres, en partant de la droite.
fn group_thousands(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    out
}

/// Accorde un nom avec son nombre : « 1 opération », « 3 opérations ».
///
/// En français, zéro prend le singulier — et les noms en `-eau`, `-eu` ou `-au`
/// prennent un `x` : « créneaux », et non « créneaus ».
pub fn plural(count: usize, word: &str) -> String {
    if count <= 1 {
        return format!("{count} {word}");
    }
    let lower = word.to_lowercase();
    let suffix = match lower.ends_with("eau") || lower.ends_with("eu") || lower.ends_with("au") {
        true => "x",
        false => "s",
    };
    format!("{count} {word}{suffix}")
}

/// Coupe une chaîne trop longue, en comptant les caractères et non les octets :
/// les libellés bancaires contiennent des accents.
pub fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Mention du retrait des attentes périmées, tue quand il n'y en a pas.
///
/// Silencieuse dans le cas ordinaire : signaler « 0 régularisée » à chaque
/// synchronisation noierait l'information le jour où elle compte.
pub fn settled(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => ", 1 attente régularisée".to_string(),
        n => format!(", {n} attentes régularisées"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le cas ordinaire ne dit rien : annoncer « 0 régularisée » à chaque
    /// synchronisation noierait l'information le jour où elle compte.
    #[test]
    fn settled_says_nothing_when_there_is_nothing_to_say() {
        assert_eq!(settled(0), "");
        assert_eq!(settled(1), ", 1 attente régularisée");
        assert_eq!(settled(8), ", 8 attentes régularisées");
    }
    use std::str::FromStr;

    #[test]
    fn formats_positive_amount() {
        let amount = Decimal::from_str("1234.5").unwrap();
        assert_eq!(money(amount, "EUR"), "1\u{202f}234,50 €");
    }

    #[test]
    fn formats_negative_amount() {
        let amount = Decimal::from_str("-42.07").unwrap();
        assert_eq!(money(amount, "EUR"), "-42,07 €");
    }

    #[test]
    fn formats_whole_amount() {
        let amount = Decimal::from_str("1000000").unwrap();
        assert_eq!(money(amount, "EUR"), "1\u{202f}000\u{202f}000,00 €");
    }

    #[test]
    fn agrees_plural_with_count() {
        assert_eq!(plural(0, "opération"), "0 opération");
        assert_eq!(plural(1, "opération"), "1 opération");
        assert_eq!(plural(4, "opération"), "4 opérations");
    }

    /// Les noms en -eau, -eu et -au prennent un x au pluriel.
    #[test]
    fn agrees_nouns_ending_in_eau() {
        assert_eq!(plural(5, "créneau"), "5 créneaux");
        assert_eq!(plural(2, "cheveu"), "2 cheveux");
        assert_eq!(plural(1, "créneau"), "1 créneau");
        // Les autres gardent leur s.
        assert_eq!(plural(3, "relevé"), "3 relevés");
        assert_eq!(plural(2, "jour"), "2 jours");
    }

    #[test]
    fn truncates_on_characters_not_bytes() {
        assert_eq!(truncate("Crédit Agricole", 8), "Crédit…");
        assert_eq!(truncate("court", 20), "court");
    }
}
