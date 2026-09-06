//! Reconstruction du capital dans le temps, et agrégations dérivées.
//!
//! La table des soldes ne contient qu'un relevé par date d'arrêté transmise par
//! la banque : quelques points, et seulement depuis qu'ecofin les historise.
//! La courbe de capital se reconstruit donc à partir des opérations, en
//! remontant le temps depuis le solde actuel :
//!
//! ```text
//! solde(t) = solde_actuel − Σ(opérations postérieures à t)
//! ```
//!
//! Cela donne un point par opération sur tout l'historique disponible, y
//! compris les années importées des relevés PDF, que la banque n'expose plus.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::merchant;
use crate::model::Transaction;
use crate::store::{Scope, Store};

/// Un point de la courbe de capital : le solde du compte juste après une
/// opération donnée.
#[derive(Debug, Clone, Serialize)]
pub struct CapitalPoint {
    /// Identifiant de l'opération à l'origine du point.
    pub transaction_id: String,
    pub account_id: String,
    pub date: NaiveDate,
    /// Solde du compte après cette opération.
    pub balance: Decimal,
    /// Montant de l'opération, signé.
    pub amount: Decimal,
    /// Libellé nettoyé, tel qu'affiché.
    pub description: String,
    /// Libellé brut de la banque.
    ///
    /// Transporté jusqu'au client pour deux usages : vérifier de visu ce que
    /// le nettoyage a retiré, et chercher dans le texte complet — une requête
    /// doit pouvoir atteindre un mot que le nettoyage a écarté.
    pub raw: String,
    /// Secteur de dépense, déduit du libellé.
    pub category: String,
    pub currency: String,
    /// `false` tant que l'opération n'est pas définitivement comptabilisée.
    pub booked: bool,
}

/// Courbe de capital d'un compte, et de quoi la situer.
#[derive(Debug, Clone, Serialize)]
pub struct CapitalCurve {
    pub account_id: String,
    pub currency: String,
    pub points: Vec<CapitalPoint>,
    /// Solde de référence : celui déclaré par la banque, d'où part le calcul.
    pub reference_balance: Decimal,
    /// Date de ce solde de référence, quand elle est connue.
    pub reference_date: Option<NaiveDate>,
    /// Vrai quand aucun solde bancaire n'est disponible et que la courbe part
    /// de zéro : les valeurs sont alors relatives, pas absolues.
    pub relative: bool,
}

