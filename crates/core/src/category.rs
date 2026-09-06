//! Classement des opérations par secteur de dépense.
//!
//! Le classement est **déterministe et local** : une table de motifs, lue dans
//! l'ordre, appliquée au libellé nettoyé. Aucune connaissance du monde, aucun
//! appel extérieur — donc un résultat qu'on peut lire, prévoir et corriger.
//!
//! Les motifs sont calibrés sur des libellés réels plutôt que devinés. Un
//! commerçant inconnu tombe dans [`Category::Uncategorised`] : mieux vaut une
//! opération non classée qu'une opération mal classée, qui fausserait
//! silencieusement les totaux d'un secteur.
//!
//! # Les virements internes
//!
//! Déplacer de l'argent vers son propre livret ou son courtier n'est pas une
//! dépense. Confondre les deux gonflerait les sorties de plusieurs milliers
//! d'euros sans que rien n'ait été consommé, d'où [`Category::Transfer`] et
//! [`Category::Investment`], tenus à l'écart des dépenses.

use serde::{Deserialize, Serialize};

/// Secteur d'une opération.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Housing,
    Groceries,
    Dining,
    Transport,
    Health,
    Shopping,
    Subscriptions,
    Leisure,
    Gambling,
    Banking,
    Credit,
    Cash,
    /// Mouvement vers un autre de ses propres comptes.
    Transfer,
    /// Versement vers un support d'épargne ou un courtier.
    Investment,
    Income,
    Uncategorised,
}

impl Category {
    /// Libellé français, pour l'affichage.
    pub fn label(self) -> &'static str {
        match self {
            Category::Housing => "Logement",
            Category::Groceries => "Alimentation",
            Category::Dining => "Restauration",
            Category::Transport => "Transport",
            Category::Health => "Santé",
            Category::Shopping => "Achats",
            Category::Subscriptions => "Abonnements",
            Category::Leisure => "Loisirs",
            Category::Gambling => "Jeux d'argent",
            Category::Banking => "Banque et assurances",
            Category::Credit => "Crédit",
            Category::Cash => "Retraits",
            Category::Transfer => "Virements internes",
            Category::Investment => "Épargne et investissement",
            Category::Income => "Revenus",
            Category::Uncategorised => "Non classé",
        }
    }

    /// Vrai si la catégorie constitue une dépense réelle.
    ///
    /// Un virement vers son propre livret sort du compte sans rien consommer :
    /// l'additionner aux dépenses ferait croire à une consommation qui n'a pas
    /// eu lieu.
    pub fn is_spending(self) -> bool {
        !matches!(
            self,
            Category::Transfer | Category::Investment | Category::Income
        )
    }

    /// Toutes les catégories, pour l'affichage d'une répartition.
    pub fn all() -> [Category; 16] {
        [
            Category::Housing,
            Category::Groceries,
            Category::Dining,
            Category::Transport,
            Category::Health,
            Category::Shopping,
            Category::Subscriptions,
            Category::Leisure,
            Category::Gambling,
            Category::Banking,
            Category::Credit,
            Category::Cash,
            Category::Transfer,
            Category::Investment,
            Category::Income,
            Category::Uncategorised,
        ]
    }
}

