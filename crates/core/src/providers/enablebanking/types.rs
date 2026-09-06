//! Représentation des réponses de l'API Enable Banking.
//!
//! Ces types collent au JSON renvoyé par l'API. La conversion vers les types de
//! domaine se fait dans `super::client`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ApplicationDto {
    pub name: String,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub redirect_urls: Vec<String>,
}

// ---------------------------------------------------------------------------
// Établissements
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AspspsEnvelope {
    #[serde(default)]
    pub aspsps: Vec<AspspDto>,
}

#[derive(Debug, Deserialize)]
pub struct AspspDto {
    pub name: String,
    pub country: String,
    /// Durée de consentement maximale, en secondes.
    #[serde(default)]
    pub maximum_consent_validity: Option<i64>,
    #[serde(default)]
    pub psu_types: Vec<String>,
    #[serde(default)]
    pub beta: bool,
}

// ---------------------------------------------------------------------------
// Autorisation
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AuthRequest<'a> {
    pub access: AccessSpec,
    pub aspsp: AspspRef<'a>,
    /// Valeur opaque que l'on retrouve sur l'URL de retour ; sert à vérifier
    /// que la réponse correspond bien à la demande qu'on a émise.
    pub state: &'a str,
    pub redirect_url: &'a str,
    pub psu_type: &'a str,
    /// Langue de l'interface bancaire, quand la banque la respecte.
    pub language: &'a str,
}

#[derive(Debug, Serialize)]
pub struct AccessSpec {
    /// Échéance du consentement, en RFC 3339.
    pub valid_until: String,
}

#[derive(Debug, Serialize)]
pub struct AspspRef<'a> {
    pub name: &'a str,
    pub country: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    /// URL vers laquelle envoyer l'utilisateur pour qu'il s'authentifie.
    pub url: String,
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SessionRequest<'a> {
    /// Code à usage unique récupéré sur l'URL de retour.
    pub code: &'a str,
}

/// Réponse de `POST /sessions` comme de `GET /sessions/{id}`.
///
/// Les deux endpoints ne renvoient pas tout à fait la même chose : `POST`
/// fournit un `session_id` et un tableau `accounts` d'objets détaillés, `GET`
/// omet le `session_id` et réduit `accounts` à des identifiants. On s'appuie
/// donc uniquement sur `accounts_data`, seul champ de forme stable, et le
/// détail de chaque compte est ensuite lu sur `/accounts/{uid}/details`.
#[derive(Debug, Deserialize)]
pub struct SessionResponse {
    /// Absent de `GET /sessions/{id}`, où l'identifiant est déjà connu.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Identifiants des comptes ouverts par la session.
    #[serde(default)]
    pub accounts_data: Vec<AccountDataDto>,
    #[serde(default)]
    pub aspsp: Option<AspspRefOwned>,
    #[serde(default)]
    pub access: Option<AccessDto>,
}

#[derive(Debug, Deserialize)]
pub struct AspspRefOwned {
    pub name: String,
    pub country: String,
}

#[derive(Debug, Deserialize)]
pub struct AccessDto {
    #[serde(default)]
    pub valid_until: Option<String>,
}

/// Entrée d'`accounts_data` : l'identifiant du compte, et rien d'autre
/// d'exploitable — les empreintes qui l'accompagnent servent à reconnaître un
/// même compte d'une session à l'autre, ce dont on n'a pas l'usage ici.
#[derive(Debug, Deserialize)]
pub struct AccountDataDto {
    pub uid: String,
}

/// Réponse de `GET /accounts/{uid}/details`.
#[derive(Debug, Deserialize)]
pub struct AccountDetailsDto {
    #[serde(default)]
    pub account_id: Option<AccountIdDto>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub cash_account_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AccountIdDto {
    #[serde(default)]
    pub iban: Option<String>,
    #[serde(default)]
    pub other: Option<OtherIdDto>,
}

#[derive(Debug, Deserialize)]
pub struct OtherIdDto {
    #[serde(default)]
    pub identification: Option<String>,
}

// ---------------------------------------------------------------------------
// Soldes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BalancesEnvelope {
    #[serde(default)]
    pub balances: Vec<BalanceDto>,
}

