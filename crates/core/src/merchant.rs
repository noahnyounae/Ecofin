//! Nettoyage des libellés bancaires.
//!
//! Les banques composent leurs libellés pour un relevé papier, en y empilant le
//! type d'opération, le numéro de carte, le nom du porteur, des dates et des
//! références internes :
//!
//! ```text
//! CARTE 0602685 CB POINT P 1233 26/08/26 CBLM NOAH NYOUNAE
//! ```
//!
//! Le seul mot utile est « POINT P ». Ce module retire le reste.
//!
//! **Tout y est déterministe et local.** Une même entrée donne toujours la même
//! sortie, et chaque transformation se lit dans le code : sur des données
//! financières, un libellé bruyant mais vrai vaut mieux qu'un libellé net et
//! inventé.
//!
//! Ce que ce module ne fait pas, faute de connaissance du monde : réunir
//! `MC DONALDS 21`, `MAC DONALD'S` et `MC DO CHARCOT` sous un même nom, ou
//! deviner ce que désigne `CDIL`.

/// Marqueurs de type d'opération, retirés seulement en tête de libellé.
///
/// La liste est courte à dessein. Y ajouter `ASSURANCE` ou `PRET` viderait
/// entièrement « ASSURANCE MOYEN DE PAIEMENT » ou « ECHEANCE PRET PERSONNEL »,
/// dont c'est tout le sens — erreur commise puis corrigée en éprouvant ces
/// règles sur des libellés réels.
const OPERATION_PREFIXES: [&str; 11] = [
    "CB",
    "CARTE",
    "VIR",
    "VIREMENT",
    "INSTANTANE",
    "PRLV",
    "PRELVT",
    "PRELEVEMENT",
    "SEPA",
    "INST",
    "RECU",
];

/// Formules toutes faites, retirées en tant que phrases entières.
///
/// Le retrait est fait au niveau de la phrase et non du mot : ôter `CREDIT`
/// isolément mutilerait `CREDIT AGRICOLE` ou tout commerçant portant ce mot.
const BOILERPLATE_PHRASES: [&str; 5] = [
    "EMETTEUR CREDIT BENEF.",
    "EMETTEUR CREDIT BENEF",
    "DONNEUR D'ORDRE",
    // LCL a changé de format en cours de route : « DEBIT DIVERS ASSURANCE
    // MOYEN DE PAIEMENT » puis « ASSURANCE MOYEN DE PAIEMENT ». Sans ce
    // retrait, une même dépense se scinde en deux abonnements distincts.
    "DEBIT DIVERS",
    "RECU D/O",
];

/// Qualificatifs porteurs de sens, conservés en tête mais qui ne doivent pas
/// interrompre le retrait des marqueurs qui les suivent.
///
/// « FRAIS VIR INST HARMONIIE SAS » désigne des frais sur un virement instantané :
/// « FRAIS » est l'information, « VIR INST » le bruit. S'arrêter au premier mot
/// non-marqueur laisserait les deux.
const QUALIFIERS: [&str; 3] = ["FRAIS", "COMMISSION", "REMISE"];

/// Champs structurés que certaines banques accolent au libellé.
///
/// `CREANCIER INITIAL` est traité à part : il porte le nom du véritable
/// créancier, souvent plus parlant que le libellé lui-même.
const CREDITOR_FIELD: &str = "CREANCIER INITIAL:";

/// Champs à couper, contenu compris : ce sont des références opaques.
const NOISE_FIELDS: [&str; 5] = [
    "REF.CLIENT:",
    "ID.CREANCIER:",
    "REF.MANDAT:",
    "LIBELLE:",
    "MOTIF:",
];

/// Longueur à partir de laquelle une suite de chiffres est tenue pour une
/// référence.
///
/// En deçà, on préserve : le « 69 » de `TCL 69 LYO` situe la ligne, le « 21 »
/// de `MC DONALDS 21` distingue le restaurant.
const REFERENCE_DIGITS: usize = 4;

/// Réduit un libellé à une forme comparable : sans casse ni accents.
///
/// Sert de clé pour regrouper ce qui désigne le même bénéficiaire. Les banques
/// ne sont pas constantes dans leur typographie — « SCI Berthelot » un mois,
/// « SCI BERTHELOT » le suivant — et sans ce repli, une même dépense se scinde
/// en deux.
pub fn fold(label: &str) -> String {
    label
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect::<String>()
        .to_uppercase()
}