/// Motifs reconnus, dans l'ordre d'examen.
///
/// L'ordre est significatif : le premier motif trouvé l'emporte. Les enseignes
/// nommément reconnues viennent donc avant les mots génériques, sans quoi
/// « UBER *EATS » serait rangé dans les transports par le mot « UBER ».
const RULES: &[(&str, Category)] = &[
    // --- Livraison de repas, avant les transports ------------------------
    ("UBER *EATS", Category::Dining),
    ("UBER EATS", Category::Dining),
    ("DELIVEROO", Category::Dining),
    ("JUST EAT", Category::Dining),
    ("STUART", Category::Dining),
    // --- Abonnements numériques, avant les achats en ligne ---------------
    ("NETFLIX", Category::Subscriptions),
    ("SPOTIFY", Category::Subscriptions),
    ("DEEZER", Category::Subscriptions),
    ("CRUNCHYROLL", Category::Subscriptions),
    ("MICROSOF", Category::Subscriptions),
    ("OPENAI", Category::Subscriptions),
    ("CHATGPT", Category::Subscriptions),
    ("AMAZON PRIME", Category::Subscriptions),
    ("UBER *ONE", Category::Subscriptions),
    ("UBER ONE", Category::Subscriptions),
    ("PADDLE", Category::Subscriptions),
    ("GOOGLE", Category::Subscriptions),
    ("APPLE.COM", Category::Subscriptions),
    // --- Logement ---------------------------------------------------------
    ("SCI ", Category::Housing),
    ("SOLYGEST", Category::Housing),
    ("NOTICE IMMO", Category::Housing),
    ("LOCATION IMMO", Category::Housing),
    ("LOYER", Category::Housing),
    ("FONCIA", Category::Housing),
    ("NEXITY", Category::Housing),
    ("EAU DU GRAND LYON", Category::Housing),
    ("REGIE D'EAU", Category::Housing),
    ("VATTENFALL", Category::Housing),
    ("EDF", Category::Housing),
    ("ENGIE", Category::Housing),
    ("TOTALENERGIES", Category::Housing),
    ("FREE TELECOM", Category::Housing),
    ("ORANGE", Category::Housing),
    ("SFR", Category::Housing),
    ("BOUYGUES TEL", Category::Housing),
    // --- Alimentation -----------------------------------------------------
    ("INTERMARCHE", Category::Groceries),
    ("CARREFOUR", Category::Groceries),
    ("LECLERC", Category::Groceries),
    ("AUCHAN", Category::Groceries),
    ("LIDL", Category::Groceries),
    ("ALDI", Category::Groceries),
    ("MONOPRIX", Category::Groceries),
    ("FRANPRIX", Category::Groceries),
    ("CASINO", Category::Groceries),
    ("U EXPRESS", Category::Groceries),
    ("SUPER U", Category::Groceries),
    ("PICARD", Category::Groceries),
    ("BANETTE", Category::Groceries),
    ("BOULANGERIE", Category::Groceries),
    ("EPICERIE", Category::Groceries),
    ("PRIMEUR", Category::Groceries),
    // --- Restauration -----------------------------------------------------
    ("MC DONALD", Category::Dining),
    ("MAC DONALD", Category::Dining),
    ("MCDONALD", Category::Dining),
    ("MC DO", Category::Dining),
    ("BURGER KING", Category::Dining),
    ("KFC", Category::Dining),
    ("SUBWAY", Category::Dining),
    ("KEBAB", Category::Dining),
    ("TACOS", Category::Dining),
    ("PIZZ", Category::Dining),
    ("SUSHI", Category::Dining),
    ("BAGELSTEIN", Category::Dining),
    ("STARBUCKS", Category::Dining),
    ("BRASSERIE", Category::Dining),
    ("RESTAURANT", Category::Dining),
    ("BAR ", Category::Dining),
    ("CAFE", Category::Dining),
    ("BOUCHON", Category::Dining),
    // --- Transport --------------------------------------------------------
    ("SNCF", Category::Transport),
    ("TCL ", Category::Transport),
    ("RATP", Category::Transport),
    ("BLABLACAR", Category::Transport),
    ("OUIGO", Category::Transport),
    ("TRAINLINE", Category::Transport),
    ("ESCOTA", Category::Transport),
    ("VINCI AUTOROUTE", Category::Transport),
    ("APRR", Category::Transport),
    ("SEMEPA", Category::Transport),
    ("PARKING", Category::Transport),
    ("ESSO", Category::Transport),
    ("TOTAL ACCESS", Category::Transport),
    ("STATION", Category::Transport),
    ("UBER", Category::Transport),
    ("BOLT", Category::Transport),
    ("TAXI", Category::Transport),
    // --- Santé ------------------------------------------------------------
    ("PHARMACIE", Category::Health),
    ("DOCTEUR", Category::Health),
    ("SELARL", Category::Health),
    ("CABINET MEDICAL", Category::Health),
    ("LABORATOIRE", Category::Health),
    ("MUTUELLE", Category::Health),
    ("HARMONIE MUT", Category::Health),
    ("CPAM", Category::Health),
    ("DENTAIRE", Category::Health),
    ("OPTIC", Category::Health),
    // --- Achats -----------------------------------------------------------
    ("AMAZON", Category::Shopping),
    ("FNAC", Category::Shopping),
    ("DARTY", Category::Shopping),
    ("BOULANGER", Category::Shopping),
    ("CDISCOUNT", Category::Shopping),
    ("LEROY MERLIN", Category::Shopping),
    ("POINT P", Category::Shopping),
    ("CASTORAMA", Category::Shopping),
    ("DECATHLON", Category::Shopping),
    ("ZARA", Category::Shopping),
    ("H&M", Category::Shopping),
    ("VINTED", Category::Shopping),
    ("ALIEXPRESS", Category::Shopping),
    ("SHEIN", Category::Shopping),
    // --- Loisirs ----------------------------------------------------------
    ("CINEMA", Category::Leisure),
    ("UGC", Category::Leisure),
    ("PATHE", Category::Leisure),
    ("STEAM", Category::Leisure),
    ("NINTENDO", Category::Leisure),
    ("PLAYSTATION", Category::Leisure),
    ("FITNESS", Category::Leisure),
    ("BASIC FIT", Category::Leisure),
    ("SALLE DE SPORT", Category::Leisure),
    ("AIRSOFT", Category::Leisure),
    ("POPCORN", Category::Leisure),
    // --- Jeux d'argent ----------------------------------------------------
    ("BETCLIC", Category::Gambling),
    ("WINAMAX", Category::Gambling),
    ("UNIBET", Category::Gambling),
    ("PMU", Category::Gambling),
    ("FDJ", Category::Gambling),
    ("POKERSTARS", Category::Gambling),
    // --- Épargne et investissement, avant les virements -------------------
    ("TRADE REPUBLIC", Category::Investment),
    ("BOURSORAMA", Category::Investment),
    ("DEGIRO", Category::Investment),
    ("YOMONI", Category::Investment),
    ("LINXEA", Category::Investment),
    ("LIVRET", Category::Investment),
    ("PEA", Category::Investment),
    ("ASSURANCE VIE", Category::Investment),
    // --- Banque et assurances ---------------------------------------------
    ("ASSURANCE MOYEN DE PAIEMENT", Category::Banking),
    ("ASSURANCE LCL", Category::Banking),
    ("COTISATION", Category::Banking),
    ("FRAIS ", Category::Banking),
    ("COMMISSION", Category::Banking),
    ("AGIOS", Category::Banking),
    ("AXA", Category::Banking),
    ("MAIF", Category::Banking),
    ("MACIF", Category::Banking),
    ("MAAF", Category::Banking),
    ("ASSURANCE", Category::Banking),
    // --- Crédit -----------------------------------------------------------
    ("ECHEANCE PRET", Category::Credit),
    ("PRET PERSONNEL", Category::Credit),
    ("REMBOURSEMENT PRET", Category::Credit),
    // --- Retraits ---------------------------------------------------------
    ("RETRAIT", Category::Cash),
    ("DISTRIBUTEUR", Category::Cash),
    // --- Revenus ----------------------------------------------------------
    ("SALAIRE", Category::Income),
    ("CAF ", Category::Income),
    ("CROUS", Category::Dining),
    ("POLE EMPLOI", Category::Income),
    ("FRANCE TRAVAIL", Category::Income),
    ("URSSAF", Category::Income),
    ("REMBOURSEMENT", Category::Income),
];

