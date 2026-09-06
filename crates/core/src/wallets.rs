//! Portefeuilles virtuels : répartir le solde en enveloppes mensuelles.
//!
//! Un portefeuille n'est pas un compte : aucun argent n'y est déplacé. C'est
//! une **enveloppe** — une somme qu'on décide de consacrer chaque mois à un
//! usage, et que les dépenses des catégories rattachées viennent entamer.
//!
//! Les enveloppes s'emboîtent : une enveloppe mère **dote** ses filles. Sa
//! dotation est le budget de toute la branche, et ce qu'il en reste après
//! avoir servi ses filles finance ses propres catégories. C'est la métaphore
//! prise au mot — l'argent de la petite enveloppe est déjà dans la grande, et
//! ne saurait donc être compté deux fois.
//!
//! Le report est la règle qui donne son sens à l'ensemble : ce qui n'a pas été
//! dépensé ne s'évapore pas au changement de mois. Une enveloppe de 300 € dont
//! 290 ont été consommés rouvre le mois suivant à 310 €. Le reliquat peut aussi
//! être dirigé vers une autre enveloppe, ce qui permet de faire refluer
//! l'économie d'un poste vers un autre sans y penser chaque mois.
//!
//! Ce module ne connaît ni base ni réseau : il prend l'état réglé par
//! l'utilisateur et les mouvements observés, et déroule les mois. Toute la
//! logique de report tient donc dans des fonctions pures, vérifiables sans
//! monter une base.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::Serialize;

/// Nombre maximal de mois déroulés d'une traite.
///
/// Une date de départ aberrante — un portefeuille daté de 1900 par une base
/// abîmée — ferait sinon tourner la projection sans fin à chaque affichage.
const MAX_MONTHS: usize = 600;

/// Profondeur maximale d'emboîtement parcourue.
const MAX_DEPTH: usize = 8;

/// Un mois calendaire, sans jour.
///
/// Le report se raisonne par mois entiers : un type dédié évite de trimballer
/// une date dont le jour n'aurait aucun sens, et rend l'enchaînement des mois
/// explicite plutôt que calculé au petit bonheur à chaque usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(into = "String")]
pub struct Month {
    pub year: i32,
    /// De 1 à 12.
    pub month: u32,
}

impl Month {
    /// Construit un mois, en ramenant un rang hors bornes dans l'intervalle.
    pub fn new(year: i32, month: u32) -> Self {
        Self {
            year,
            month: month.clamp(1, 12),
        }
    }

    /// Le mois auquel appartient une date.
    pub fn of(date: NaiveDate) -> Self {
        Self::new(date.year(), date.month())
    }

    /// Le mois suivant.
    pub fn next(self) -> Self {
        match self.month {
            12 => Self::new(self.year + 1, 1),
            m => Self::new(self.year, m + 1),
        }
    }

    /// Lit un mois écrit `AAAA-MM`.
    pub fn parse(text: &str) -> Option<Self> {
        let (year, month) = text.trim().split_once('-')?;
        let year = year.parse().ok()?;
        let month = month.parse().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        Some(Self { year, month })
    }
}

impl std::fmt::Display for Month {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl From<Month> for String {
    fn from(month: Month) -> Self {
        month.to_string()
    }
}

/// Un portefeuille tel que l'utilisateur l'a réglé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallet {
    pub id: i64,
    pub name: String,
    /// Somme dont l'enveloppe est dotée au début de chaque mois.
    ///
    /// L'utilisateur la fixe lui-même : les entrées d'argent ne se répartissent
    /// pas toutes seules, parce que rien dans un virement ne dit à quoi il est
    /// destiné.
    pub allocation: Decimal,
    /// Catégories dont les mouvements sont imputés à cette enveloppe.
    ///
    /// Déjà étendues à leurs sous-catégories par l'appelant : une enveloppe
    /// rattachée à « Transport » doit compter « Essence » et « Péage ».
    pub categories: HashSet<String>,
    /// Enveloppe qui reçoit le reliquat en fin de mois.
    ///
    /// `None` : l'enveloppe garde son propre reliquat, ce qui est le cas
    /// courant. Renseigné, le reliquat s'en va, et l'enveloppe rouvre à sa
    /// seule dotation.
    ///
    /// L'emboîtement ne s'en mêle pas : une fille garde son reliquat, sauf à
    /// désigner explicitement sa mère.
    pub carry_to: Option<i64>,
    /// Enveloppe qui contient celle-ci, et sur la dotation de laquelle elle
    /// est prise.
    pub parent: Option<i64>,
    /// Premier mois où l'enveloppe est dotée.
    pub start: Month,
}