/// Rend un libellé bancaire lisible.
///
/// Ne renvoie jamais de chaîne vide : si les règles retirent tout, c'est
/// qu'elles se trompent, et le libellé d'origine reprend ses droits. Un
/// libellé encombré reste préférable à un libellé absent.
pub fn normalize(raw: &str) -> String {
    let collapsed = collapse(raw);
    if collapsed.is_empty() {
        return "(sans libellé)".to_string();
    }

    // Le créancier réel prime : sur un prélèvement, « Regie d'eau potable du
    // grand Lyon » vaut mieux que la raison sociale de l'organisme collecteur.
    if let Some(creditor) = extract_creditor(&collapsed) {
        return creditor;
    }

    let candidate = strip_all(&collapsed);
    match candidate.is_empty() {
        true => collapsed,
        false => candidate,
    }
}

/// Applique les règles de retrait, sans garantie de non-vacuité.
fn strip_all(text: &str) -> String {
    let text = drop_boilerplate(text);
    let text = cut_noise_fields(&text);
    let text = cut_cardholder(&text);
    let tokens = drop_references(&text);
    let tokens = drop_leading_prefixes(tokens);
    let tokens = drop_currency_echo(tokens);
    collapse_repetition(&tokens.join(" "))
}

/// Retire les formules toutes faites, où qu'elles se trouvent.
fn drop_boilerplate(text: &str) -> String {
    let mut result = text.to_string();
    for phrase in BOILERPLATE_PHRASES {
        while let Some(at) = find_ignoring_case(&result, phrase) {
            result.replace_range(at..at + phrase.len(), " ");
        }
    }
    collapse(&result)
}

/// Réduit toute suite d'espaces à une seule.
fn collapse(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extrait le nom du créancier d'un prélèvement SEPA.
fn extract_creditor(text: &str) -> Option<String> {
    let start = find_ignoring_case(text, CREDITOR_FIELD)? + CREDITOR_FIELD.len();
    let rest = &text[start..];

    // Le nom court jusqu'au champ structuré suivant.
    let end = NOISE_FIELDS
        .iter()
        .filter_map(|field| find_ignoring_case(rest, field))
        .min()
        .unwrap_or(rest.len());

    let name = collapse(&rest[..end]);
    (!name.is_empty()).then_some(name)
}

/// Coupe le libellé au premier champ de référence rencontré.
fn cut_noise_fields(text: &str) -> String {
    let cut = NOISE_FIELDS
        .iter()
        .filter_map(|field| find_ignoring_case(text, field))
        .min()
        .unwrap_or(text.len());
    text[..cut].trim().to_string()
}

/// Retire la mention du porteur de carte, que LCL accole en fin de libellé.
///
/// On repère le marqueur `CBLM` plutôt que le nom lui-même : la règle vaut
/// alors pour n'importe quel titulaire, sans rien coder en dur.
fn cut_cardholder(text: &str) -> String {
    match find_token(text, "CBLM") {
        Some(at) => text[..at].trim().to_string(),
        None => text.to_string(),
    }
}

/// Écarte dates incrustées et longues références numériques.
fn drop_references(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|token| !is_date(token) && !is_reference(token))
        .map(String::from)
        .collect()
}

/// Retire les marqueurs de type d'opération, tant qu'ils ouvrent le libellé.
fn drop_leading_prefixes(tokens: Vec<String>) -> Vec<String> {
    let mut kept = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let upper = tokens[index].to_uppercase();
        if OPERATION_PREFIXES.contains(&upper.as_str()) {
            index += 1;
            continue;
        }
        // Un qualificatif est conservé, mais le retrait continue après lui.
        if kept.is_empty() && QUALIFIERS.contains(&upper.as_str()) {
            kept.push(tokens[index].clone());
            index += 1;
            continue;
        }
        break;
    }

    kept.extend_from_slice(&tokens[index..]);
    // Tout retirer signifierait que le libellé n'était fait que de marqueurs :
    // mieux vaut alors le garder tel quel.
    match kept.is_empty() {
        true => tokens,
        false => kept,
    }
}

/// Retire l'écho de devise que LCL accole en fin de ligne : « EUR 16,80 ».
///
/// Le montant figure déjà dans sa propre colonne ; le répéter dans le libellé
/// n'apprend rien et masque le commerçant.
fn drop_currency_echo(tokens: Vec<String>) -> Vec<String> {
    const CURRENCIES: [&str; 3] = ["EUR", "USD", "GBP"];

    if tokens.len() < 2 {
        return tokens;
    }
    let last = &tokens[tokens.len() - 1];
    let before = tokens[tokens.len() - 2].to_uppercase();

    let looks_like_amount = last
        .chars()
        .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
        && last.chars().any(|c| c.is_ascii_digit());

    match CURRENCIES.contains(&before.as_str()) && looks_like_amount {
        true => tokens[..tokens.len() - 2].to_vec(),
        false => tokens,
    }
}

