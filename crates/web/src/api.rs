//! API JSON, en lecture seule sur la base locale.
//!
//! Aucune route ne déclenche d'appel bancaire : la synchronisation reste au
//! CLI. Le serveur ne peut donc ni consommer le quota DSP2, ni dépendre de la
//! disponibilité de la banque.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use ecofin_core::analysis::{self, CapitalCurve, PeriodSummary, Range};
use ecofin_core::model::{Account, Transaction};

use crate::access::Authenticated;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Erreur d'API, rendue en JSON pour que le front n'ait pas à deviner.
#[derive(Debug)]
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let ApiError(status, message) = self;
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!("{err:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Un compte, tel que l'interface a besoin de le présenter.
#[derive(Serialize)]
pub struct AccountView {
    pub id: String,
    pub institution: String,
    pub name: String,
    pub iban: Option<String>,
    pub currency: String,
    pub balance: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Étendue des opérations connues, qui borne utilement les sélecteurs de
    /// dates de l'interface.
    pub first_transaction: Option<chrono::NaiveDate>,
    pub last_transaction: Option<chrono::NaiveDate>,
}

pub async fn accounts(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
) -> ApiResult<Vec<AccountView>> {
    let scope = user.scope();
    let views = state.with_store(|store| {
        store
            .accounts(scope)?
            .into_iter()
            .map(|account: Account| {
                let balance = store.primary_balance(scope, &account.id)?;
                let span = store.transaction_span(scope, &account.id)?;
                Ok(AccountView {
                    name: account.display_name(),
                    institution: account.institution_name,
                    iban: account.iban,
                    currency: account.currency,
                    // Les montants voyagent en chaîne : un flottant JSON
                    // perdrait des centimes sur de gros soldes.
                    balance: balance.map(|b| b.amount.to_string()),
                    last_synced_at: account.last_synced_at,
                    first_transaction: span.map(|(first, _)| first),
                    last_transaction: span.map(|(_, last)| last),
                    id: account.id,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })?;
    Ok(Json(views))
}

/// Bornes de la période demandée, en RFC 3339.
#[derive(Deserialize)]
pub struct CapitalQuery {
    pub account: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Courbe de capital et totaux de la période, en une réponse.
///
/// Les deux sont toujours affichés ensemble : les demander séparément
/// doublerait les allers-retours et risquerait de les désynchroniser.
#[derive(Serialize)]
pub struct CapitalResponse {
    #[serde(flatten)]
    pub curve: CapitalCurve,
    pub summary: PeriodSummary,
}

pub async fn capital(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Query(query): Query<CapitalQuery>,
) -> ApiResult<CapitalResponse> {
    let scope = user.scope();
    let range = Range {
        from: parse_bound(query.from.as_deref())?,
        to: parse_bound(query.to.as_deref())?,
    };

    if let (Some(from), Some(to)) = (range.from, range.to)
        && from > to
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "la date de début est postérieure à la date de fin".into(),
        ));
    }

    // Un compte hors périmètre est traité comme inexistant : un 404 plutôt
    // qu'un 500, et surtout le même code que pour un identifiant fantaisiste,
    // ce qui ne révèle pas qu'il appartient à quelqu'un d'autre.
    let known = state
        .with_store(|store| Ok(store.accounts(scope)?.iter().any(|a| a.id == query.account)))?;
    if !known {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("compte « {} » inconnu", query.account),
        ));
    }

    let overrides = state.rules_for(user.id)?;
    let redirects = state.with_preferences(|prefs| prefs.redirects(user.id))?;
    let response = state.with_store(|store| {
        Ok(CapitalResponse {
            curve: analysis::capital_curve(
                store,
                scope,
                &query.account,
                range,
                &overrides,
                &redirects,
            )?,
            summary: analysis::summarize(store, scope, &query.account, range)?,
        })
    })?;

    Ok(Json(response))
}

/// Abonnements détectés sur un compte.
pub async fn subscriptions(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Query(query): Query<AccountQuery>,
) -> ApiResult<Vec<ecofin_core::subscriptions::Subscription>> {
    let scope = user.scope();
    let found = state
        .with_store(|store| ecofin_core::subscriptions::detect(store, scope, &query.account))?;
    Ok(Json(found))
}

#[derive(Deserialize)]
pub struct AccountQuery {
    pub account: String,
}

/// Périodes que l'API ne peut plus rendre, à combler par un import.
pub async fn gaps(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
) -> ApiResult<Vec<ecofin_core::schedule::FogGap>> {
    let scope = user.scope();
    Ok(Json(state.with_store(|store| store.open_gaps(scope))?))
}

/// Détail d'une opération, pour le panneau latéral.
#[derive(Serialize)]
pub struct TransactionView {
    pub id: String,
    pub account_id: String,
    pub amount: String,
    pub currency: String,
    pub booking_date: Option<chrono::NaiveDate>,
    pub value_date: Option<chrono::NaiveDate>,
    /// Libellé nettoyé, tel qu'affiché.
    pub description: String,
    /// Libellé brut de la banque, conservé pour vérification.
    ///
    /// Le nettoyage retire du texte : pouvoir revenir à l'original permet de
    /// constater ce qui a été écarté plutôt que d'avoir à le croire.
    pub raw_description: String,
    pub counterparty: Option<String>,
    pub bank_category: Option<String>,
    pub booked: bool,
    /// Secteur de dépense, tel qu'affiché.
    pub category: String,
    /// Vrai si la catégorie vient d'une règle posée par l'utilisateur.
    pub category_is_manual: bool,
    /// Provenance : l'API bancaire, ou un relevé importé.
    pub source: &'static str,
}

impl TransactionView {
    fn build(
        t: Transaction,
        own_names: &[String],
        overrides: &std::collections::HashMap<String, String>,
        redirects: &std::collections::HashMap<String, String>,
    ) -> Self {
        // Les opérations issues d'un relevé portent un identifiant préfixé,
        // faute de référence d'écriture fournie par la banque.
        let source = match t.id.starts_with("lcl:") || t.id.starts_with("import:") {
            true => "relevé",
            false => "banque",
        };
        let label = ecofin_core::merchant::normalize(&t.description);
        let category =
            ecofin_core::category::classify(&label, !t.is_debit(), own_names, overrides, redirects);

        Self {
            id: t.id,
            account_id: t.account_id,
            amount: t.amount.to_string(),
            currency: t.currency,
            booking_date: t.booking_date,
            value_date: t.value_date,
            // Une règle posée sur ce libellé : l'interface peut alors indiquer
            // que le classement est le fait de l'utilisateur, pas de l'app.
            category_is_manual: overrides.contains_key(&ecofin_core::merchant::fold(&label)),
            category,
            description: label,
            raw_description: t.description,
            counterparty: t.counterparty,
            bank_category: t.bank_category,
            booked: t.booked,
            source,
        }
    }
}

pub async fn transaction(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(id): Path<String>,
) -> ApiResult<TransactionView> {
    let scope = user.scope();
    // Une opération hors périmètre est indiscernable d'une inexistante : le
    // 404 ne révèle donc pas qu'elle appartient à quelqu'un d'autre.
    let overrides = state.rules_for(user.id)?;
    let redirects = state.with_preferences(|prefs| prefs.redirects(user.id))?;
    let own_names = state.with_store(|store| store.own_account_names())?;
    let found = state.with_store(|store| store.transaction(scope, &id))?;
    match found {
        Some(transaction) => Ok(Json(TransactionView::build(
            transaction,
            &own_names,
            &overrides,
            &redirects,
        ))),
        None => Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("opération « {id} » inconnue"),
        )),
    }
}

