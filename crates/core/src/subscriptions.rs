//! Détection des abonnements et prélèvements récurrents.
//!
//! Une banque ne dit jamais qu'une opération est un abonnement : c'est un
//! virement ou un paiement carte comme un autre. La régularité seule le
//! trahit — un même bénéficiaire, à intervalle constant, pour un montant
//! stable.
//!
//! La détection repose donc sur trois signaux, et exige les trois :
//!
//! - **un bénéficiaire commun**, une fois les libellés nettoyés ;
//! - **un rythme**, mesuré par l'écart médian entre échéances ;
//! - **une régularité**, c'est-à-dire des écarts qui se ressemblent.
//!
//! L'écart *médian* et non moyen : une échéance manquée ou un rattrapage
//! doublerait un intervalle, et une moyenne s'en trouverait déportée alors que
//! la médiane l'ignore.

use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::HashMap;

use crate::merchant;
use crate::model::Transaction;
use crate::store::{Scope, Store};

/// Nombre minimal d'échéances pour parler de rythme.
///
/// Deux opérations ne définissent qu'un seul intervalle, dont rien ne dit
/// qu'il se répétera. Il en faut trois pour observer une régularité.
const MIN_OCCURRENCES: usize = 3;

/// Écart toléré autour d'un rythme, en proportion de sa période.
///
/// Un prélèvement mensuel tombe entre 28 et 31 jours selon les mois, et glisse
/// encore avec les week-ends : 25 % de tolérance absorbe ces décalages sans
/// confondre un rythme mensuel avec un trimestriel.
const CADENCE_TOLERANCE: f64 = 0.25;

/// Proportion d'intervalles devant respecter le rythme.
///
/// En deçà, les échéances sont trop irrégulières pour qu'on parle d'abonnement :
/// c'est un commerçant fréquenté souvent, pas un prélèvement automatique.
const MIN_REGULARITY: f64 = 0.6;

/// Régularité exigée des rythmes longs.
///
/// Un rythme annuel s'infère de trois ou quatre échéances étalées sur autant
/// d'années : la coïncidence est bien plus facile que sur trente échéances
/// mensuelles. Des retraits d'espèces espacés au hasard ont ainsi été pris pour
/// un abonnement annuel, d'où cette exigence renforcée.
const MIN_REGULARITY_LONG: f64 = 0.8;

/// Délai au-delà duquel un abonnement est tenu pour arrêté.
///
/// Deux périodes sans échéance : une seule pourrait n'être qu'un retard.
const LAPSED_PERIODS: i64 = 2;

/// Rythme d'un prélèvement récurrent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cadence {
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
}

impl Cadence {
    /// Période nominale, en jours.
    fn days(self) -> f64 {
        match self {
            Cadence::Weekly => 7.0,
            Cadence::Monthly => 30.44,
            Cadence::Quarterly => 91.31,
            Cadence::Yearly => 365.25,
        }
    }

    /// Rythme dont la période est la plus proche de l'écart observé.
    fn nearest(gap: f64) -> Option<Self> {
        [
            Cadence::Weekly,
            Cadence::Monthly,
            Cadence::Quarterly,
            Cadence::Yearly,
        ]
        .into_iter()
        .find(|cadence| {
            let period = cadence.days();
            (gap - period).abs() <= period * CADENCE_TOLERANCE
        })
    }

    /// Nombre d'échéances par mois, pour ramener les rythmes à une base
    /// commune et pouvoir les additionner.
    fn per_month(self) -> Decimal {
        match self {
            Cadence::Weekly => Decimal::try_from(30.44 / 7.0).unwrap_or(Decimal::ONE),
            Cadence::Monthly => Decimal::ONE,
            Cadence::Quarterly => Decimal::try_from(1.0 / 3.0).unwrap_or(Decimal::ONE),
            Cadence::Yearly => Decimal::try_from(1.0 / 12.0).unwrap_or(Decimal::ONE),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cadence::Weekly => "hebdomadaire",
            Cadence::Monthly => "mensuel",
            Cadence::Quarterly => "trimestriel",
            Cadence::Yearly => "annuel",
        }
    }
}