/// Réduit un libellé fait deux fois de la même phrase.
///
/// LCL produit par exemple « COTISATION MENSUELLE CARTE COTISATION MENSUELLE
/// CARTE », la seconde moitié n'apportant rien.
fn collapse_repetition(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 2 || !tokens.len().is_multiple_of(2) {
        return text.to_string();
    }
    let (first, second) = tokens.split_at(tokens.len() / 2);
    match first.eq_ignore_ascii_case_slice(second) {
        true => first.join(" "),
        false => text.to_string(),
    }
}

/// Comparaison de deux tranches de mots, insensible à la casse.
trait SliceCaseCompare {
    fn eq_ignore_ascii_case_slice(&self, other: &[&str]) -> bool;
}

impl SliceCaseCompare for [&str] {
    fn eq_ignore_ascii_case_slice(&self, other: &[&str]) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }
}

/// Reconnaît une date incrustée : `26/08/26`, `26.08.2026`.
fn is_date(token: &str) -> bool {
    let parts: Vec<&str> = token.split(['/', '.']).collect();
    (parts.len() == 2 || parts.len() == 3)
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 4 && p.chars().all(|c| c.is_ascii_digit()))
}

/// Reconnaît une référence : une longue suite de chiffres.
fn is_reference(token: &str) -> bool {
    let digits = token.trim_matches(|c: char| !c.is_ascii_digit());
    digits.len() >= REFERENCE_DIGITS && digits.chars().all(|c| c.is_ascii_digit())
}

/// Position d'un motif, sans tenir compte de la casse.
fn find_ignoring_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_uppercase().find(&needle.to_uppercase())
}