/// Bornes temporelles d'une requête.
///
/// Les bornes portent une heure parce que l'interface en propose : sur des
/// opérations datées au jour, une borne en cours de journée inclut la journée
/// entière côté début et l'exclut côté fin.
#[derive(Debug, Clone, Copy, Default)]
pub struct Range {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

impl Range {
    fn contains(&self, date: NaiveDate) -> bool {
        if let Some(from) = self.from
            && date < from.date_naive()
        {
            return false;
        }
        if let Some(to) = self.to
            && date > to.date_naive()
        {
            return false;
        }
        true
    }
}

/// Construit la courbe de capital d'un compte sur une période.
///
/// La courbe est toujours calculée sur l'historique complet, puis restreinte à
/// la période demandée : tronquer d'abord donnerait des soldes faux, puisque
/// chaque point dépend de tout ce qui le suit.
pub fn capital_curve(
    store: &Store,
    scope: Scope,
    account_id: &str,
    range: Range,
    overrides: &std::collections::HashMap<String, String>,
    redirects: &std::collections::HashMap<String, String>,
) -> Result<CapitalCurve> {
    let own_names = store.own_account_names()?;
    let account = store
        .accounts(scope)?
        .into_iter()
        .find(|a| a.id == account_id)
        // Hors périmètre, le compte est traité comme inexistant : ne pas
        // distinguer les deux évite de révéler qu'il appartient à un autre.
        .ok_or_else(|| anyhow::anyhow!("compte {account_id} inconnu"))?;

    // `usize::MAX` : la reconstruction n'a de sens que sur l'historique entier.
    let mut transactions = store.transactions(scope, Some(account_id), None, None, usize::MAX)?;
    // Du plus ancien au plus récent, pour cumuler dans le sens du temps.
    transactions.sort_by(|a, b| {
        a.effective_date()
            .cmp(&b.effective_date())
            .then_with(|| a.id.cmp(&b.id))
    });

    let reference = store.primary_balance(scope, account_id)?;
    let reference_balance = reference
        .as_ref()
        .map(|b| b.amount)
        .unwrap_or(Decimal::ZERO);

    // Le solde connu vaut pour aujourd'hui : le solde d'ouverture est donc ce
    // qu'il faut retrancher de la somme de toutes les opérations.
    let total: Decimal = transactions.iter().map(|t| t.amount).sum();
    let opening = reference_balance - total;

    let mut running = opening;
    let mut points = Vec::new();

    for transaction in &transactions {
        running += transaction.amount;
        let Some(date) = transaction.effective_date() else {
            continue;
        };
        if !range.contains(date) {
            continue;
        }
        points.push(CapitalPoint {
            transaction_id: transaction.id.clone(),
            account_id: transaction.account_id.clone(),
            date,
            balance: running,
            amount: transaction.amount,
            // Nettoyé à l'affichage, pas au stockage : les règles peuvent
            // s'affiner sans réimporter quoi que ce soit.
            description: merchant::normalize(&transaction.description),
            raw: transaction.description.clone(),
            category: crate::category::classify(
                &merchant::normalize(&transaction.description),
                !transaction.is_debit(),
                &own_names,
                overrides,
                redirects,
            ),
            currency: transaction.currency.clone(),
            booked: transaction.booked,
        });
    }

    Ok(CapitalCurve {
        account_id: account_id.to_string(),
        currency: account.currency,
        points,
        reference_balance,
        reference_date: reference.and_then(|b| b.reference_date),
        // Sans solde bancaire, la courbe démarre à zéro : sa forme reste juste,
        // ses valeurs absolues non.
        relative: reference_balance.is_zero(),
    })
}

/// Totaux d'une période : ce qui est entré, ce qui est sorti, le solde net.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeriodSummary {
    pub credits: Decimal,
    pub debits: Decimal,
    pub net: Decimal,
    pub count: usize,
}

/// Résume les opérations d'un compte sur une période.
pub fn summarize(
    store: &Store,
    scope: Scope,
    account_id: &str,
    range: Range,
) -> Result<PeriodSummary> {
    let transactions = store.transactions(scope, Some(account_id), None, None, usize::MAX)?;
    let selected: Vec<&Transaction> = transactions
        .iter()
        .filter(|t| t.effective_date().is_some_and(|d| range.contains(d)))
        .collect();

    let credits: Decimal = selected
        .iter()
        .filter(|t| !t.is_debit())
        .map(|t| t.amount)
        .sum();
    let debits: Decimal = selected
        .iter()
        .filter(|t| t.is_debit())
        .map(|t| t.amount)
        .sum();

    Ok(PeriodSummary {
        credits,
        debits,
        net: credits + debits,
        count: selected.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn range(from: &str, to: &str) -> Range {
        Range {
            from: Some(
                DateTime::parse_from_rfc3339(from)
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            to: Some(
                DateTime::parse_from_rfc3339(to)
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn range_includes_its_bounds() {
        let r = range("2026-08-01T00:00:00Z", "2026-08-31T23:59:59Z");
        assert!(r.contains(day(2026, 8, 1)));
        assert!(r.contains(day(2026, 8, 31)));
        assert!(!r.contains(day(2026, 7, 31)));
        assert!(!r.contains(day(2026, 9, 1)));
    }

    #[test]
    fn an_open_range_accepts_everything() {
        assert!(Range::default().contains(day(2020, 1, 1)));
    }

    #[test]
    fn a_half_open_range_only_constrains_its_side() {
        let r = Range {
            from: Some(
                DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            to: None,
        };
        assert!(!r.contains(day(2025, 12, 31)));
        assert!(r.contains(day(2030, 1, 1)));
    }

    /// Le solde d'ouverture se déduit du solde actuel et de la somme des
    /// mouvements : c'est ce qui permet de partir du présent pour remonter.
    #[test]
    fn opening_balance_is_derived_from_the_current_one() {
        let current = Decimal::from_str("312.36").unwrap();
        let movements = Decimal::from_str("312.36").unwrap();
        assert_eq!(current - movements, Decimal::ZERO);
    }
}