/// Un apport ponctuel versé à une enveloppe.
///
/// Distinct de la dotation, qui est une consigne permanente : celui-ci ne vaut
/// que pour le mois où il est versé. Une prime qu'on met de côté, un
/// réajustement en cours de route.
///
/// Il ne se prend **pas** sur la dotation de la mère : c'est de l'argent qui
/// vient de la part non allouée du compte, et non du budget de la branche. Une
/// mère n'est donc pas appauvrie parce qu'on a renfloué une de ses filles.
///
/// Le montant est signé : négatif, l'apport devient un retrait.
#[derive(Debug, Clone)]
pub struct Contribution {
    pub wallet_id: i64,
    pub month: Month,
    pub amount: Decimal,
}

/// Un mouvement observé, déjà rattaché à sa catégorie.
///
/// Le montant est signé comme en banque : négatif pour une dépense. Un
/// remboursement, positif, regarnit donc l'enveloppe de lui-même.
#[derive(Debug, Clone)]
pub struct Movement {
    pub month: Month,
    pub category: String,
    pub amount: Decimal,
}

/// L'état d'une enveloppe sur un mois.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonthlyState {
    pub wallet_id: i64,
    pub month: Month,
    /// Reliquat reçu à l'ouverture : le sien, ou celui d'une autre enveloppe.
    pub carried_in: Decimal,
    /// Dotation qui finance réellement l'enveloppe ce mois-ci.
    ///
    /// Pour une mère, c'est ce qui lui reste après avoir doté ses filles :
    /// négatif si elles réclament plus qu'elle n'a.
    pub allocation: Decimal,
    /// Apports ponctuels versés ce mois-ci, hors dotation.
    pub contributions: Decimal,
    /// Somme signée des mouvements imputés. Négative quand on a dépensé.
    pub movements: Decimal,
    /// Ce qui reste : `carried_in + allocation + contributions + movements`.
    ///
    /// Peut être négatif : une enveloppe dépassée reporte sa dette plutôt que
    /// de la faire disparaître au changement de mois.
    pub balance: Decimal,
}

impl MonthlyState {
    /// Ce dont l'enveloppe disposait avant toute dépense.
    pub fn available(&self) -> Decimal {
        self.carried_in + self.allocation + self.contributions
    }
}

/// Dotation qui finance réellement chaque enveloppe.
///
/// Une mère dote ses filles : sa dotation est le budget de toute la branche, et
/// ce qui lui reste après les avoir servies finance ses propres catégories.
/// Sans cette soustraction, l'argent des filles serait compté deux fois — une
/// fois dans la mère, une fois dans chacune d'elles — et la somme des
/// enveloppes cesserait de valoir le solde du compte.
///
/// Le résultat peut être négatif : les filles réclament alors plus que la mère
/// ne dispose. C'est un signal à montrer, pas une erreur à corriger en
/// silence — corriger reviendrait à décider à la place de l'utilisateur
/// laquelle de ses enveloppes rogner.
pub fn effective_allocations(wallets: &[Wallet]) -> HashMap<i64, Decimal> {
    let mut handed: HashMap<i64, Decimal> = HashMap::new();
    for wallet in wallets {
        // Se contenir soi-même n'a pas de sens et se retirerait sa propre
        // dotation : le stockage le refuse, le calcul l'ignore.
        if let Some(parent) = wallet.parent
            && parent != wallet.id
        {
            *handed.entry(parent).or_insert(Decimal::ZERO) += wallet.allocation;
        }
    }

    wallets
        .iter()
        .map(|w| {
            let given = handed.get(&w.id).copied().unwrap_or(Decimal::ZERO);
            (w.id, w.allocation - given)
        })
        .collect()
}

