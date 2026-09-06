//! Client HTTP pour l'API Enable Banking.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use reqwest::{Client, Method, Response, StatusCode};
use rust_decimal::Decimal;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::str::FromStr;

use super::auth::{self, sign_jwt};
use super::types::*;
use crate::config::EnableBankingConfig;
use crate::model::{Account, Balance, Institution, Link, LinkStatus, Transaction};

const API_BASE: &str = "https://api.enablebanking.com";

/// Nombre maximum de pages d'opérations lues en une synchronisation.
///
/// Garde-fou contre une pagination qui ne se terminerait pas ; à 500 opérations
/// par page environ, c'est très au-delà d'un historique personnel courant.
const MAX_TRANSACTION_PAGES: usize = 50;

pub struct EnableBankingClient {
    http: Client,
    jwt: String,
    config: EnableBankingConfig,
}

impl EnableBankingClient {
    /// Construit un client et signe un JWT à partir de la clé privée locale.
    ///
    /// Aucun appel réseau : la signature est purement locale, contrairement aux
    /// API à jeton d'accès.
    pub fn new(config: &EnableBankingConfig) -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("ecofin/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("construction du client HTTP")?;

        let key = config.private_key()?;
        let jwt = sign_jwt(&config.application_id, &key)?;

        Ok(Self {
            http,
            jwt,
            config: config.clone(),
        })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = format!("{API_BASE}{path}");
        let mut req = self
            .http
            .request(method, &url)
            .bearer_auth(&self.jwt)
            .header("Accept", "application/json");
        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await.with_context(|| format!("appel à {url}"))?;
        let response = check_status(response, &url).await?;
        let text = response.text().await.context("lecture de la réponse")?;

        serde_json::from_str(&text).with_context(|| {
            format!(
                "réponse inattendue de {url} : {}",
                text.chars().take(400).collect::<String>()
            )
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path, None::<&()>).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.request(Method::POST, path, Some(body)).await
    }

    /// Vérifie que l'application est reconnue et renvoie sa description.
    /// Sert de test de bout en bout pour les identifiants.
    pub async fn application(&self) -> Result<ApplicationDto> {
        self.get("/application").await
    }

    /// Liste les établissements couverts dans un pays.
    pub async fn institutions(&self, country: &str) -> Result<Vec<Institution>> {
        let envelope: AspspsEnvelope = self
            .get(&format!("/aspsps?country={}", country.to_uppercase()))
            .await?;
        Ok(envelope
            .aspsps
            .into_iter()
            .map(|a| Institution {
                name: a.name,
                country: a.country,
                max_consent_validity: a.maximum_consent_validity,
                psu_types: a.psu_types,
                beta: a.beta,
            })
            .collect())
    }

    /// Demande une URL d'autorisation pour un établissement.
    ///
    /// `state` est repris tel quel sur l'URL de retour ; l'appelant s'en sert
    /// pour vérifier la provenance de la réponse.
    pub async fn start_authorization(
        &self,
        institution: &Institution,
        state: &str,
        valid_until: DateTime<Utc>,
        psu_type: &str,
    ) -> Result<String> {
        let response: AuthResponse = self
            .post(
                "/auth",
                &AuthRequest {
                    access: AccessSpec {
                        // L'API attend une échéance en RFC 3339 avec
                        // millisecondes.
                        valid_until: valid_until
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    },
                    aspsp: AspspRef {
                        name: &institution.name,
                        country: &institution.country,
                    },
                    state,
                    redirect_url: &self.config.redirect_url,
                    psu_type,
                    language: "fr",
                },
            )
            .await
            .context("demande d'autorisation")?;
        Ok(response.url)
    }

    /// Échange le code d'autorisation contre une session utilisable, et renvoie
    /// la session avec les comptes qu'elle ouvre.
    pub async fn create_session(&self, code: &str) -> Result<(Link, Vec<Account>)> {
        let session: SessionResponse = self
            .post("/sessions", &SessionRequest { code })
            .await
            .context("création de la session")?;

        let session_id = session
            .session_id
            .clone()
            .context("l'API n'a pas renvoyé d'identifiant de session")?;

        self.build_session(session_id, session).await
    }

    /// Reprend une session déjà autorisée à partir de son identifiant.
    ///
    /// Le consentement vit côté Enable Banking : si une session a été créée
    /// mais n'a pas pu être enregistrée localement, elle reste exploitable et
    /// n'a pas à être redemandée à la banque.
    pub async fn import_session(&self, session_id: &str) -> Result<(Link, Vec<Account>)> {
        let session: SessionResponse = self
            .get(&format!("/sessions/{session_id}"))
            .await
            .with_context(|| format!("lecture de la session {session_id}"))?;

        self.build_session(session_id.to_string(), session).await
    }

    /// Assemble les types de domaine à partir d'une réponse de session.
    ///
    /// Les détails de chaque compte sont lus séparément : `GET /sessions` ne
    /// les fournit pas, et le tableau `accounts` de `POST /sessions` a une
    /// forme différente. Passer par `/accounts/{uid}/details` donne le même
    /// résultat dans les deux cas.
    async fn build_session(
        &self,
        session_id: String,
        session: SessionResponse,
    ) -> Result<(Link, Vec<Account>)> {
        let (institution_name, institution_country) = match &session.aspsp {
            Some(a) => (a.name.clone(), a.country.clone()),
            None => ("(inconnu)".to_string(), "FR".to_string()),
        };

        let link = Link {
            id: session_id.clone(),
            institution_name: institution_name.clone(),
            institution_country: institution_country.clone(),
            status: parse_session_status(session.status.as_deref()),
            valid_until: session
                .access
                .as_ref()
                .and_then(|a| a.valid_until.as_deref())
                .and_then(parse_datetime),
            created_at: Utc::now(),
        };

        let mut accounts = Vec::with_capacity(session.accounts_data.len());
        for data in &session.accounts_data {
            // Un compte dont le détail est indisponible reste utilisable : son
            // identifiant suffit pour lire soldes et opérations.
            let details = self
                .get::<AccountDetailsDto>(&format!("/accounts/{}/details", data.uid))
                .await
                .ok();

            accounts.push(Account {
                id: data.uid.clone(),
                session_id: session_id.clone(),
                institution_name: institution_name.clone(),
                institution_country: institution_country.clone(),
                name: details.as_ref().and_then(|d| {
                    d.product
                        .clone()
                        .or_else(|| d.name.clone())
                        .or_else(|| d.cash_account_type.clone())
                }),
                iban: details.as_ref().and_then(|d| {
                    d.account_id.as_ref().and_then(|id| {
                        id.iban
                            .clone()
                            .or_else(|| id.other.as_ref().and_then(|o| o.identification.clone()))
                    })
                }),
                currency: normalize_currency(details.as_ref().and_then(|d| d.currency.as_deref())),
                last_synced_at: None,
            });
        }

        Ok((link, accounts))
    }

    /// Relit l'état d'une session existante.
    pub async fn session_status(&self, session_id: &str) -> Result<LinkStatus> {
        match self
            .get::<SessionResponse>(&format!("/sessions/{session_id}"))
            .await
        {
            Ok(session) => Ok(parse_session_status(session.status.as_deref())),
            // Une session révoquée ou échue disparaît côté API ; ce n'est pas
            // une panne, c'est une information.
            Err(err) if is_not_found(&err) => Ok(LinkStatus::Expired),
            Err(err) => Err(err),
        }
    }

    /// Soldes d'un compte.
    pub async fn balances(&self, account_id: &str) -> Result<Vec<Balance>> {
        let envelope: BalancesEnvelope = self
            .get(&format!("/accounts/{account_id}/balances"))
            .await?;
        envelope
            .balances
            .into_iter()
            .map(|b| {
                Ok(Balance {
                    account_id: account_id.to_string(),
                    kind: b
                        .balance_type
                        .or(b.name)
                        .unwrap_or_else(|| "unknown".to_string()),
                    amount: parse_amount(&b.balance_amount.amount)?,
                    currency: b
                        .balance_amount
                        .currency
                        .unwrap_or_else(|| "EUR".to_string()),
                    reference_date: b.reference_date.as_deref().and_then(parse_date),
                })
            })
            .collect()
    }

    /// Opérations d'un compte, toutes pages confondues.
    pub async fn transactions(
        &self,
        account_id: &str,
        date_from: Option<NaiveDate>,
    ) -> Result<Vec<Transaction>> {
        let mut all = Vec::new();
        let mut continuation: Option<String> = None;

        for page in 0..MAX_TRANSACTION_PAGES {
            let mut path = format!("/accounts/{account_id}/transactions");
            let mut query = Vec::new();
            if let Some(date) = date_from {
                query.push(format!("date_from={date}"));
            }
            if let Some(key) = &continuation {
                query.push(format!("continuation_key={}", urlencode(key)));
            }
            if !query.is_empty() {
                path.push('?');
                path.push_str(&query.join("&"));
            }

            let envelope: TransactionsEnvelope = self.get(&path).await?;
            let last_page = envelope.continuation_key.is_none();

            for dto in envelope.transactions {
                all.push(convert_transaction(dto, account_id)?);
            }

            if last_page {
                return Ok(all);
            }
            continuation = envelope.continuation_key;

            if page + 1 == MAX_TRANSACTION_PAGES {
                eprintln!(
                    "  attention : pagination interrompue après {MAX_TRANSACTION_PAGES} pages, \
                     l'historique le plus ancien peut manquer."
                );
            }
        }

        Ok(all)
    }

    /// URL de retour déclarée pour cette application.
    pub fn redirect_url(&self) -> &str {
        &self.config.redirect_url
    }

    /// Adresse d'écoute pour intercepter le retour, si le CLI peut le faire.
    ///
    /// Enable Banking refuse les URL de retour en `http://`, donc sans tunnel
    /// c'est `None` et l'utilisateur recopie l'URL. Avec un tunnel déclaré via
    /// `listen`, la capture automatique redevient possible.
    pub fn callback_listen_addr(&self) -> Result<Option<String>> {
        self.config.listen_addr()
    }

    /// Attend le retour du navigateur sur l'adresse d'écoute donnée.
    pub async fn wait_for_callback(&self, addr: &str, state: &str) -> Result<String> {
        let callback = auth::wait_for_callback(addr, state).await?;
        Ok(callback.code)
    }
}

/// Transforme une réponse non-2xx en erreur lisible.
async fn check_status(response: Response, url: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<ApiError>(&body)
        .ok()
        .and_then(|e| e.message())
        .unwrap_or_else(|| body.chars().take(300).collect());

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => bail!(
            "accès refusé par Enable Banking ({status}). Vérifie l'identifiant \
             d'application et la clé privée avec `ecofin auth status`.\n{detail}"
        ),
        StatusCode::NOT_FOUND => bail!("introuvable ({status}) : {detail}"),
        StatusCode::TOO_MANY_REQUESTS => bail!(
            "trop de requêtes ({status}). Attends quelques minutes avant de relancer \
             `ecofin sync`.\n{detail}"
        ),
        _ => bail!("échec de l'appel à {url} ({status}) : {detail}"),
    }
}

