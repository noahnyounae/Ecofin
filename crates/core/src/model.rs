//! Types de domaine, indépendants du fournisseur de données.
//!
//! Tout provider (Enable Banking aujourd'hui, un import CSV ou un agrégateur
//! payant demain) doit savoir produire ces structures. C'est la seule chose que
//! le reste de l'application connaît. Le vocabulaire y reste neutre : on parle
//! d'« établissement » et non d'ASPSP, terme propre à la DSP2.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Un compte bancaire accessible via une session de consentement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Identifiant du compte chez le provider (`uid` chez Enable Banking).
    pub id: String,
    /// Session de consentement qui donne accès à ce compte.
    pub session_id: String,
    pub institution_name: String,
    pub institution_country: String,
    /// Libellé du compte tel que renvoyé par la banque, quand elle en donne un.
    pub name: Option<String>,
    pub iban: Option<String>,
    pub currency: String,
    /// Dernière synchronisation réussie des opérations.
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl Account {
    /// Libellé court pour l'affichage : nom du compte si la banque en fournit
    /// un, sinon les quatre derniers caractères de l'IBAN.
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.name
            && !name.trim().is_empty()
        {
            return name.clone();
        }
        match &self.iban {
            Some(iban) if iban.len() >= 4 => format!("···{}", &iban[iban.len() - 4..]),
            _ => self.id.chars().take(8).collect(),
        }
    }
}

/// Solde d'un compte à un instant donné.
///
/// Une banque renvoie souvent plusieurs soldes pour le même compte
/// (`CLBD`, `XPCD`, `ITAV`...), d'où le champ `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub account_id: String,
    /// Type de solde tel que nommé par la norme Berlin Group.
    pub kind: String,
    pub amount: Decimal,
    pub currency: String,
    pub reference_date: Option<NaiveDate>,
}

/// Une opération sur un compte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Identifiant fourni par la banque. Sert de clé de déduplication.
    pub id: String,
    pub account_id: String,
    /// Montant signé : négatif pour un débit, positif pour un crédit.
    ///
    /// Enable Banking transporte un montant toujours positif accompagné d'un
    /// sens (`CRDT`/`DBIT`) ; le signe est appliqué à la conversion pour que le
    /// reste de l'application n'ait plus à y penser.
    pub amount: Decimal,
    pub currency: String,
    /// Date de comptabilisation.
    pub booking_date: Option<NaiveDate>,
    /// Date de valeur, quand elle diffère de la date de comptabilisation.
    pub value_date: Option<NaiveDate>,
    /// Libellé brut, tel qu'affiché sur le relevé.
    pub description: String,
    /// Nom de la contrepartie, quand la banque le distingue du libellé.
    pub counterparty: Option<String>,
    /// Code métier Berlin Group, ex. `PMNT-RCDT-SALA` pour un salaire.
    pub bank_category: Option<String>,
    /// `false` tant que l'opération n'est pas définitivement comptabilisée.
    pub booked: bool,
}

impl Transaction {
    /// Date à retenir pour trier et regrouper : on privilégie la date de
    /// comptabilisation, avec la date de valeur en repli.
    pub fn effective_date(&self) -> Option<NaiveDate> {
        self.booking_date.or(self.value_date)
    }

    pub fn is_debit(&self) -> bool {
        self.amount.is_sign_negative()
    }
}

/// Un établissement bancaire proposé par le provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Institution {
    /// Nom exact attendu par l'API lors de la demande d'autorisation.
    pub name: String,
    pub country: String,
    /// Durée de consentement maximale acceptée, en secondes.
    pub max_consent_validity: Option<i64>,
    /// Types d'utilisateur acceptés : `personal`, `business`.
    pub psu_types: Vec<String>,
    /// L'intégration de cette banque est encore en test chez le provider.
    pub beta: bool,
}

impl Institution {
    /// Durée de consentement en jours, arrondie à l'inférieur.
    pub fn max_consent_days(&self) -> Option<i64> {
        self.max_consent_validity.map(|s| s / 86_400)
    }
}

/// Une session de consentement : le droit de lire un ensemble de comptes
/// pendant une durée limitée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Identifiant de session chez le provider.
    pub id: String,
    pub institution_name: String,
    pub institution_country: String,
    pub status: LinkStatus,
    /// Échéance du consentement. Passée cette date, il faut refaire `link new`.
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Link {
    /// Nombre de jours restants avant expiration du consentement. Négatif si
    /// l'échéance est déjà passée.
    pub fn days_remaining(&self) -> Option<i64> {
        self.valid_until
            .map(|until| (until - Utc::now()).num_days())
    }
}

/// Où en est une session de consentement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkStatus {
    /// Autorisation demandée, l'utilisateur ne l'a pas encore accordée.
    Pending,
    /// Session active, les comptes sont lisibles.
    Linked,
    /// Consentement échu : il faut relancer `ecofin link new`.
    Expired,
    /// Refusée par l'utilisateur, ou révoquée par la banque.
    Rejected,
}

impl std::fmt::Display for LinkStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LinkStatus::Pending => "en attente",
            LinkStatus::Linked => "active",
            LinkStatus::Expired => "expirée",
            LinkStatus::Rejected => "révoquée",
        };
        f.write_str(s)
    }
}