/// Somme confiée aux filles directes d'une enveloppe.
pub fn handed_to_children(wallets: &[Wallet], id: i64) -> Decimal {
    wallets
        .iter()
        .filter(|w| w.parent == Some(id) && w.id != id)
        .map(|w| w.allocation)
        .sum()
}

/// Une enveloppe et tout ce qu'elle contient, à toute profondeur.
///
/// Le parcours est borné comme les autres : une base abîmée ne doit pas faire
/// tourner le serveur à chaque affichage.
pub fn branch(wallets: &[Wallet], id: i64) -> HashSet<i64> {
    let mut branch = HashSet::from([id]);
    for _ in 0..MAX_DEPTH {
        let before = branch.len();
        for wallet in wallets {
            if let Some(parent) = wallet.parent
                && branch.contains(&parent)
            {
                branch.insert(wallet.id);
            }
        }
        if branch.len() == before {
            break;
        }
    }
    branch
}

/// Déroule les mois et rend l'état de chaque enveloppe sur chacun.
///
/// Le calcul part du premier mois doté et avance jusqu'à `through`. Chaque mois
/// s'obtient du précédent : la dotation s'ajoute au reliquat reçu, les
/// mouvements des catégories rattachées viennent l'entamer, et ce qu'il reste
/// part au mois suivant — dans la même enveloppe, ou dans celle que
/// l'utilisateur a désignée.
///
/// Rien n'est figé en base : l'historique se recalcule à chaque affichage. Une
/// opération qui arrive en retard, ou une catégorie corrigée après coup, se
/// répercute donc sur tous les mois suivants au lieu de laisser un solde faux
/// derrière elle. C'est aussi pourquoi changer une dotation réécrit le passé :
/// la dotation est une consigne permanente, pas un versement historique.
pub fn project(
    wallets: &[Wallet],
    movements: &[Movement],
    contributions: &[Contribution],
    through: Month,
) -> Vec<MonthlyState> {
    let Some(first) = wallets.iter().map(|w| w.start).min() else {
        return Vec::new();
    };
    if first > through {
        return Vec::new();
    }

    // Les mouvements sont regroupés une fois : les reparcourir pour chaque
    // enveloppe et chaque mois coûterait le produit des trois.
    let mut by_month: HashMap<Month, Vec<&Movement>> = HashMap::new();
    for movement in movements {
        by_month.entry(movement.month).or_default().push(movement);
    }

    // Regroupés une fois, comme les mouvements : les reparcourir pour chaque
    // enveloppe et chaque mois coûterait le produit des trois.
    let mut given: HashMap<(i64, Month), Decimal> = HashMap::new();
    for contribution in contributions {
        *given
            .entry((contribution.wallet_id, contribution.month))
            .or_insert(Decimal::ZERO) += contribution.amount;
    }

    let known: HashSet<i64> = wallets.iter().map(|w| w.id).collect();
    let effective = effective_allocations(wallets);
    let mut carried: HashMap<i64, Decimal> = HashMap::new();
    let mut states = Vec::new();

    let mut month = first;
    for _ in 0..MAX_MONTHS {
        let empty = Vec::new();
        let this_month = by_month.get(&month).unwrap_or(&empty);
        let mut next_carried: HashMap<i64, Decimal> = HashMap::new();

        for wallet in wallets {
            if wallet.start > month {
                continue;
            }

            let carried_in = carried.get(&wallet.id).copied().unwrap_or(Decimal::ZERO);
            let moved: Decimal = this_month
                .iter()
                .filter(|m| wallet.categories.contains(&m.category))
                .map(|m| m.amount)
                .sum();
            let allocation = effective
                .get(&wallet.id)
                .copied()
                .unwrap_or(wallet.allocation);
            let contributed = given
                .get(&(wallet.id, month))
                .copied()
                .unwrap_or(Decimal::ZERO);
            let balance = carried_in + allocation + contributed + moved;

            states.push(MonthlyState {
                wallet_id: wallet.id,
                month,
                carried_in,
                allocation,
                contributions: contributed,
                movements: moved,
                balance,
            });

            // Une destination inconnue — enveloppe supprimée entre-temps — ne
            // doit pas faire disparaître le reliquat : il reste sur place.
            let target = match wallet.carry_to {
                Some(id) if known.contains(&id) && id != wallet.id => id,
                _ => wallet.id,
            };
            *next_carried.entry(target).or_insert(Decimal::ZERO) += balance;
        }

        if month == through {
            break;
        }
        carried = next_carried;
        month = month.next();
    }

    states
}