/// Reconnaît une erreur « ressource absente » produite par `check_status`.
fn is_not_found(err: &anyhow::Error) -> bool {
    err.to_string().starts_with("introuvable")
}

fn convert_transaction(dto: TransactionDto, account_id: &str) -> Result<Transaction> {
    let description = dto.description();
    let counterparty = dto.counterparty();
    let bank_category = dto.category();
    let booked = dto.is_booked();
    let id = dto.stable_id();

    // L'API renvoie un montant toujours positif ; le sens vient du champ
    // `credit_debit_indicator`. On applique le signe ici pour que le reste de
    // l'application manipule des montants signés.
    let magnitude = parse_amount(&dto.transaction_amount.amount)?.abs();
    let amount = match dto.credit_debit_indicator.as_deref() {
        Some("DBIT") => -magnitude,
        Some("CRDT") => magnitude,
        // Sans indicateur, on fait confiance au signe transmis.
        _ => parse_amount(&dto.transaction_amount.amount)?,
    };

    Ok(Transaction {
        id,
        account_id: account_id.to_string(),
        amount,
        currency: dto
            .transaction_amount
            .currency
            .unwrap_or_else(|| "EUR".to_string()),
        booking_date: dto
            .booking_date
            .as_deref()
            .or(dto.transaction_date.as_deref())
            .and_then(parse_date),
        value_date: dto.value_date.as_deref().and_then(parse_date),
        description,
        counterparty,
        bank_category,
        booked,
    })
}