/// Lit une borne temporelle en RFC 3339.
fn parse_bound(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw)
        .map(|d| Some(d.with_timezone(&Utc)))
        .map_err(|_| {
            ApiError(
                StatusCode::BAD_REQUEST,
                format!("date « {raw} » illisible, format attendu RFC 3339"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_rfc3339_bound() {
        let parsed = parse_bound(Some("2026-08-01T00:00:00Z")).unwrap();
        assert!(parsed.is_some());
    }

    #[test]
    fn treats_a_blank_bound_as_absent() {
        assert!(parse_bound(Some("   ")).unwrap().is_none());
        assert!(parse_bound(None).unwrap().is_none());
    }

    #[test]
    fn rejects_an_unreadable_bound() {
        assert!(parse_bound(Some("01/08/2026")).is_err());
    }

    #[test]
    fn marks_imported_transactions_as_coming_from_a_statement() {
        let mut t = sample();
        t.id = "lcl:2026-08-01:-12.50:0:ACHAT".into();
        assert_eq!(view(t).source, "relevé");
    }

    #[test]
    fn marks_api_transactions_as_coming_from_the_bank() {
        assert_eq!(view(sample()).source, "banque");
    }

    fn view(t: Transaction) -> TransactionView {
        TransactionView::build(
            t,
            &[],
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        )
    }

    fn sample() -> Transaction {
        Transaction {
            id: "ref-1".into(),
            account_id: "acc".into(),
            amount: rust_decimal::Decimal::ZERO,
            currency: "EUR".into(),
            booking_date: None,
            value_date: None,
            description: "x".into(),
            counterparty: None,
            bank_category: None,
            booked: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Catégories
// ---------------------------------------------------------------------------

/// Une catégorie, telle que l'interface la présente.
#[derive(Serialize)]
pub struct CategoryView {
    /// Nom stable, utilisé dans les échanges. Ne change jamais.
    pub key: String,
    /// Nom affiché, modifiable.
    pub label: String,
    /// Faux pour les mouvements qui ne consomment rien.
    pub is_spending: bool,
    /// Vraie pour les catégories d'origine, que des motifs désignent et qui ne
    /// peuvent donc être retirées que par redirection.
    pub builtin: bool,
    /// Catégorie dont celle-ci relève, quand elle en relève d'une.
    pub parent: Option<String>,
}

/// Catégories de l'utilisateur, les siennes comprises.
pub async fn categories(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
) -> ApiResult<Vec<CategoryView>> {
    let defs = state.with_preferences(|prefs| prefs.categories(user.id))?;
    Ok(Json(
        defs.into_iter()
            .map(|c| CategoryView {
                key: c.key,
                label: c.label,
                is_spending: c.is_spending,
                builtin: c.builtin,
                parent: c.parent,
            })
            .collect(),
    ))
}

/// Création d'une catégorie.
#[derive(Deserialize)]
pub struct NewCategory {
    pub label: String,
    /// Faux pour un mouvement qui ne consomme rien.
    #[serde(default = "yes")]
    pub is_spending: bool,
    /// Catégorie dont la nouvelle relève, le cas échéant.
    #[serde(default)]
    pub parent: Option<String>,
}

fn yes() -> bool {
    true
}

pub async fn add_category(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    axum::Json(body): axum::Json<NewCategory>,
) -> Result<Json<CategoryView>, ApiError> {
    let key = state
        .with_preferences(|prefs| {
            prefs.add_category(
                user.id,
                &body.label,
                body.is_spending,
                body.parent.as_deref(),
            )
        })
        .map_err(|err| ApiError(StatusCode::BAD_REQUEST, format!("{err:#}")))?;

    Ok(Json(CategoryView {
        key,
        label: body.label.trim().to_string(),
        is_spending: body.is_spending,
        builtin: false,
        parent: body.parent,
    }))
}

#[derive(Deserialize)]
pub struct RenameCategory {
    pub label: String,
}

pub async fn rename_category(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(key): Path<String>,
    axum::Json(body): axum::Json<RenameCategory>,
) -> Result<StatusCode, ApiError> {
    state
        .with_preferences(|prefs| prefs.rename_category(user.id, &key, &body.label))
        .map_err(|err| ApiError(StatusCode::BAD_REQUEST, format!("{err:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Rattachement d'une catégorie à une autre.
///
/// `parent` nul ramène la catégorie à la racine ; c'est une valeur légitime,
/// distincte d'une absence, d'où une route à part plutôt qu'un champ optionnel
/// du renommage où les deux seraient indiscernables.
#[derive(Deserialize)]
pub struct MoveCategory {
    pub parent: Option<String>,
}

pub async fn set_parent(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(key): Path<String>,
    axum::Json(body): axum::Json<MoveCategory>,
) -> Result<StatusCode, ApiError> {
    state
        .with_preferences(|prefs| prefs.set_parent(user.id, &key, body.parent.as_deref()))
        .map_err(|err| ApiError(StatusCode::BAD_REQUEST, format!("{err:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Cible de reprise, exigée pour une catégorie d'origine.
#[derive(Deserialize)]
pub struct RemoveCategory {
    /// Catégorie qui reçoit les motifs de celle qu'on retire.
    pub into: Option<String>,
}

pub async fn remove_category(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(key): Path<String>,
    Query(query): Query<RemoveCategory>,
) -> Result<StatusCode, ApiError> {
    state
        .with_preferences(|prefs| prefs.remove_category(user.id, &key, query.into.as_deref()))
        .map_err(|err| ApiError(StatusCode::BAD_REQUEST, format!("{err:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Une règle posée par l'utilisateur.
#[derive(Serialize, Deserialize)]
pub struct RuleView {
    /// Libellé nettoyé du bénéficiaire.
    pub label: String,
    /// Nom stable de la catégorie choisie.
    pub category: String,
}

/// Règles de classement de l'utilisateur.
pub async fn rules(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
) -> ApiResult<Vec<RuleView>> {
    let listed = state.with_preferences(|prefs| prefs.list(user.id))?;
    Ok(Json(
        listed
            .into_iter()
            .map(|rule| RuleView {
                label: rule.label,
                category: rule.category,
            })
            .collect(),
    ))
}

/// Pose ou remplace la règle d'un libellé.
///
/// La règle porte sur le bénéficiaire, pas sur l'opération : classer un achat
/// classe toutes les opérations du même commerçant, passées comme à venir.
pub async fn set_rule(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    axum::Json(rule): axum::Json<RuleView>,
) -> Result<StatusCode, ApiError> {
    // La catégorie est validée contre celles de l'utilisateur, non contre les
    // seules catégories d'origine : c'est ce qui permet de classer dans une
    // catégorie qu'il a lui-même créée.
    let known = state.with_preferences(|prefs| prefs.category_exists(user.id, &rule.category))?;
    if !known {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("catégorie « {} » inconnue", rule.category),
        ));
    }

    state.with_preferences(|prefs| prefs.set(user.id, &rule.label, &rule.category))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Retire une règle, rendant le libellé au classement automatique.
pub async fn delete_rule(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(label): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.with_preferences(|prefs| prefs.remove(user.id, &label))?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Authentification
// ---------------------------------------------------------------------------

/// Identifiants soumis par l'écran de connexion.
#[derive(Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct Session {
    pub email: String,
}

/// Ouvre une session.
///
/// Le message d'échec est le même que l'adresse soit inconnue ou le mot de
/// passe faux : distinguer les deux révélerait quels comptes existent.
pub async fn login(
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    axum::Json(credentials): axum::Json<Credentials>,
) -> Result<(axum_extra::extract::CookieJar, Json<Session>), ApiError> {
    let key = peer.ip().to_string();
    if state.throttle().is_locked(&key) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "trop de tentatives, réessaie dans quelques minutes".into(),
        ));
    }

    let refused = || {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "adresse ou mot de passe incorrect".into(),
        )
    };

    let user = state
        .with_store(|store| store.user_by_email(&credentials.email))?
        .ok_or_else(|| {
            state.throttle().record_failure(&key);
            refused()
        })?;

    if !ecofin_core::auth::verify_password(&credentials.password, &user.password_hash) {
        state.throttle().record_failure(&key);
        return Err(refused());
    }

    let secret = state.session_secret().ok_or_else(|| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "aucun secret de session : lance `ecofin user add` depuis le CLI".into(),
        )
    })?;

    let token = ecofin_core::auth::issue_session(&user, &secret)?;
    state.throttle().record_success(&key);

    Ok((
        jar.add(crate::access::session_cookie(token, state.secure_cookies())),
        Json(Session { email: user.email }),
    ))
}

/// Ferme la session en vidant le cookie.
pub async fn logout(
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
) -> (axum_extra::extract::CookieJar, StatusCode) {
    (
        jar.add(crate::access::expired_cookie(state.secure_cookies())),
        StatusCode::NO_CONTENT,
    )
}

/// Compte associé à la session en cours.
///
/// La barrière ayant déjà validé la session, atteindre ce point suffit à
/// prouver qu'elle est bonne ; l'adresse est relue pour l'afficher.
pub async fn me(axum::Extension(user): axum::Extension<Authenticated>) -> Json<Session> {
    // La barrière a déjà validé la session : atteindre ce point suffit.
    Json(Session { email: user.email })
}

// --- Portefeuilles virtuels ------------------------------------------------

/// Un portefeuille, tel que l'écran l'affiche.
#[derive(Serialize)]
pub struct WalletView {
    pub id: i64,
    pub name: String,
    /// Dotation mensuelle, en texte pour que les décimales traversent intactes.
    pub allocation: String,
    /// Catégories désignées, avant extension à leurs sous-catégories.
    pub categories: Vec<String>,
    pub carry_to: Option<i64>,
    /// Enveloppe qui contient celle-ci, et sur la dotation de laquelle elle
    /// est prise.
    pub parent: Option<i64>,
    /// Ce qui finance réellement l'enveloppe : sa dotation moins ce qu'elle
    /// confie à ses filles. Négatif si elles réclament plus qu'elle n'a.
    pub effective_allocation: String,
    /// Somme confiée aux filles directes.
    pub children_allocation: String,
    /// Ce qui reste sur toute la branche, filles comprises.
    pub branch_balance: String,
    /// Vrai quand les filles réclament plus que la mère ne dispose.
    pub overallocated: bool,
    pub start: String,
    /// Reliquat reçu du mois précédent.
    pub carried_in: String,
    /// Apports ponctuels versés ce mois-ci, hors dotation.
    pub contributions: String,
    /// Le détail de ces apports, pour pouvoir en annuler un.
    pub contribution_entries: Vec<ContributionView>,
    /// Somme signée des mouvements du mois : négative quand on a dépensé.
    pub movements: String,
    /// Ce dont l'enveloppe disposait avant toute dépense.
    pub available: String,
    /// Ce qu'il reste.
    pub balance: String,
}

/// Un apport ponctuel versé à une enveloppe.
#[derive(Serialize)]
pub struct ContributionView {
    pub id: i64,
    pub amount: String,
    pub note: Option<String>,
}

/// L'état des enveloppes sur un mois.
#[derive(Serialize)]
pub struct WalletsResponse {
    pub month: String,
    pub wallets: Vec<WalletView>,
    /// Solde du compte, dont les enveloppes sont censées rendre compte.
    pub account_balance: String,
    /// Ce que les enveloppes ne couvrent pas.
    ///
    /// C'est ce qui rend vraie la promesse « la somme des portefeuilles vaut
    /// le solde du compte » : sans cette part, la somme des enveloppes n'a
    /// aucune raison de tomber juste.
    pub unallocated: String,
}

#[derive(Deserialize)]
pub struct WalletsQuery {
    pub account: String,
    /// Mois observé, écrit `AAAA-MM`. Le mois courant par défaut.
    pub month: Option<String>,
}

/// État des enveloppes sur un mois, dotations et reliquats compris.
pub async fn wallets(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Query(query): Query<WalletsQuery>,
) -> ApiResult<WalletsResponse> {
    use ecofin_core::wallets::{Month, Movement, Wallet};

    let scope = user.scope();
    let month = match query.month.as_deref() {
        Some(text) => Month::parse(text).ok_or_else(|| {
            ApiError(
                StatusCode::BAD_REQUEST,
                format!("mois « {text} » illisible, attendu AAAA-MM"),
            )
        })?,
        None => Month::of(chrono::Utc::now().date_naive()),
    };

    let defs = state.with_preferences(|prefs| prefs.wallets(user.id))?;

    // Les catégories désignées sont étendues à leurs sous-catégories : une
    // enveloppe « Transport » doit compter « Essence » et « Péage », sans quoi
    // classer plus finement ferait sortir des dépenses de l'enveloppe.
    let mut engine = Vec::with_capacity(defs.len());
    for def in &defs {
        let categories =
            state.with_preferences(|prefs| prefs.branch_of(user.id, &def.categories))?;
        engine.push(Wallet {
            id: def.id,
            name: def.name.clone(),
            allocation: def.allocation,
            categories,
            carry_to: def.carry_to,
            parent: def.parent,
            start: def.start,
        });
    }

    let overrides = state.rules_for(user.id)?;
    let redirects = state.with_preferences(|prefs| prefs.redirects(user.id))?;
    let given = state.with_preferences(|prefs| prefs.contributions(user.id))?;
    let contributions: Vec<ecofin_core::wallets::Contribution> = given
        .iter()
        .map(|c| ecofin_core::wallets::Contribution {
            wallet_id: c.wallet_id,
            month: c.month,
            amount: c.amount,
        })
        .collect();

    let (movements, account_balance) = state.with_store(|store| {
        let own_names = store.own_account_names()?;
        let transactions =
            store.transactions(scope, Some(&query.account), None, None, usize::MAX)?;
        let movements: Vec<Movement> = transactions
            .iter()
            .filter_map(|t| {
                let date = t.effective_date()?;
                Some(Movement {
                    month: Month::of(date),
                    category: ecofin_core::category::classify(
                        &ecofin_core::merchant::normalize(&t.description),
                        !t.is_debit(),
                        &own_names,
                        &overrides,
                        &redirects,
                    ),
                    amount: t.amount,
                })
            })
            .collect();
        let balance = store
            .primary_balance(scope, &query.account)?
            .map(|b| b.amount)
            .unwrap_or(rust_decimal::Decimal::ZERO);
        Ok((movements, balance))
    })?;

    let states = ecofin_core::wallets::project(&engine, &movements, &contributions, month);
    let current = ecofin_core::wallets::states_for(&states, month);

    let views: Vec<WalletView> = defs
        .iter()
        .filter_map(|def| {
            let state = current.iter().find(|s| s.wallet_id == def.id)?;
            let handed = ecofin_core::wallets::handed_to_children(&engine, def.id);
            let branch = ecofin_core::wallets::branch(&engine, def.id);
            let branch_balance: rust_decimal::Decimal = current
                .iter()
                .filter(|s| branch.contains(&s.wallet_id))
                .map(|s| s.balance)
                .sum();
            Some(WalletView {
                id: def.id,
                name: def.name.clone(),
                allocation: def.allocation.to_string(),
                categories: def.categories.clone(),
                carry_to: def.carry_to,
                parent: def.parent,
                effective_allocation: state.allocation.to_string(),
                children_allocation: handed.to_string(),
                branch_balance: branch_balance.to_string(),
                overallocated: state.allocation < rust_decimal::Decimal::ZERO,
                start: def.start.to_string(),
                carried_in: state.carried_in.to_string(),
                contributions: state.contributions.to_string(),
                contribution_entries: given
                    .iter()
                    .filter(|c| c.wallet_id == def.id && c.month == month)
                    .map(|c| ContributionView {
                        id: c.id,
                        amount: c.amount.to_string(),
                        note: c.note.clone(),
                    })
                    .collect(),
                movements: state.movements.to_string(),
                available: state.available().to_string(),
                balance: state.balance.to_string(),
            })
        })
        .collect();

    let held: rust_decimal::Decimal = current.iter().map(|s| s.balance).sum();

    Ok(Json(WalletsResponse {
        month: month.to_string(),
        wallets: views,
        account_balance: account_balance.to_string(),
        unallocated: (account_balance - held).to_string(),
    }))
}

/// Création d'un portefeuille.
#[derive(Deserialize)]
pub struct NewWallet {
    pub name: String,
    pub allocation: String,
    #[serde(default)]
    pub categories: Vec<String>,
    /// Enveloppe dans laquelle ranger la nouvelle, le cas échéant.
    #[serde(default)]
    pub parent: Option<i64>,
}

pub async fn add_wallet(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    axum::Json(body): axum::Json<NewWallet>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let allocation = parse_amount(&body.allocation)?;
    // Doté à partir du mois en cours : le doter rétroactivement inventerait
    // des reliquats qui n'ont jamais existé.
    let start = ecofin_core::wallets::Month::of(chrono::Utc::now().date_naive());

    let id = state
        .with_preferences(|prefs| prefs.add_wallet(user.id, &body.name, allocation, start))
        .map_err(bad_request)?;
    state
        .with_preferences(|prefs| prefs.set_wallet_categories(user.id, id, &body.categories))
        .map_err(bad_request)?;
    if body.parent.is_some() {
        state
            .with_preferences(|prefs| prefs.set_wallet_parent(user.id, id, body.parent))
            .map_err(bad_request)?;
    }

    Ok(Json(serde_json::json!({ "id": id })))
}

/// Modification d'un portefeuille.
#[derive(Deserialize)]
pub struct EditWallet {
    pub name: String,
    pub allocation: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub carry_to: Option<i64>,
    /// Enveloppe contenante. Nul ramène au premier rang.
    #[serde(default)]
    pub parent: Option<i64>,
}

pub async fn update_wallet(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(id): Path<i64>,
    axum::Json(body): axum::Json<EditWallet>,
) -> Result<StatusCode, ApiError> {
    let allocation = parse_amount(&body.allocation)?;
    state
        .with_preferences(|prefs| {
            prefs.update_wallet(user.id, id, &body.name, allocation, body.carry_to)
        })
        .map_err(bad_request)?;
    state
        .with_preferences(|prefs| prefs.set_wallet_categories(user.id, id, &body.categories))
        .map_err(bad_request)?;
    state
        .with_preferences(|prefs| prefs.set_wallet_parent(user.id, id, body.parent))
        .map_err(bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_wallet(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .with_preferences(|prefs| prefs.remove_wallet(user.id, id))
        .map_err(bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Lit un montant, en refusant ce qui n'en est pas un.
///
/// Accepter une virgule décimale évite de renvoyer l'utilisateur français à sa
/// disposition de clavier pour une raison qui ne le regarde pas.
fn parse_amount(raw: &str) -> Result<rust_decimal::Decimal, ApiError> {
    use std::str::FromStr;
    rust_decimal::Decimal::from_str(raw.trim().replace(',', ".").as_str()).map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            format!("montant « {raw} » illisible"),
        )
    })
}

fn bad_request(err: anyhow::Error) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, format!("{err:#}"))
}

/// Versement d'un apport ponctuel.
#[derive(Deserialize)]
pub struct NewContribution {
    pub amount: String,
    /// Mois auquel le rattacher, écrit `AAAA-MM`. Le mois courant par défaut.
    #[serde(default)]
    pub month: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Verse un apport ponctuel à une enveloppe.
///
/// Distinct de la dotation, qui est une consigne permanente : celui-ci ne vaut
/// que pour son mois, et ne se prend pas sur le budget d'une enveloppe mère.
pub async fn add_contribution(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(id): Path<i64>,
    axum::Json(body): axum::Json<NewContribution>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let amount = parse_amount(&body.amount)?;
    let month = match body.month.as_deref() {
        Some(text) => ecofin_core::wallets::Month::parse(text).ok_or_else(|| {
            ApiError(
                StatusCode::BAD_REQUEST,
                format!("mois « {text} » illisible, attendu AAAA-MM"),
            )
        })?,
        None => ecofin_core::wallets::Month::of(chrono::Utc::now().date_naive()),
    };

    let created = state
        .with_preferences(|prefs| {
            prefs.add_contribution(user.id, id, month, amount, body.note.as_deref())
        })
        .map_err(bad_request)?;

    Ok(Json(serde_json::json!({ "id": created })))
}

/// Annule un apport ponctuel.
pub async fn remove_contribution(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<Authenticated>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .with_preferences(|prefs| prefs.remove_contribution(user.id, id))
        .map_err(bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}