/// Classe une opération d'après son libellé.
///
/// `label` est le libellé déjà nettoyé ; `is_credit` distingue une entrée d'une
/// sortie. Le sens compte : « CARREFOUR » au crédit est un remboursement, pas
/// une course.
pub fn categorize(label: &str, is_credit: bool) -> Category {
    categorize_with(label, is_credit, &[])
}

/// Comme [`categorize`], en reconnaissant en plus ses propres comptes.
///
/// `own_names` énumère les libellés désignant l'utilisateur ou ses autres
/// comptes — le nom du titulaire, celui d'un livret. Ces mouvements sortent du
/// compte sans rien consommer, et les classer comme dépenses gonflerait les
/// sorties de plusieurs milliers d'euros.
///
/// Cette liste ne peut pas être devinée : « Noah Nyounae » est un virement
/// interne, « Cyril Maurange » un remboursement entre amis, et rien dans le
/// libellé ne les distingue. C'est à l'utilisateur de le dire.
pub fn categorize_with(label: &str, is_credit: bool, own_names: &[String]) -> Category {
    categorize_builtin(label, is_credit, own_names)
}

/// Applique les redirections en vigueur à une catégorie.
///
/// Retirer une catégorie d'origine ne réécrit pas les motifs qui la
/// désignent : ils continuent de classer, et c'est ici que le résultat est
/// renvoyé vers la catégorie d'accueil. La chaîne est suivie sur quelques
/// sauts — A vers B, B vers C — et bornée pour qu'un cycle, qu'une base
/// abîmée pourrait contenir, ne fasse pas tourner le serveur indéfiniment.
pub fn resolve(key: &str, redirects: &std::collections::HashMap<String, String>) -> String {
    let mut current = key.to_string();
    for _ in 0..8 {
        match redirects.get(&current) {
            Some(next) if *next != current => current = next.clone(),
            _ => return current,
        }
    }
    current
}