#[derive(Debug, Deserialize)]
pub struct BalanceDto {
    pub balance_amount: AmountDto,
    /// Code Berlin Group : `CLBD` (comptable), `XPCD` (prévu), `ITAV`
    /// (disponible)...
    #[serde(default)]
    pub balance_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reference_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AmountDto {
    /// Montant décimal transporté en chaîne, pour ne rien perdre en précision.
    /// Toujours positif : le sens est porté par `credit_debit_indicator`.
    pub amount: String,
    #[serde(default)]
    pub currency: Option<String>,
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TransactionsEnvelope {
    #[serde(default)]
    pub transactions: Vec<TransactionDto>,
    /// Jeton de pagination. Présent tant qu'il reste des pages à lire.
    #[serde(default)]
    pub continuation_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionDto {
    /// Référence de l'écriture chez la banque.
    #[serde(default)]
    pub entry_reference: Option<String>,
    #[serde(default)]
    pub transaction_id: Option<String>,
    pub transaction_amount: AmountDto,
    /// `CRDT` pour un crédit, `DBIT` pour un débit.
    #[serde(default)]
    pub credit_debit_indicator: Option<String>,
    /// `BOOK` pour une opération comptabilisée, `PDNG` pour une opération en
    /// attente.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub booking_date: Option<String>,
    #[serde(default)]
    pub value_date: Option<String>,
    #[serde(default)]
    pub transaction_date: Option<String>,
    /// Libellé libre, éclaté en plusieurs lignes par certaines banques.
    #[serde(default)]
    pub remittance_information: Vec<String>,
    #[serde(default)]
    pub creditor: Option<PartyDto>,
    #[serde(default)]
    pub debtor: Option<PartyDto>,
    #[serde(default)]
    pub bank_transaction_code: Option<BankTransactionCodeDto>,
}

#[derive(Debug, Deserialize)]
pub struct PartyDto {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BankTransactionCodeDto {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub sub_code: Option<String>,
}

impl TransactionDto {
    /// Libellé le plus informatif que l'on puisse tirer de l'opération.
    pub fn description(&self) -> String {
        let joined = collapse_whitespace(&self.remittance_information.join(" "));
        if !joined.is_empty() {
            return joined;
        }
        if let Some(name) = self.counterparty() {
            return name;
        }
        if let Some(code) = &self.bank_transaction_code
            && let Some(description) = &code.description
            && !description.trim().is_empty()
        {
            return description.trim().to_string();
        }
        "(sans libellé)".to_string()
    }

    /// Nom de la contrepartie : le créancier pour un débit, le débiteur pour un
    /// crédit. Une seule des deux clés est renseignée selon le sens.
    pub fn counterparty(&self) -> Option<String> {
        self.creditor
            .as_ref()
            .and_then(|p| p.name.clone())
            .or_else(|| self.debtor.as_ref().and_then(|p| p.name.clone()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Code métier, sous la forme `code/sub_code` quand les deux existent.
    pub fn category(&self) -> Option<String> {
        let code = self.bank_transaction_code.as_ref()?;
        match (&code.code, &code.sub_code) {
            (Some(c), Some(s)) => Some(format!("{c}/{s}")),
            (Some(c), None) => Some(c.clone()),
            _ => code.description.clone(),
        }
    }

    /// Vrai si l'opération est définitivement comptabilisée.
    pub fn is_booked(&self) -> bool {
        // Une banque qui ne renseigne pas le statut ne renvoie en général que
        // des opérations comptabilisées.
        self.status.as_deref().is_none_or(|s| s == "BOOK")
    }

    /// Identifiant stable pour la déduplication.
    ///
    /// Toutes les banques ne fournissent ni `entry_reference` ni
    /// `transaction_id`. En dernier recours on fabrique une clé à partir du
    /// contenu : deux opérations réellement identiques le même jour
    /// fusionneront, ce qui est préférable à un doublon à chaque
    /// synchronisation.
    pub fn stable_id(&self) -> String {
        for candidate in [&self.entry_reference, &self.transaction_id] {
            if let Some(id) = candidate
                && !id.trim().is_empty()
            {
                return id.trim().to_string();
            }
        }
        let date = self
            .booking_date
            .as_deref()
            .or(self.value_date.as_deref())
            .or(self.transaction_date.as_deref())
            .unwrap_or("");
        format!(
            "synth:{}:{}:{}:{}",
            date,
            self.credit_debit_indicator.as_deref().unwrap_or("?"),
            self.transaction_amount.amount,
            self.description()
        )
    }
}

/// Ramène un libellé bancaire sur une seule ligne.
///
/// Les banques mettent en forme leurs libellés pour un relevé papier : LCL
/// renvoie par exemple `"CARTE\n0602685\nCB  POINT P 1233\nCBLM NOAH NYOUNAE"`.
/// Recopiés tels quels, ces retours à la ligne disloquent l'affichage en
/// tableau. On réduit donc toute suite d'espaces, tabulations et sauts de ligne
/// à une espace unique.
fn collapse_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------------

/// Corps d'erreur renvoyé par l'API. Les champs varient selon l'endpoint, on
/// prend ce qui vient.
#[derive(Debug, Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ApiError {
    pub fn message(&self) -> Option<String> {
        self.message
            .clone()
            .or_else(|| self.detail.clone())
            .or_else(|| self.error.clone())
            .filter(|s| !s.trim().is_empty())
    }
}