/// L'état de chaque enveloppe sur un mois donné.
pub fn states_for(states: &[MonthlyState], month: Month) -> Vec<&MonthlyState> {
    states.iter().filter(|s| s.month == month).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    fn mois(text: &str) -> Month {
        Month::parse(text).unwrap()
    }

    fn enveloppe(id: i64, nom: &str, dotation: &str, categories: &[&str]) -> Wallet {
        Wallet {
            id,
            name: nom.to_string(),
            allocation: dec(dotation),
            categories: categories.iter().map(|c| c.to_string()).collect(),
            carry_to: None,
            parent: None,
            start: mois("2026-01"),
        }
    }

    fn depense(month: &str, category: &str, montant: &str) -> Movement {
        Movement {
            month: mois(month),
            category: category.to_string(),
            amount: dec(montant),
        }
    }

    fn solde(states: &[MonthlyState], id: i64, month: &str) -> Decimal {
        states
            .iter()
            .find(|s| s.wallet_id == id && s.month == mois(month))
            .unwrap()
            .balance
    }

    #[test]
    fn a_month_rolls_over_to_the_next() {
        assert_eq!(mois("2026-01").next(), mois("2026-02"));
        assert_eq!(mois("2026-12").next(), mois("2027-01"));
    }

    #[test]
    fn a_month_reads_and_writes_itself() {
        assert_eq!(mois("2026-09").to_string(), "2026-09");
        assert_eq!(Month::parse("2026-13"), None);
        assert_eq!(Month::parse("charabia"), None);
    }

    /// L'exemple donné : 300 dotés, 290 dépensés, le mois suivant rouvre à 310.
    #[test]
    fn the_leftover_reopens_the_next_month() {
        let wallets = vec![enveloppe(1, "Alimentation", "300", &["groceries"])];
        let movements = vec![depense("2026-01", "groceries", "-290")];

        let states = project(&wallets, &movements, &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("10"));
        assert_eq!(solde(&states, 1, "2026-02"), dec("310"));
    }

    /// Sans dépense, l'enveloppe cumule sa dotation mois après mois.
    #[test]
    fn an_untouched_wallet_accumulates() {
        let wallets = vec![enveloppe(1, "Abonnements", "50", &["subscriptions"])];

        let states = project(&wallets, &[], &[], mois("2026-03"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("50"));
        assert_eq!(solde(&states, 1, "2026-03"), dec("150"));
    }

    /// Un dépassement se reporte en dette : l'effacer au changement de mois
    /// donnerait une enveloppe qui se renfloue toute seule.
    #[test]
    fn an_overspend_carries_its_debt_forward() {
        let wallets = vec![enveloppe(1, "Alimentation", "300", &["groceries"])];
        let movements = vec![depense("2026-01", "groceries", "-350")];

        let states = project(&wallets, &movements, &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("-50"));
        assert_eq!(solde(&states, 1, "2026-02"), dec("250"));
    }

    /// Le second exemple donné : A repart à sa dotation, son reliquat va chez
    /// B, qui démarre donc à sa propre dotation augmentée de ce reliquat.
    #[test]
    fn the_leftover_can_be_sent_to_another_wallet() {
        let mut a = enveloppe(1, "A", "300", &["groceries"]);
        a.carry_to = Some(2);
        let b = enveloppe(2, "B", "300", &["leisure"]);
        let movements = vec![
            depense("2026-01", "groceries", "-290"),
            // B consomme sa dotation : il rouvre donc sur la seule dotation du
            // mois, augmentée de ce qui lui vient de A.
            depense("2026-01", "leisure", "-300"),
        ];

        let states = project(&[a, b], &movements, &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("10"));
        // A rouvre à sa seule dotation : son reliquat est parti.
        assert_eq!(solde(&states, 1, "2026-02"), dec("300"));
        // B rouvre à sa dotation augmentée des 10 venus de A.
        assert_eq!(solde(&states, 2, "2026-02"), dec("310"));
    }

    /// Le report ne crée ni ne détruit d'argent, quelle que soit la
    /// destination : c'est ce qui garantit que la somme des enveloppes garde un
    /// sens d'un mois sur l'autre.
    #[test]
    fn redirecting_a_leftover_conserves_the_total() {
        let mut a = enveloppe(1, "A", "300", &["groceries"]);
        a.carry_to = Some(2);
        let b = enveloppe(2, "B", "120", &["leisure"]);
        let movements = vec![
            depense("2026-01", "groceries", "-290"),
            depense("2026-01", "leisure", "-20"),
        ];

        let states = project(&[a, b], &movements, &[], mois("2026-02"));

        let fin_janvier = solde(&states, 1, "2026-01") + solde(&states, 2, "2026-01");
        let ouverture_fevrier = states
            .iter()
            .filter(|s| s.month == mois("2026-02"))
            .map(|s| s.carried_in)
            .sum::<Decimal>();
        assert_eq!(fin_janvier, ouverture_fevrier);
    }

    /// Deux enveloppes qui se renvoient leur reliquat l'échangent, sans que le
    /// calcul tourne en rond : un mois se déduit du précédent, jamais de
    /// lui-même.
    #[test]
    fn two_wallets_can_swap_their_leftovers() {
        let mut a = enveloppe(1, "A", "100", &["groceries"]);
        a.carry_to = Some(2);
        let mut b = enveloppe(2, "B", "200", &["leisure"]);
        b.carry_to = Some(1);

        let states = project(&[a, b], &[], &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-02"), dec("300"));
        assert_eq!(solde(&states, 2, "2026-02"), dec("300"));
    }

    /// Une destination disparue ne doit pas engloutir le reliquat.
    #[test]
    fn an_unknown_target_keeps_the_leftover_in_place() {
        let mut a = enveloppe(1, "A", "300", &["groceries"]);
        a.carry_to = Some(404);

        let states = project(&[a], &[], &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-02"), dec("600"));
    }

    /// Se désigner soi-même revient à garder son reliquat, sans le compter deux
    /// fois.
    #[test]
    fn pointing_at_itself_is_the_ordinary_carry() {
        let mut a = enveloppe(1, "A", "300", &["groceries"]);
        a.carry_to = Some(1);

        let states = project(&[a], &[], &[], mois("2026-02"));

        assert_eq!(solde(&states, 1, "2026-02"), dec("600"));
    }

    /// Un remboursement regarnit l'enveloppe : la dépense est un solde signé,
    /// pas un total de sorties.
    #[test]
    fn a_refund_replenishes_the_envelope() {
        let wallets = vec![enveloppe(1, "Alimentation", "300", &["groceries"])];
        let movements = vec![
            depense("2026-01", "groceries", "-290"),
            depense("2026-01", "groceries", "40"),
        ];

        let states = project(&wallets, &movements, &[], mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("50"));
    }

    /// Plusieurs catégories alimentent une même enveloppe.
    #[test]
    fn a_wallet_can_watch_several_categories() {
        let wallets = vec![enveloppe(1, "Nourriture", "400", &["groceries", "dining"])];
        let movements = vec![
            depense("2026-01", "groceries", "-250"),
            depense("2026-01", "dining", "-90"),
            depense("2026-01", "leisure", "-70"),
        ];

        let states = project(&wallets, &movements, &[], mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("60"));
    }

    /// Une enveloppe créée en cours de route ne se dote pas rétroactivement.
    #[test]
    fn a_wallet_starts_the_month_it_was_opened() {
        let mut tardive = enveloppe(2, "Tardive", "100", &["leisure"]);
        tardive.start = mois("2026-03");
        let wallets = vec![enveloppe(1, "Ancienne", "50", &["groceries"]), tardive];

        let states = project(&wallets, &[], &[], mois("2026-03"));

        assert!(
            !states
                .iter()
                .any(|s| s.wallet_id == 2 && s.month < mois("2026-03"))
        );
        assert_eq!(solde(&states, 2, "2026-03"), dec("100"));
    }

    #[test]
    fn without_wallets_there_is_nothing_to_project() {
        assert!(project(&[], &[], &[], mois("2026-01")).is_empty());
    }

    /// Demander un mois antérieur à toute dotation ne rend rien, plutôt que de
    /// dérouler l'histoire à l'envers.
    #[test]
    fn a_month_before_the_first_allocation_yields_nothing() {
        let wallets = vec![enveloppe(1, "A", "300", &["groceries"])];
        assert!(project(&wallets, &[], &[], mois("2025-12")).is_empty());
    }

    #[test]
    fn available_is_what_the_envelope_held_before_spending() {
        let wallets = vec![enveloppe(1, "A", "300", &["groceries"])];
        let movements = vec![depense("2026-01", "groceries", "-290")];
        let states = project(&wallets, &movements, &[], mois("2026-02"));

        let fevrier = states.iter().find(|s| s.month == mois("2026-02")).unwrap();
        assert_eq!(fevrier.available(), dec("310"));
    }

    fn fille(id: i64, nom: &str, dotation: &str, mere: i64, categories: &[&str]) -> Wallet {
        let mut w = enveloppe(id, nom, dotation, categories);
        w.parent = Some(mere);
        w
    }

    /// La mère dote ses filles : ce qu'il lui reste finance ses catégories.
    #[test]
    fn a_mother_funds_her_children_out_of_her_own_allocation() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let a = fille(2, "Alimentation", "300", 1, &["groceries"]);
        let b = fille(3, "Restaurants", "150", 1, &["dining"]);

        let effective = effective_allocations(&[mere, a, b]);

        assert_eq!(effective[&1], dec("50"));
        assert_eq!(effective[&2], dec("300"));
        assert_eq!(effective[&3], dec("150"));
    }

    /// L'argent des filles ne doit pas être compté deux fois : la somme des
    /// dotations réelles vaut celle des seules enveloppes de premier rang.
    #[test]
    fn the_effective_allocations_sum_to_the_roots() {
        let wallets = vec![
            enveloppe(1, "Vie courante", "500", &["housing"]),
            fille(2, "Alimentation", "300", 1, &["groceries"]),
            fille(3, "Restaurants", "150", 1, &["dining"]),
            enveloppe(4, "Abonnements", "50", &["subscriptions"]),
        ];

        let total: Decimal = effective_allocations(&wallets).values().sum();

        // 500 + 50 : les 450 des filles sont déjà dans les 500 de leur mère.
        assert_eq!(total, dec("550"));
    }

    /// Des filles trop gourmandes laissent la mère en négatif : c'est un
    /// signal à montrer, non une erreur à corriger en douce.
    #[test]
    fn an_overallocated_mother_goes_negative() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let a = fille(2, "Alimentation", "300", 1, &["groceries"]);
        let b = fille(3, "Restaurants", "300", 1, &["dining"]);

        assert_eq!(effective_allocations(&[mere, a, b])[&1], dec("-100"));
    }

    /// L'emboîtement tient sur plusieurs niveaux.
    #[test]
    fn nesting_holds_over_several_levels() {
        let wallets = vec![
            enveloppe(1, "Vie courante", "500", &["housing"]),
            fille(2, "Nourriture", "400", 1, &["groceries"]),
            fille(3, "Restaurants", "150", 2, &["dining"]),
        ];

        let effective = effective_allocations(&wallets);

        assert_eq!(effective[&1], dec("100"));
        assert_eq!(effective[&2], dec("250"));
        assert_eq!(effective[&3], dec("150"));
        assert_eq!(effective.values().sum::<Decimal>(), dec("500"));
    }

    /// La projection applique la dotation réelle, pas celle qui est déclarée.
    #[test]
    fn the_projection_uses_the_effective_allocation() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let fille_a = fille(2, "Alimentation", "300", 1, &["groceries"]);
        let movements = vec![depense("2026-01", "groceries", "-290")];

        let states = project(&[mere, fille_a], &movements, &[], mois("2026-01"));

        // 500 dotés, 300 confiés à la fille : il reste 200 à la mère.
        assert_eq!(solde(&states, 1, "2026-01"), dec("200"));
        assert_eq!(solde(&states, 2, "2026-01"), dec("10"));
    }

    /// Le report reste propre à chaque enveloppe : une fille ne reverse rien à
    /// sa mère sans qu'on l'ait demandé.
    #[test]
    fn a_child_keeps_its_own_leftover() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let fille_a = fille(2, "Alimentation", "300", 1, &["groceries"]);
        let movements = vec![depense("2026-01", "groceries", "-290")];

        let states = project(&[mere, fille_a], &movements, &[], mois("2026-02"));

        assert_eq!(solde(&states, 2, "2026-02"), dec("310"));
        assert_eq!(solde(&states, 1, "2026-02"), dec("400"));
    }

    /// Une fille peut tout de même désigner sa mère, explicitement.
    #[test]
    fn a_child_may_still_be_told_to_report_upwards() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let mut fille_a = fille(2, "Alimentation", "300", 1, &["groceries"]);
        fille_a.carry_to = Some(1);
        let movements = vec![depense("2026-01", "groceries", "-290")];

        let states = project(&[mere, fille_a], &movements, &[], mois("2026-02"));

        assert_eq!(solde(&states, 2, "2026-02"), dec("300"));
        // 200 de janvier, plus les 10 de sa fille, plus les 200 de février.
        assert_eq!(solde(&states, 1, "2026-02"), dec("410"));
    }

    #[test]
    fn a_branch_gathers_the_whole_descent() {
        let wallets = vec![
            enveloppe(1, "Vie courante", "500", &["housing"]),
            fille(2, "Nourriture", "400", 1, &["groceries"]),
            fille(3, "Restaurants", "150", 2, &["dining"]),
            enveloppe(4, "Abonnements", "50", &["subscriptions"]),
        ];

        assert_eq!(branch(&wallets, 1), HashSet::from([1, 2, 3]));
        assert_eq!(branch(&wallets, 2), HashSet::from([2, 3]));
        assert_eq!(branch(&wallets, 4), HashSet::from([4]));
    }

    #[test]
    fn what_is_handed_to_children_is_counted() {
        let wallets = vec![
            enveloppe(1, "Vie courante", "500", &["housing"]),
            fille(2, "Alimentation", "300", 1, &["groceries"]),
            fille(3, "Restaurants", "150", 1, &["dining"]),
        ];

        assert_eq!(handed_to_children(&wallets, 1), dec("450"));
        assert_eq!(handed_to_children(&wallets, 2), Decimal::ZERO);
    }

    /// Se contenir soi-même se retirerait sa propre dotation : le calcul
    /// l'ignore, faute de quoi l'enveloppe tomberait à zéro sans raison.
    #[test]
    fn a_wallet_that_contains_itself_is_ignored() {
        let mut seule = enveloppe(1, "A", "300", &["groceries"]);
        seule.parent = Some(1);

        assert_eq!(effective_allocations(&[seule])[&1], dec("300"));
    }

    fn apport(id: i64, month: &str, montant: &str) -> Contribution {
        Contribution {
            wallet_id: id,
            month: mois(month),
            amount: dec(montant),
        }
    }

    /// Un apport ponctuel s'ajoute au mois où il est versé, hors dotation.
    #[test]
    fn a_one_off_contribution_adds_to_its_month() {
        let wallets = vec![enveloppe(1, "Vacances", "100", &["leisure"])];
        let apports = vec![apport(1, "2026-01", "500")];

        let states = project(&wallets, &[], &apports, mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("600"));
    }

    /// Il ne vaut que pour son mois : la dotation, elle, revient chaque mois.
    #[test]
    fn a_one_off_contribution_does_not_repeat() {
        let wallets = vec![enveloppe(1, "Vacances", "100", &["leisure"])];
        let apports = vec![apport(1, "2026-01", "500")];

        let states = project(&wallets, &[], &apports, mois("2026-02"));

        // Reporté, mais non reversé : 600 de janvier, plus les 100 de février.
        assert_eq!(solde(&states, 1, "2026-02"), dec("700"));
        let fevrier = states
            .iter()
            .find(|s| s.wallet_id == 1 && s.month == mois("2026-02"))
            .unwrap();
        assert_eq!(fevrier.contributions, Decimal::ZERO);
    }

    /// Il est comptabilisé à part : l'écran doit pouvoir distinguer ce qui
    /// vient de la consigne permanente de ce qui a été versé à la main.
    #[test]
    fn a_contribution_is_counted_apart_from_the_allocation() {
        let wallets = vec![enveloppe(1, "Vacances", "100", &["leisure"])];
        let apports = vec![apport(1, "2026-01", "500")];

        let states = project(&wallets, &[], &apports, mois("2026-01"));
        let janvier = &states[0];

        assert_eq!(janvier.allocation, dec("100"));
        assert_eq!(janvier.contributions, dec("500"));
        assert_eq!(janvier.available(), dec("600"));
    }

    /// Un apport ne se prend pas sur la dotation de la mère : il vient de la
    /// part non allouée du compte, non du budget de la branche.
    #[test]
    fn a_contribution_to_a_child_does_not_impoverish_its_mother() {
        let mere = enveloppe(1, "Vie courante", "500", &["housing"]);
        let mut fille = enveloppe(2, "Alimentation", "300", &["groceries"]);
        fille.parent = Some(1);
        let apports = vec![apport(2, "2026-01", "200")];

        let states = project(&[mere, fille], &[], &apports, mois("2026-01"));

        // La mère garde ses 200 : 500 dotés, 300 confiés à sa fille.
        assert_eq!(solde(&states, 1, "2026-01"), dec("200"));
        assert_eq!(solde(&states, 2, "2026-01"), dec("500"));
    }

    /// Un montant négatif retire de l'enveloppe.
    #[test]
    fn a_negative_contribution_is_a_withdrawal() {
        let wallets = vec![enveloppe(1, "Vacances", "100", &["leisure"])];
        let apports = vec![apport(1, "2026-01", "-40")];

        let states = project(&wallets, &[], &apports, mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("60"));
    }

    /// Plusieurs apports le même mois s'additionnent.
    #[test]
    fn several_contributions_in_one_month_add_up() {
        let wallets = vec![enveloppe(1, "Vacances", "0", &["leisure"])];
        let apports = vec![apport(1, "2026-01", "200"), apport(1, "2026-01", "50")];

        let states = project(&wallets, &[], &apports, mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("250"));
    }

    /// Un apport visant une enveloppe inconnue est ignoré, sans fausser les
    /// autres : une enveloppe supprimée ne doit pas emporter le calcul.
    #[test]
    fn a_contribution_to_an_unknown_wallet_is_ignored() {
        let wallets = vec![enveloppe(1, "Vacances", "100", &["leisure"])];
        let apports = vec![apport(404, "2026-01", "500")];

        let states = project(&wallets, &[], &apports, mois("2026-01"));

        assert_eq!(solde(&states, 1, "2026-01"), dec("100"));
    }

    /// La projection est bornée : une date de départ aberrante ne doit pas
    /// faire tourner le serveur à chaque affichage.
    #[test]
    fn the_projection_is_bounded() {
        let mut ancienne = enveloppe(1, "A", "1", &["groceries"]);
        ancienne.start = mois("1900-01");

        let states = project(&ancienne_seule(ancienne), &[], &[], mois("2026-01"));

        assert!(states.len() <= MAX_MONTHS);
    }

    fn ancienne_seule(w: Wallet) -> Vec<Wallet> {
        vec![w]
    }
}