/// Ramène une devise de compte à quelque chose d'affichable.
///
/// LCL renvoie `XXX`, le code ISO 4217 signifiant « aucune devise », sur un
/// compte pourtant libellé en euros — ses soldes, eux, portent bien `EUR`.
/// La devise du compte n'est de toute façon qu'indicative : montants et soldes
/// utilisent celle que la banque joint à chaque valeur.
fn normalize_currency(raw: Option<&str>) -> String {
    match raw {
        Some(code) if !code.is_empty() && code != "XXX" => code.to_string(),
        _ => "EUR".to_string(),
    }
}

fn parse_amount(raw: &str) -> Result<Decimal> {
    Decimal::from_str(raw.trim())
        .with_context(|| format!("montant illisible renvoyé par la banque : {raw:?}"))
}

/// Les dates arrivent en `YYYY-MM-DD`. Une date absente ou malformée n'est pas
/// bloquante : on préfère garder l'opération sans date que la perdre.
fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

fn parse_datetime(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Échappe les caractères qui ne passent pas tels quels dans une chaîne de
/// requête. Les clés de continuation sont opaques et peuvent en contenir.
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Réduit l'état de session de l'API aux quatre cas qui nous intéressent.
fn parse_session_status(code: Option<&str>) -> LinkStatus {
    match code {
        Some("AUTHORIZED") | Some("ACTIVE") | Some("VALID") => LinkStatus::Linked,
        Some("EXPIRED") | Some("CLOSED") => LinkStatus::Expired,
        Some("REJECTED") | Some("REVOKED") | Some("CANCELLED") => LinkStatus::Rejected,
        Some("PENDING") => LinkStatus::Pending,
        // Une session que l'API vient de nous rendre en échange d'un code est
        // utilisable, même si elle ne porte pas d'état explicite.
        _ => LinkStatus::Linked,
    }
}

/// Échéance de consentement à demander, plafonnée par ce que la banque accepte.
pub fn consent_deadline(institution: &Institution, requested_days: i64) -> DateTime<Utc> {
    let max_days = institution.max_consent_days().unwrap_or(requested_days);
    let days = requested_days.min(max_days).max(1);
    // Une minute de retrait : une échéance calculée au ras de la limite est
    // parfois refusée par la banque le temps que la requête arrive.
    Utc::now() + Duration::days(days) - Duration::minutes(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(indicator: Option<&str>, amount: &str) -> TransactionDto {
        TransactionDto {
            entry_reference: Some("ref-1".into()),
            transaction_id: None,
            transaction_amount: AmountDto {
                amount: amount.into(),
                currency: Some("EUR".into()),
            },
            credit_debit_indicator: indicator.map(String::from),
            status: Some("BOOK".into()),
            booking_date: Some("2026-08-20".into()),
            value_date: None,
            transaction_date: None,
            remittance_information: vec!["CARTE 19/08 MONOPRIX".into()],
            creditor: None,
            debtor: None,
            bank_transaction_code: None,
        }
    }

    #[test]
    fn debit_indicator_makes_amount_negative() {
        let tx = convert_transaction(dto(Some("DBIT"), "42.50"), "acc").unwrap();
        assert_eq!(tx.amount, Decimal::from_str("-42.50").unwrap());
        assert!(tx.is_debit());
    }

    #[test]
    fn credit_indicator_keeps_amount_positive() {
        let tx = convert_transaction(dto(Some("CRDT"), "1200.00"), "acc").unwrap();
        assert_eq!(tx.amount, Decimal::from_str("1200.00").unwrap());
        assert!(!tx.is_debit());
    }

    #[test]
    fn missing_indicator_trusts_transmitted_sign() {
        let tx = convert_transaction(dto(None, "-15.00"), "acc").unwrap();
        assert_eq!(tx.amount, Decimal::from_str("-15.00").unwrap());
    }

    #[test]
    fn consent_deadline_is_capped_by_institution() {
        let institution = Institution {
            name: "Test".into(),
            country: "FR".into(),
            // 10 jours.
            max_consent_validity: Some(10 * 86_400),
            psu_types: vec!["personal".into()],
            beta: false,
        };
        let deadline = consent_deadline(&institution, 90);
        let days = (deadline - Utc::now()).num_days();
        assert_eq!(days, 9, "10 jours moins la minute de retrait");
    }

    /// Cas réel observé chez LCL : le libellé est mis en forme pour un relevé
    /// papier et arrive sur plusieurs lignes.
    #[test]
    fn flattens_multiline_bank_labels() {
        let mut raw = dto(Some("DBIT"), "209.49");
        raw.remittance_information =
            vec!["CARTE\n0602685\nCB  POINT P 1233     26/08/26\nCBLM NOAH NYOUNAE".into()];
        let tx = convert_transaction(raw, "acc").unwrap();

        assert_eq!(
            tx.description,
            "CARTE 0602685 CB POINT P 1233 26/08/26 CBLM NOAH NYOUNAE"
        );
        assert!(!tx.description.contains('\n'));
    }

    /// Les lignes vides intercalées ne doivent pas laisser d'espaces doubles.
    #[test]
    fn drops_blank_label_segments() {
        let mut raw = dto(Some("DBIT"), "5.00");
        raw.remittance_information =
            vec!["COTISATION MENSUELLE CARTE\n\nCOTISATION MENSUELLE CARTE 0268".into()];
        let tx = convert_transaction(raw, "acc").unwrap();

        assert_eq!(
            tx.description,
            "COTISATION MENSUELLE CARTE COTISATION MENSUELLE CARTE 0268"
        );
    }

    #[test]
    fn urlencode_escapes_reserved_characters() {
        assert_eq!(urlencode("a+b/c=d"), "a%2Bb%2Fc%3Dd");
        assert_eq!(urlencode("safe-token_1.0~"), "safe-token_1.0~");
    }

    #[test]
    fn synthesises_id_when_bank_gives_none() {
        let mut raw = dto(Some("DBIT"), "9.99");
        raw.entry_reference = None;
        let tx = convert_transaction(raw, "acc").unwrap();
        assert!(tx.id.starts_with("synth:2026-08-20:DBIT:9.99:"));
    }
}