/// Un prélèvement récurrent détecté.
#[derive(Debug, Clone, Serialize)]
pub struct Subscription {
    /// Libellé nettoyé, commun aux échéances.
    pub label: String,
    pub cadence: Cadence,
    /// Montant représentatif : la médiane, insensible à une échéance
    /// exceptionnelle.
    pub amount: Decimal,
    /// Coût ramené au mois, pour comparer et additionner des rythmes
    /// différents.
    pub monthly_cost: Decimal,
    pub occurrences: usize,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
    /// Part des intervalles respectant le rythme, entre 0 et 1.
    pub regularity: f64,
    /// Faux quand plus aucune échéance n'est venue depuis deux périodes.
    pub active: bool,
    /// Vrai si les montants varient sensiblement d'une échéance à l'autre.
    ///
    /// Un abonnement à montant variable reste un abonnement — une facture
    /// d'électricité, par exemple — mais le montant affiché est alors moins
    /// représentatif.
    pub variable_amount: bool,
}

/// Cherche les prélèvements récurrents parmi les opérations d'un compte.
///
/// Seuls les débits sont examinés : un revenu régulier n'est pas un abonnement.
pub fn detect(store: &Store, scope: Scope, account_id: &str) -> Result<Vec<Subscription>> {
    let transactions = store.transactions(scope, Some(account_id), None, None, usize::MAX)?;
    Ok(from_transactions(&transactions))
}

/// Cœur de la détection, isolé de la base pour être éprouvable.
pub fn from_transactions(transactions: &[Transaction]) -> Vec<Subscription> {
    // Regroupement sur une clé repliée : la casse varie d'un mois à l'autre
    // chez la banque, et « SCI Berthelot » ne doit pas se séparer de
    // « SCI BERTHELOT ». On garde en regard les graphies rencontrées pour
    // pouvoir en afficher une.
    let mut groups: HashMap<String, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    let mut spellings: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for transaction in transactions {
        if !transaction.is_debit() {
            continue;
        }
        let Some(date) = transaction.effective_date() else {
            continue;
        };
        let label = merchant::normalize(&transaction.description);
        let key = merchant::fold(&label);

        *spellings
            .entry(key.clone())
            .or_default()
            .entry(label)
            .or_insert(0) += 1;
        groups
            .entry(key)
            .or_default()
            .push((date, transaction.amount.abs()));
    }

    let today = Utc::now().date_naive();
    let mut found: Vec<Subscription> = groups
        .into_iter()
        .filter_map(|(key, entries)| {
            let label = preferred_spelling(spellings.get(&key))?;
            analyse(label, entries, today)
        })
        .collect();

    // Le plus coûteux d'abord : c'est là que se trouvent les économies.
    found.sort_by(|a, b| {
        b.monthly_cost
            .cmp(&a.monthly_cost)
            .then_with(|| a.label.cmp(&b.label))
    });
    found
}

/// Graphie à afficher pour un groupe : la plus fréquemment rencontrée.
///
/// À égalité, l'ordre alphabétique tranche — sans quoi l'affichage changerait
/// d'une exécution à l'autre au gré du parcours de la table de hachage.
fn preferred_spelling(seen: Option<&HashMap<String, usize>>) -> Option<String> {
    seen?
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(label, _)| label.clone())
}

/// Décide si un groupe d'opérations forme un abonnement.
fn analyse(
    label: String,
    mut entries: Vec<(NaiveDate, Decimal)>,
    today: NaiveDate,
) -> Option<Subscription> {
    if entries.len() < MIN_OCCURRENCES {
        return None;
    }
    entries.sort_by_key(|(date, _)| *date);

    // Plusieurs achats le même jour ne sont pas des échéances distinctes :
    // c'est un commerçant fréquenté, pas un prélèvement.
    entries.dedup_by_key(|(date, _)| *date);
    if entries.len() < MIN_OCCURRENCES {
        return None;
    }

    let gaps: Vec<f64> = entries
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0).num_days() as f64)
        .collect();

    let typical_gap = median(&mut gaps.clone())?;
    let cadence = Cadence::nearest(typical_gap)?;

    // Régularité : part des intervalles proches du rythme retenu.
    let period = cadence.days();
    let regular = gaps
        .iter()
        .filter(|gap| (*gap - period).abs() <= period * CADENCE_TOLERANCE)
        .count();
    let regularity = regular as f64 / gaps.len() as f64;
    let required = match cadence {
        Cadence::Quarterly | Cadence::Yearly => MIN_REGULARITY_LONG,
        _ => MIN_REGULARITY,
    };
    if regularity < required {
        return None;
    }

    let mut amounts: Vec<Decimal> = entries.iter().map(|(_, amount)| *amount).collect();
    let amount = median_decimal(&mut amounts)?;

    // Montant variable : au moins une échéance s'écarte de plus d'un quart de
    // la médiane.
    let variable_amount = amounts.iter().any(|value| {
        let deviation = (*value - amount).abs();
        amount > Decimal::ZERO && deviation * Decimal::from(4) > amount
    });

    let first_seen = entries.first()?.0;
    let last_seen = entries.last()?.0;
    let active = (today - last_seen) <= Duration::days((period * LAPSED_PERIODS as f64) as i64);

    Some(Subscription {
        label,
        cadence,
        monthly_cost: amount * cadence.per_month(),
        amount,
        occurrences: entries.len(),
        first_seen,
        last_seen,
        regularity,
        active,
        variable_amount,
    })
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values[values.len() / 2])
}