/// Classement complet : règles de l'utilisateur, motifs internes, redirections.
///
/// Le résultat est une **clé**, pas une variante du type figé. Les catégories
/// créées par l'utilisateur n'ont pas de variante : les représenter par une
/// clé est la seule façon qu'une règle vers l'une d'elles produise un effet
/// plutôt que d'être silencieusement perdue en chemin.
///
/// `overrides` associe un libellé replié à la clé choisie. Ces règles priment
/// sur tout le reste : une correction manuelle doit tenir, même contre un
/// motif interne qui prétendrait le contraire.
pub fn classify(
    label: &str,
    is_credit: bool,
    own_names: &[String],
    overrides: &std::collections::HashMap<String, String>,
    redirects: &std::collections::HashMap<String, String>,
) -> String {
    let folded = crate::merchant::fold(label);

    if let Some(chosen) = overrides.get(&folded) {
        // La règle est suivie jusqu'à sa catégorie d'accueil : viser une
        // catégorie depuis retirée doit classer, pas disparaître.
        return resolve(chosen, redirects);
    }

    resolve(
        crate::preferences::category_key(categorize_builtin(label, is_credit, own_names)),
        redirects,
    )
}

/// Classement par les seuls motifs internes.
fn categorize_builtin(label: &str, is_credit: bool, own_names: &[String]) -> Category {
    let folded = crate::merchant::fold(label);

    // Les comptes propres priment sur toute règle : un virement vers son
    // livret ne doit pas être classé par un motif qui traînerait dans le nom.
    for name in own_names {
        let name = crate::merchant::fold(name);
        if !name.is_empty() && folded.contains(&name) {
            return Category::Transfer;
        }
    }

    for (pattern, category) in RULES {
        if folded.contains(pattern) {
            // Un motif de dépense trouvé sur un crédit désigne un
            // remboursement : le ranger dans son secteur ferait apparaître une
            // dépense négative.
            if is_credit && category.is_spending() {
                return Category::Income;
            }
            return *category;
        }
    }

    // Un crédit sans motif reconnu reste un revenu ; une sortie non reconnue
    // n'est pas devinée.
    match is_credit {
        true => Category::Income,
        false => Category::Uncategorised,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_the_common_sectors() {
        assert_eq!(categorize("INTERMARCHE", false), Category::Groceries);
        assert_eq!(categorize("SCI Berthelot", false), Category::Housing);
        assert_eq!(categorize("MC DONALDS 21", false), Category::Dining);
        assert_eq!(categorize("SNCF-VOYAGEURS", false), Category::Transport);
        assert_eq!(categorize("BETCLIC", false), Category::Gambling);
    }

    /// L'ordre des motifs compte : « UBER » seul rangerait la livraison de
    /// repas dans les transports.
    #[test]
    fn prefers_the_more_specific_pattern() {
        assert_eq!(categorize("UBER *EATS", false), Category::Dining);
        assert_eq!(categorize("UBER *ONE MEMB", false), Category::Subscriptions);
        // La course reste un transport.
        assert_eq!(categorize("UBER *TRIP", false), Category::Transport);
    }

    /// Même exigence entre l'abonnement et l'achat.
    #[test]
    fn separates_a_subscription_from_a_purchase() {
        assert_eq!(
            categorize("AMAZON PRIME FR", false),
            Category::Subscriptions
        );
        assert_eq!(categorize("AMAZON PAYMENTS", false), Category::Shopping);
    }

    #[test]
    fn ignores_case_and_accents() {
        assert_eq!(categorize("Regie d'eau potable", false), Category::Housing);
        assert_eq!(categorize("intermarche", false), Category::Groceries);
    }

    /// Déplacer de l'argent vers son courtier n'est pas une dépense.
    #[test]
    fn keeps_savings_out_of_spending() {
        let investment = categorize("TRADE REPUBLIC", false);
        assert_eq!(investment, Category::Investment);
        assert!(!investment.is_spending());
        assert!(!Category::Transfer.is_spending());
    }

    /// Un crédit portant un motif de dépense est un remboursement.
    #[test]
    fn treats_a_credit_on_a_spending_pattern_as_income() {
        assert_eq!(categorize("INTERMARCHE", true), Category::Income);
    }

    /// Un crédit sans motif reconnu reste un revenu ; une sortie inconnue
    /// n'est pas devinée, elle est signalée comme telle.
    #[test]
    fn leaves_an_unknown_debit_unclassified() {
        assert_eq!(
            categorize("BOUTIQUE INCONNUE XYZ", false),
            Category::Uncategorised
        );
        assert_eq!(categorize("BOUTIQUE INCONNUE XYZ", true), Category::Income);
    }

    /// Ce que les motifs ne peuvent pas deviner : « Noah Nyounae » est un
    /// virement interne, « Cyril Maurange » un remboursement entre amis.
    #[test]
    fn recognises_declared_own_accounts() {
        let miens = vec!["Noah Nyounae".to_string()];
        assert_eq!(
            categorize_with("Noah Nyounae CI", false, &miens),
            Category::Transfer
        );
        assert_eq!(
            categorize_with("Cyril Maurange", false, &miens),
            Category::Uncategorised
        );
    }

    /// Un compte propre l'emporte sur un motif qui traînerait dans le nom.
    #[test]
    fn own_accounts_win_over_patterns() {
        let miens = vec!["Livret Bar".to_string()];
        assert_eq!(
            categorize_with("Livret Bar", false, &miens),
            Category::Transfer,
            "« BAR » ne doit pas en faire une sortie au restaurant"
        );
    }

    #[test]
    fn ignores_a_blank_own_name() {
        let miens = vec!["".to_string(), "   ".to_string()];
        assert_eq!(
            categorize_with("INTERMARCHE", false, &miens),
            Category::Groceries
        );
    }

    fn regle(libelle: &str, cle: &str) -> std::collections::HashMap<String, String> {
        let mut regles = std::collections::HashMap::new();
        regles.insert(crate::merchant::fold(libelle), cle.to_string());
        regles
    }

    fn sans() -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    /// Une correction manuelle doit primer sur tout motif interne.
    #[test]
    fn a_manual_rule_wins_over_the_built_in_patterns() {
        let regles = regle("INTERMARCHE", "leisure");
        assert_eq!(
            classify("INTERMARCHE", false, &[], &regles, &sans()),
            "leisure"
        );
    }

    /// Et même sur la déclaration de ses propres comptes.
    #[test]
    fn a_manual_rule_wins_over_own_accounts() {
        let regles = regle("Noah Nyounae", "investment");
        assert_eq!(
            classify(
                "Noah Nyounae",
                false,
                &["Noah Nyounae".into()],
                &regles,
                &sans()
            ),
            "investment"
        );
    }

    /// Le défaut qui a fait échouer l'association : une catégorie créée par
    /// l'utilisateur n'a pas de variante interne, et doit malgré tout classer.
    #[test]
    fn a_rule_can_point_at_a_user_created_category() {
        let regles = regle("INTERMARCHE", "u_cadeaux");
        assert_eq!(
            classify("INTERMARCHE", false, &[], &regles, &sans()),
            "u_cadeaux"
        );
    }

    /// Une règle visant une catégorie depuis retirée suit sa redirection.
    #[test]
    fn a_rule_follows_the_redirect_of_a_removed_category() {
        let regles = regle("INTERMARCHE", "u_cadeaux");
        let mut vers = std::collections::HashMap::new();
        vers.insert("u_cadeaux".to_string(), "shopping".to_string());
        assert_eq!(
            classify("INTERMARCHE", false, &[], &regles, &vers),
            "shopping"
        );
    }

    /// Sans règle, le classement interne s'applique et se replie sur sa clé.
    #[test]
    fn without_a_rule_the_built_in_patterns_still_apply() {
        assert_eq!(
            classify("INTERMARCHE", false, &[], &sans(), &sans()),
            "groceries"
        );
    }

    /// Retirer une catégorie redirige ce qu'elle classait, sans réécrire les
    /// motifs qui la désignent.
    #[test]
    fn follows_a_redirect() {
        let mut vers = std::collections::HashMap::new();
        vers.insert("dining".to_string(), "groceries".to_string());
        assert_eq!(resolve("dining", &vers), "groceries");
        assert_eq!(resolve("transport", &vers), "transport");
    }

    /// Deux retraits successifs se suivent jusqu'au bout.
    #[test]
    fn follows_a_chain_of_redirects() {
        let mut vers = std::collections::HashMap::new();
        vers.insert("dining".to_string(), "groceries".to_string());
        vers.insert("groceries".to_string(), "shopping".to_string());
        assert_eq!(resolve("dining", &vers), "shopping");
    }

    /// Une base abîmée ne doit pas faire tourner le serveur indéfiniment.
    #[test]
    fn a_cycle_terminates() {
        let mut vers = std::collections::HashMap::new();
        vers.insert("a".to_string(), "b".to_string());
        vers.insert("b".to_string(), "a".to_string());
        // La valeur importe peu ; ne pas boucler, si.
        let _ = resolve("a", &vers);
    }

    #[test]
    fn every_category_has_a_label() {
        for category in Category::all() {
            assert!(!category.label().is_empty());
        }
    }

    /// Trois catégories seulement échappent aux dépenses.
    #[test]
    fn only_movements_escape_spending() {
        let hors = Category::all().iter().filter(|c| !c.is_spending()).count();
        assert_eq!(hors, 3);
    }
}