/// Position d'un mot entier, sans tenir compte de la casse.
fn find_token(text: &str, token: &str) -> Option<usize> {
    let upper = text.to_uppercase();
    let target = token.to_uppercase();
    let mut offset = 0;
    for word in upper.split_whitespace() {
        let at = upper[offset..].find(word)? + offset;
        if word == target {
            return Some(at);
        }
        offset = at + word.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cas emblématique : le seul mot utile est noyé au milieu.
    #[test]
    fn keeps_only_the_merchant_of_a_card_payment() {
        assert_eq!(
            normalize("CARTE 0602685 CB POINT P 1233 26/08/26 CBLM NOAH NYOUNAE"),
            "POINT P"
        );
    }

    #[test]
    fn strips_the_operation_prefix() {
        assert_eq!(normalize("CB U EXPRESS 02/07/26"), "U EXPRESS");
        assert_eq!(normalize("CB TCL 69 LYO 29/06/26"), "TCL 69 LYO");
        assert_eq!(normalize("VIR INST SCI Berthelot"), "SCI Berthelot");
        assert_eq!(normalize("VIR SEPA RECU SALAIRE AOUT"), "SALAIRE AOUT");
    }

    /// Le créancier réel est plus parlant que l'organisme qui collecte.
    #[test]
    fn prefers_the_actual_creditor_of_a_direct_debit() {
        let raw = "PRLV SEPA DRFIP AUVERGNE RHONE ALPES DEPARTEMENT RHONE \
                   CREANCIER INITIAL:Regie d'eau potable du grand Lyon \
                   LIBELLE:EGL EAU 2153356 MENS REF.CLIENT:1D0690000EC29";
        assert_eq!(normalize(raw), "Regie d'eau potable du grand Lyon");
    }

    /// L'erreur qu'un premier prototype avait commise : ces libellés sont
    /// entièrement porteurs de sens et ne doivent rien perdre.
    #[test]
    fn never_empties_a_meaningful_label() {
        assert_eq!(
            normalize("ASSURANCE MOYEN DE PAIEMENT"),
            "ASSURANCE MOYEN DE PAIEMENT"
        );
        assert_eq!(
            normalize("ECHEANCE PRET PERSONNEL 140824"),
            "ECHEANCE PRET PERSONNEL"
        );
    }

    /// Formule bancaire qui n'apprend rien sur l'opération.
    #[test]
    fn drops_transfer_boilerplate() {
        assert_eq!(
            normalize("VIREMENT EMETTEUR CREDIT BENEF. VIR PRET PERSONNEL 140826 226001700782"),
            "PRET PERSONNEL"
        );
    }

    /// Le retrait porte sur la phrase entière : un commerçant dont le nom
    /// contient « CREDIT » doit rester intact.
    #[test]
    fn keeps_merchants_containing_a_boilerplate_word() {
        assert_eq!(normalize("CB CREDIT AGRICOLE 12/03/25"), "CREDIT AGRICOLE");
        assert_eq!(normalize("PRLV SEPA CREDIT MUTUEL"), "CREDIT MUTUEL");
    }

    /// « FRAIS » porte l'information, « VIR INST » le bruit : le premier reste,
    /// le second part.
    #[test]
    fn keeps_the_qualifier_but_drops_the_markers_behind_it() {
        assert_eq!(
            normalize("FRAIS VIR INST HARMONIIE SAS"),
            "FRAIS HARMONIIE SAS"
        );
    }

    /// Le montant a déjà sa colonne ; le répéter masque le commerçant.
    #[test]
    fn drops_the_currency_echo() {
        assert_eq!(
            normalize("CB CHAMAS TACOS LYO LYON EUR 18,50"),
            "CHAMAS TACOS LYO LYON"
        );
        assert_eq!(normalize("CB TCL 69 LYO EUR 2,10"), "TCL 69 LYO");
    }

    /// Une devise qui fait partie du nom ne doit pas déclencher le retrait.
    #[test]
    fn keeps_a_currency_word_that_is_not_an_echo() {
        assert_eq!(normalize("CB CRUNCHYROLL *EUR"), "CRUNCHYROLL *EUR");
    }

    /// LCL a changé ses formats au fil des années. Sans unifier ces variantes,
    /// une même dépense apparaît comme deux abonnements distincts, l'un
    /// « arrêté » et l'autre « actif ».
    #[test]
    fn unifies_labels_that_changed_format() {
        assert_eq!(
            normalize("VIREMENT INSTANTANE VIR INST Solygest"),
            normalize("VIR INST Solygest")
        );
        assert_eq!(
            normalize("DEBIT DIVERS ASSURANCE MOYEN DE PAIEMENT"),
            normalize("ASSURANCE MOYEN DE PAIEMENT")
        );
    }

    #[test]
    fn collapses_a_doubled_label() {
        assert_eq!(
            normalize("COTISATION MENSUELLE CARTE COTISATION MENSUELLE CARTE 0268"),
            "COTISATION MENSUELLE CARTE"
        );
    }

    /// Un libellé fait uniquement de marqueurs perdrait tout : on le préserve.
    #[test]
    fn keeps_a_label_made_only_of_markers() {
        assert_eq!(normalize("CB CARTE"), "CB CARTE");
    }

    /// Les banques varient leur typographie d'un mois à l'autre : sans repli,
    /// « SCI Berthelot » et « SCI BERTHELOT » deviennent deux dépenses.
    #[test]
    fn folds_case_and_accents_for_grouping() {
        assert_eq!(fold("SCI Berthelot"), fold("SCI BERTHELOT"));
        assert_eq!(fold("Burger King"), fold("BURGER KING"));
        assert_eq!(fold("Régie d'eau"), fold("REGIE D'EAU"));
    }

    /// Le repli ne doit pas confondre des bénéficiaires distincts.
    #[test]
    fn folding_keeps_different_merchants_apart() {
        assert_ne!(fold("UBER *EATS"), fold("UBER *ONE"));
        assert_ne!(fold("NETFLIX"), fold("SPOTIFY"));
    }

    #[test]
    fn never_returns_an_empty_label() {
        for raw in ["", "   ", "0602685", "26/08/26", "CBLM NOAH NYOUNAE"] {
            assert!(
                !normalize(raw).is_empty(),
                "libellé vidé pour l'entrée {raw:?}"
            );
        }
    }

    #[test]
    fn keeps_short_numbers_that_carry_meaning() {
        // Le département et le numéro de restaurant situent l'opération.
        assert_eq!(normalize("CB TCL 69 LYO"), "TCL 69 LYO");
        assert_eq!(normalize("CB MC DONALDS 21 08/05/24"), "MC DONALDS 21");
    }

    #[test]
    fn recognises_embedded_dates() {
        assert!(is_date("26/08/26"));
        assert!(is_date("26.08.2026"));
        assert!(is_date("14/08"));
        assert!(!is_date("POINT"));
        assert!(!is_date("1D0690000EC29"));
    }

    #[test]
    fn recognises_long_references() {
        assert!(is_reference("0602685"));
        assert!(is_reference("1233"));
        assert!(!is_reference("69"));
        assert!(!is_reference("21"));
        assert!(!is_reference("UBER"));
    }

    /// Le nettoyage doit être stable : deux passages donnent le même résultat.
    #[test]
    fn is_idempotent() {
        for raw in [
            "CARTE 0602685 CB POINT P 1233 26/08/26 CBLM NOAH NYOUNAE",
            "CB U EXPRESS 02/07/26",
            "ASSURANCE MOYEN DE PAIEMENT",
        ] {
            let once = normalize(raw);
            assert_eq!(normalize(&once), once, "instable pour {raw:?}");
        }
    }
}