fn median_decimal(values: &mut [Decimal]) -> Option<Decimal> {
    if values.is_empty() {
        return None;
    }
    values.sort();
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn tx(date: &str, amount: &str, label: &str) -> Transaction {
        Transaction {
            id: format!("{date}-{label}"),
            account_id: "acc".into(),
            amount: Decimal::from_str(amount).unwrap(),
            currency: "EUR".into(),
            booking_date: Some(NaiveDate::from_str(date).unwrap()),
            value_date: None,
            description: label.into(),
            counterparty: None,
            bank_category: None,
            booked: true,
        }
    }

    /// Un prélèvement mensuel régulier : le cas nominal.
    fn monthly(label: &str, amount: &str, months: u32) -> Vec<Transaction> {
        (0..months)
            .map(|i| {
                let date =
                    NaiveDate::from_ymd_opt(2025, 1, 15).unwrap() + Duration::days(30 * i as i64);
                tx(&date.to_string(), amount, label)
            })
            .collect()
    }

    #[test]
    fn detects_a_monthly_subscription() {
        let found = from_transactions(&monthly("CB NETFLIX COM", "-5.99", 6));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].cadence, Cadence::Monthly);
        assert_eq!(found[0].amount, Decimal::from_str("5.99").unwrap());
        assert_eq!(found[0].occurrences, 6);
    }

    /// Deux occurrences ne définissent qu'un intervalle : rien ne dit qu'il se
    /// répétera.
    #[test]
    fn ignores_a_pair_of_operations() {
        assert!(from_transactions(&monthly("CB QUELQUE CHOSE", "-9.99", 2)).is_empty());
    }

    /// Un commerçant fréquenté à intervalles quelconques n'est pas un
    /// abonnement, même vu vingt fois.
    #[test]
    fn ignores_an_irregular_merchant() {
        let days = [0, 3, 4, 11, 12, 13, 27, 40, 41, 42, 60];
        let transactions: Vec<_> = days
            .iter()
            .map(|d| {
                let date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + Duration::days(*d);
                tx(&date.to_string(), "-12.50", "CB UBER *EATS")
            })
            .collect();
        assert!(from_transactions(&transactions).is_empty());
    }

    /// Un revenu régulier — un salaire — n'est pas un abonnement.
    #[test]
    fn ignores_recurring_income() {
        let salary: Vec<_> = monthly("VIR SEPA SALAIRE", "-2450.00", 6)
            .into_iter()
            .map(|mut t| {
                t.amount = -t.amount;
                t
            })
            .collect();
        assert!(from_transactions(&salary).is_empty());
    }

    /// Une échéance manquée ne doit pas faire perdre la trace de l'abonnement :
    /// c'est le rôle de la médiane plutôt que de la moyenne.
    #[test]
    fn tolerates_a_missed_instalment() {
        let mut transactions = monthly("CB SPOTIFY", "-9.99", 8);
        transactions.remove(3);
        let found = from_transactions(&transactions);
        assert_eq!(found.len(), 1, "l'abonnement reste détecté");
        assert_eq!(found[0].cadence, Cadence::Monthly);
        assert!(found[0].regularity < 1.0, "l'irrégularité est signalée");
    }

    /// Des retraits d'espèces vaguement espacés d'un an ne forment pas un
    /// abonnement : le hasard suffit à produire cette régularité-là.
    #[test]
    fn rejects_a_loosely_yearly_rhythm() {
        // Un intervalle sur trois n'a rien d'annuel : la régularité tombe à
        // 67 %, sous l'exigence renforcée des rythmes longs. C'est le motif
        // qu'ont réellement produit des retraits d'espèces.
        let days = [0, 365, 500, 865];
        let transactions: Vec<_> = days
            .iter()
            .map(|d| {
                let date = NaiveDate::from_ymd_opt(2022, 1, 1).unwrap() + Duration::days(*d);
                tx(&date.to_string(), "-150.00", "CB RETRAIT DU")
            })
            .collect();
        assert!(from_transactions(&transactions).is_empty());
    }

    /// Une graphie changeante ne doit pas scinder une dépense en deux
    /// abonnements, dont l'un paraîtrait arrêté.
    #[test]
    fn merges_spellings_that_differ_only_by_case() {
        let mut transactions = monthly("VIR SEPA SCI Berthelot", "-565.00", 4);
        transactions.extend(
            monthly("VIR SEPA SCI BERTHELOT", "-565.00", 4)
                .into_iter()
                .enumerate()
                .map(|(i, mut t)| {
                    // Les quatre suivantes, dans la continuité des premières.
                    t.booking_date = Some(
                        NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()
                            + Duration::days(30 * (4 + i as i64)),
                    );
                    t.id = format!("second-{i}");
                    t
                }),
        );

        let found = from_transactions(&transactions);
        assert_eq!(found.len(), 1, "un seul abonnement, pas deux");
        assert_eq!(found[0].occurrences, 8);
    }

    /// La graphie retenue est la plus fréquente, et reste stable.
    #[test]
    fn shows_the_most_common_spelling() {
        let mut transactions = monthly("VIR SEPA Solygest", "-599.51", 5);
        transactions.push(tx("2025-06-20", "-599.51", "VIR SEPA SOLYGEST"));
        assert_eq!(from_transactions(&transactions)[0].label, "Solygest");
    }

    #[test]
    fn detects_a_yearly_subscription() {
        let transactions: Vec<_> = (0..4)
            .map(|i| {
                let date = NaiveDate::from_ymd_opt(2022, 3, 1).unwrap() + Duration::days(365 * i);
                tx(&date.to_string(), "-49.00", "CB ASSURANCE ANNUELLE")
            })
            .collect();
        let found = from_transactions(&transactions);
        assert_eq!(found[0].cadence, Cadence::Yearly);
    }

    /// Les rythmes doivent se comparer entre eux : un abonnement annuel à 120 €
    /// pèse 10 € par mois.
    #[test]
    fn normalises_cost_to_a_monthly_basis() {
        let transactions: Vec<_> = (0..3)
            .map(|i| {
                let date = NaiveDate::from_ymd_opt(2022, 3, 1).unwrap() + Duration::days(365 * i);
                tx(&date.to_string(), "-120.00", "CB ANNUEL")
            })
            .collect();
        let found = from_transactions(&transactions);
        let monthly = found[0].monthly_cost;
        assert!(
            monthly > Decimal::from_str("9.9").unwrap()
                && monthly < Decimal::from_str("10.1").unwrap(),
            "coût mensuel inattendu : {monthly}"
        );
    }

    /// Un abonnement dont les échéances ont cessé doit être signalé comme tel.
    #[test]
    fn marks_a_stopped_subscription_as_inactive() {
        let transactions: Vec<_> = (0..5)
            .map(|i| {
                let date = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap() + Duration::days(30 * i);
                tx(&date.to_string(), "-9.99", "CB ANCIEN SERVICE")
            })
            .collect();
        assert!(!from_transactions(&transactions)[0].active);
    }

    /// Plusieurs achats le même jour ne forment pas des échéances distinctes.
    #[test]
    fn does_not_mistake_same_day_purchases_for_instalments() {
        let transactions = vec![
            tx("2025-01-10", "-4.50", "CB BOULANGERIE"),
            tx("2025-01-10", "-4.50", "CB BOULANGERIE"),
            tx("2025-01-10", "-4.50", "CB BOULANGERIE"),
        ];
        assert!(from_transactions(&transactions).is_empty());
    }

    #[test]
    fn flags_a_variable_amount() {
        let mut transactions = monthly("PRLV SEPA ELECTRICITE", "-60.00", 5);
        transactions[2].amount = Decimal::from_str("-140.00").unwrap();
        assert!(from_transactions(&transactions)[0].variable_amount);
    }

    #[test]
    fn classifies_gaps_into_cadences() {
        assert_eq!(Cadence::nearest(7.0), Some(Cadence::Weekly));
        assert_eq!(Cadence::nearest(30.0), Some(Cadence::Monthly));
        assert_eq!(Cadence::nearest(31.0), Some(Cadence::Monthly));
        assert_eq!(Cadence::nearest(91.0), Some(Cadence::Quarterly));
        assert_eq!(Cadence::nearest(365.0), Some(Cadence::Yearly));
        // Un écart sans rythme reconnaissable n'en reçoit aucun.
        assert_eq!(Cadence::nearest(17.0), None);
        assert_eq!(Cadence::nearest(200.0), None);
    }
}
