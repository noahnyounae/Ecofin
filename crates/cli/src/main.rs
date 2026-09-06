//! ecofin — suivi de comptes bancaires en ligne de commande.

mod cli;
mod ui;

use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate};
use clap::Parser;
use std::io::Write;
use std::path::PathBuf;

use cli::{AuthCommand, Cli, Command, LinkCommand, UserCommand};
use ecofin_core::config::{self, Config, DEFAULT_REDIRECT_URL, EnableBankingConfig};
use ecofin_core::model::{self, LinkStatus};
use ecofin_core::providers::enablebanking::{EnableBankingClient, consent_deadline};
use ecofin_core::store::{Scope, Store};

/// Nombre d'accès non assistés autorisés par compte et par 24 h.
///
/// La DSP2 (RTS sur l'authentification forte) plafonne à quatre par jour les
/// consultations faites sans que l'utilisateur soit présent. Au-delà, la banque
/// est en droit d'exiger une nouvelle authentification forte — ce qui
/// obligerait à refaire tout le parcours de consentement. `sync` consomme un
/// accès pour les soldes et un pour les opérations.
const UNATTENDED_ACCESS_LIMIT: u32 = 4;

/// Durée de vie du cache des établissements.
///
/// La couverture d'un pays évolue de loin en loin : une semaine évite les
/// appels répétés sans risquer de rater durablement une banque nouvellement
/// prise en charge. `ecofin banks --refresh` force la relecture.
const INSTITUTION_CACHE_MAX_AGE: Duration = Duration::weeks(1);

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("\x1b[31mErreur\x1b[0m : {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Auth(cmd) => auth(cmd).await,
        Command::Banks {
            query,
            country,
            refresh,
        } => banks(query, country, refresh).await,
        Command::Link(cmd) => link(cmd).await,
        Command::Accounts => accounts(),
        Command::Sync { account, full } => sync(account, full).await,
        Command::Import {
            file,
            account,
            dry_run,
        } => import(file, account, dry_run),
        Command::Daemon { hours, once } => daemon(hours, once).await,
        Command::Gaps => gaps(),
        Command::User(cmd) => user(cmd),
        Command::Balance { history, account } => match history {
            true => balance_history(account),
            false => balance(),
        },
        Command::Tx {
            account,
            since,
            search,
            limit,
        } => transactions(account, since, search, limit),
    }
}

/// Construit un client à partir de la configuration enregistrée.
fn client() -> Result<EnableBankingClient> {
    let config = Config::load()?;
    EnableBankingClient::new(config.require_enable_banking()?)
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

async fn auth(cmd: AuthCommand) -> Result<()> {
    match cmd {
        AuthCommand::Login {
            application_id,
            private_key,
            redirect_url,
            listen,
        } => login(application_id, private_key, redirect_url, listen).await,
        AuthCommand::Status => auth_status().await,
    }
}

async fn login(
    application_id: Option<String>,
    private_key: Option<String>,
    redirect_url: Option<String>,
    listen: Option<String>,
) -> Result<()> {
    let application_id = match application_id {
        Some(v) => v,
        None => prompt("Identifiant d'application Enable Banking : ")?,
    };
    let key_source = match private_key {
        Some(v) => v,
        None => prompt("Chemin vers la clé privée .pem : ")?,
    };

    if application_id.is_empty() || key_source.is_empty() {
        bail!("identifiant d'application ou chemin de clé vide");
    }

    // L'URL doit correspondre exactement à l'une de celles déclarées dans
    // l'application, sinon la banque refuse la redirection en fin de parcours.
    let redirect_url = match redirect_url {
        Some(v) => v,
        None => {
            println!(
                "\nURL de retour déclarée dans ton application Enable Banking.\n\
                 Elle doit être en https:// — le schéma http:// est refusé."
            );
            let entered = prompt(&format!("URL de retour [{DEFAULT_REDIRECT_URL}] : "))?;
            match entered.is_empty() {
                true => DEFAULT_REDIRECT_URL.to_string(),
                false => entered,
            }
        }
    };

    // Un `~` saisi à la main n'est pas développé par le shell quand la valeur
    // vient d'une invite plutôt que d'un argument.
    let key_source = expand_tilde(&key_source);
    let installed = config::install_private_key(&key_source)?;

    let mut config = Config::load()?;
    config.enable_banking = Some(EnableBankingConfig {
        application_id,
        private_key_path: installed.clone(),
        redirect_url,
        listen,
    });
    config.save()?;

    // On valide immédiatement : mieux vaut échouer ici que plus tard au milieu
    // d'une connexion bancaire.
    let application = EnableBankingClient::new(config.require_enable_banking()?)?
        .application()
        .await
        .context("l'application n'a pas été reconnue par Enable Banking")?;

    println!("\nApplication « {} » reconnue.", application.name);
    println!("Clé privée : {}", installed.display());
    println!("Config     : {}", config::config_path()?.display());

    check_redirect_registration(&config, &application.redirect_urls);

    println!("\nÉtape suivante : ecofin banks pour trouver ta banque");
    Ok(())
}

async fn auth_status() -> Result<()> {
    let config = Config::load()?;
    let credentials = config.require_enable_banking()?;
    println!("Application  : {}", credentials.application_id);
    println!("Clé privée   : {}", credentials.private_key_path.display());
    println!("URL de retour: {}", credentials.redirect_url);

    let client = EnableBankingClient::new(credentials)?;
    let application = client.application().await?;

    println!("\nNom          : {}", application.name);
    if let Some(environment) = &application.environment {
        println!("Environnement: {environment}");
    }
    match application.active {
        Some(false) => println!("État         : inactive — active-la dans le portail"),
        _ => println!("État         : active"),
    }

    check_redirect_registration(&config, &application.redirect_urls);

    let institutions = client.institutions("FR").await?;
    println!(
        "\nCouverture   : {} établissements français",
        institutions.len()
    );
    Store::open()?.cache_institutions("FR", &institutions)?;
    Ok(())
}

/// Prévient si l'URL de retour configurée n'est pas déclarée côté Enable
/// Banking : l'autorisation échouerait au dernier moment, après que
/// l'utilisateur se soit authentifié.
fn check_redirect_registration(config: &Config, registered: &[String]) {
    let Some(credentials) = &config.enable_banking else {
        return;
    };
    if registered.is_empty() || registered.contains(&credentials.redirect_url) {
        return;
    }
    println!(
        "\n\x1b[33mAttention\x1b[0m : {} n'est pas déclarée dans l'application.",
        credentials.redirect_url
    );
    println!("URL déclarées : {}", registered.join(", "));
    println!("Ajoute-la dans le portail, ou relance `ecofin auth login --redirect-url <URL>`.");
}

// ---------------------------------------------------------------------------
// banks
// ---------------------------------------------------------------------------

/// Liste des établissements, du cache local quand il est encore frais.
///
/// La couverture d'un pays ne bouge que de loin en loin : la retélécharger à
/// chaque commande est inutile, et empêche de préparer une connexion hors ligne.
async fn institutions_of(country: &str, refresh: bool) -> Result<Vec<model::Institution>> {
    let mut store = Store::open()?;

    if !refresh
        && let Some(cached) = store.cached_institutions(country, INSTITUTION_CACHE_MAX_AGE)?
    {
        return Ok(cached);
    }

    let institutions = client()?.institutions(country).await?;
    store.cache_institutions(country, &institutions)?;
    Ok(institutions)
}

async fn banks(query: Option<String>, country: String, refresh: bool) -> Result<()> {
    let mut institutions = institutions_of(&country, refresh).await?;

    if let Some(q) = &query {
        let needle = q.to_lowercase();
        institutions.retain(|i| i.name.to_lowercase().contains(&needle));
    }
    institutions.sort_by(|a, b| a.name.cmp(&b.name));

    if institutions.is_empty() {
        println!("Aucun établissement ne correspond.");
        println!("Sans filtre, `ecofin banks --country {country}` liste tout ce qui est couvert.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = institutions
        .iter()
        .map(|i| {
            vec![
                i.name.clone(),
                i.max_consent_days()
                    .map(|d| format!("{d} j"))
                    .unwrap_or_else(|| "—".into()),
                i.psu_types.join(", "),
                if i.beta { "bêta" } else { "" }.to_string(),
            ]
        })
        .collect();

    ui::table(&["Banque", "Consentement", "Profils", ""], rows);
    println!("\nPour connecter : ecofin link new \"<NOM EXACT>\"");
    Ok(())
}

// ---------------------------------------------------------------------------
// link
// ---------------------------------------------------------------------------

async fn link(cmd: LinkCommand) -> Result<()> {
    match cmd {
        LinkCommand::New {
            institution,
            country,
            days,
            psu_type,
            no_open,
            user,
        } => link_new(institution, country, days, psu_type, no_open, user).await,
        LinkCommand::Status { id } => link_status(id).await,
        LinkCommand::List => link_list(),
        LinkCommand::Import { session_id, user } => link_import(session_id, user).await,
        LinkCommand::Check {
            redirect_url,
            listen,
        } => link_check(redirect_url, listen).await,
    }
}

/// Reprend une session déjà autorisée côté Enable Banking.
async fn link_import(session_id: String, user: Option<String>) -> Result<()> {
    let client = client()?;
    let store = Store::open()?;
    let owner = resolve_owner(&store, user.as_deref())?;

    let (link, accounts) = client.import_session(&session_id).await?;
    store.upsert_link(&link, owner)?;

    println!("Session reprise : {}", link.institution_name);
    if let Some(remaining) = link.days_remaining() {
        println!("Consentement valable {remaining} jours.");
    }

    if accounts.is_empty() {
        println!("Aucun compte n'est rattaché à cette session.");
        return Ok(());
    }

    for account in &accounts {
        store.upsert_account(account, owner)?;
        println!("  compte importé : {}", account.display_name());
    }

    println!("\nÉtape suivante : ecofin sync --full");
    Ok(())
}

/// Vérifie que le retour d'autorisation atteint bien cette machine.
///
/// Reproduit exactement ce que fait `link new`, sans appeler l'API ni la
/// banque : mêmes adresse d'écoute et même serveur.
async fn link_check(redirect_url: Option<String>, listen: Option<String>) -> Result<()> {
    // Les options priment sur la configuration, pour pouvoir tester un tunnel
    // avant même d'avoir déclaré l'application Enable Banking.
    let (redirect_url, addr) = match (redirect_url, listen) {
        (Some(url), Some(addr)) => (url, addr),
        (url, addr) => {
            let config = Config::load()?;
            let credentials = config
                .require_enable_banking()
                .context("sans configuration enregistrée, précise --redirect-url et --listen")?;
            let addr = match addr {
                Some(a) => a,
                None => credentials.listen_addr()?.context(
                    "aucune adresse d'écoute : l'URL de retour enregistrée est publique \
                     et aucun --listen n'est configuré, donc `link new` demanderait de \
                     recopier l'URL à la main. Rien à vérifier ici.",
                )?,
            };
            (
                url.unwrap_or_else(|| credentials.redirect_url.clone()),
                addr,
            )
        }
    };

    let state = uuid::Uuid::new_v4().to_string();
    let separator = if redirect_url.contains('?') { '&' } else { '?' };
    let probe = format!("{redirect_url}{separator}code=ecofin-check&state={state}");

    println!("Écoute sur {addr}.");
    println!("\nOuvre cette adresse dans un navigateur, ou depuis un autre terminal :");
    println!("  curl -s '{probe}' > /dev/null");
    println!("\nEn attente…");

    let callback =
        ecofin_core::providers::enablebanking::auth::wait_for_callback(&addr, &state).await?;

    if callback.code != "ecofin-check" {
        bail!(
            "une requête est arrivée mais portait le code « {} » — \
             quelque chose d'autre écoute sur cette adresse",
            callback.code
        );
    }

    println!("\n\x1b[32mOK\x1b[0m — le retour d'autorisation atteint bien ecofin.");
    println!("`ecofin link new` capturera le code automatiquement.");
    Ok(())
}

async fn link_new(
    institution_name: String,
    country: String,
    days: i64,
    psu_type: String,
    no_open: bool,
    user: Option<String>,
) -> Result<()> {
    let client = client()?;
    let store = Store::open()?;
    // Résolu avant tout appel réseau : mieux vaut échouer maintenant qu'après
    // avoir fait passer l'utilisateur par l'authentification de sa banque.
    let owner = resolve_owner(&store, user.as_deref())?;

    // On vérifie que la banque existe avant d'ouvrir quoi que ce soit : l'API
    // exige le nom exact, à la casse près.
    let institutions = institutions_of(&country, false).await?;
    let institution = institutions
        .iter()
        .find(|i| i.name.eq_ignore_ascii_case(&institution_name))
        .with_context(|| {
            let suggestions = suggest(&institutions, &institution_name);
            match suggestions.is_empty() {
                true => format!(
                    "« {institution_name} » n'est pas couverte en {country} — \
                     cherche-la avec `ecofin banks`"
                ),
                false => format!(
                    "« {institution_name} » introuvable. Voulais-tu dire : {} ?",
                    suggestions.join(", ")
                ),
            }
        })?;

    if institution.beta {
        println!(
            "Note : l'intégration {} est en bêta chez Enable Banking, \
             des champs peuvent manquer.",
            institution.name
        );
    }
    if !institution.psu_types.is_empty() && !institution.psu_types.contains(&psu_type) {
        bail!(
            "{} n'accepte pas le profil « {psu_type} » (accepté : {})",
            institution.name,
            institution.psu_types.join(", ")
        );
    }

    let deadline = consent_deadline(institution, days);
    let granted_days = (deadline - chrono::Utc::now()).num_days() + 1;
    if granted_days < days {
        println!(
            "{} plafonne le consentement à {granted_days} jours, demande ajustée.",
            institution.name
        );
    }

    let state = uuid::Uuid::new_v4().to_string();
    let url = client
        .start_authorization(institution, &state, deadline, &psu_type)
        .await?;

    println!("\nAuthentifie-toi auprès de {} ici :", institution.name);
    println!("{url}\n");

    if !no_open && open::that(&url).is_err() {
        println!("(ouverture du navigateur impossible, copie l'URL à la main)");
    }

    let code = if let Some(addr) = client.callback_listen_addr()? {
        // Le serveur local intercepte la redirection et récupère le code.
        println!("En attente du retour de la banque (écoute sur {addr})…");
        client.wait_for_callback(&addr, &state).await?
    } else {
        // Enable Banking refuse les URL de retour en `http://`, donc le CLI ne
        // peut pas écouter la redirection : l'utilisateur la recopie.
        println!("Une fois authentifié, ta banque te redirigera vers :");
        println!("  {}", client.redirect_url());
        println!("\nCopie l'URL complète depuis la barre d'adresse et colle-la ci-dessous.");
        println!(
            "(la page elle-même peut être vide ou afficher une erreur, c'est sans importance)\n"
        );
        let pasted = prompt("URL de retour : ")?;
        ecofin_core::providers::enablebanking::auth::code_from_input(&pasted, &state)?
    };

    let (link, accounts) = client.create_session(&code).await?;

    store.upsert_link(&link, owner)?;
    println!("\nConsentement accordé pour {}.", link.institution_name);
    if let Some(remaining) = link.days_remaining() {
        println!("Valable {remaining} jours.");
    }

    if accounts.is_empty() {
        println!("Aucun compte n'a été partagé — vérifie la sélection faite chez ta banque.");
        return Ok(());
    }

    for account in &accounts {
        store.upsert_account(account, owner)?;
        println!("  compte importé : {}", account.display_name());
    }

    println!("\nÉtape suivante : ecofin sync");
    Ok(())
}

/// Propose les noms qui partagent un mot avec la saisie de l'utilisateur.
fn suggest(institutions: &[model::Institution], needle: &str) -> Vec<String> {
    let words: Vec<String> = needle
        .to_lowercase()
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(String::from)
        .collect();

    institutions
        .iter()
        .filter(|i| {
            let name = i.name.to_lowercase();
            words.iter().any(|w| name.contains(w.as_str()))
        })
        .take(5)
        .map(|i| format!("« {} »", i.name))
        .collect()
}

async fn link_status(id: Option<String>) -> Result<()> {
    let client = client()?;
    let store = Store::open()?;

    let targets = match &id {
        Some(needle) => vec![
            store
                .find_link(Scope::All, needle)?
                .with_context(|| format!("aucune session ne correspond à « {needle} »"))?,
        ],
        None => store.links(Scope::All)?,
    };

    if targets.is_empty() {
        println!("Aucun consentement. Commence par `ecofin banks` puis `ecofin link new`.");
        return Ok(());
    }

    for link in targets {
        let status = client.session_status(&link.id).await?;
        store.set_link_status(&link.id, status)?;

        let remaining = match link.days_remaining() {
            Some(days) if days >= 0 => format!(", {days} jours restants"),
            Some(_) => ", échéance dépassée".to_string(),
            None => String::new(),
        };
        println!("{} — {status}{remaining}", link.institution_name);

        if status != LinkStatus::Linked {
            println!(
                "  reconnecte avec : ecofin link new \"{}\"",
                link.institution_name
            );
        }
    }
    Ok(())
}

fn link_list() -> Result<()> {
    let store = Store::open()?;
    let links = store.links(Scope::All)?;
    if links.is_empty() {
        println!("Aucun consentement. Commence par `ecofin banks` puis `ecofin link new`.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = links
        .iter()
        .map(|l| {
            vec![
                l.institution_name.clone(),
                l.status.to_string(),
                match l.days_remaining() {
                    Some(days) if days >= 0 => format!("{days} j"),
                    Some(_) => "échu".into(),
                    None => "—".into(),
                },
                l.created_at.format("%d/%m/%Y").to_string(),
                l.id.chars().take(8).collect(),
            ]
        })
        .collect();

    ui::table(&["Banque", "État", "Reste", "Créé le", "Session"], rows);
    Ok(())
}

// ---------------------------------------------------------------------------
// accounts
// ---------------------------------------------------------------------------

fn accounts() -> Result<()> {
    let store = Store::open()?;
    let accounts = store.accounts(Scope::All)?;
    if accounts.is_empty() {
        println!("Aucun compte. Connecte une banque avec `ecofin link new \"<NOM>\"`.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = accounts
        .iter()
        .map(|a| {
            vec![
                a.institution_name.clone(),
                a.display_name(),
                a.currency.clone(),
                a.last_synced_at
                    .map(|d| d.format("%d/%m/%Y %H:%M").to_string())
                    .unwrap_or_else(|| "jamais".into()),
            ]
        })
        .collect();

    ui::table(&["Banque", "Compte", "Devise", "Synchronisé"], rows);
    Ok(())
}

// ---------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------

/// Identifiants des opérations encore en attente selon la banque.
///
/// C'est la référence du nettoyage : ce que cette liste ne contient pas et que
/// nous détenons pourtant comme « en attente » a été comptabilisé ailleurs.
fn pending_ids(fetched: &[ecofin_core::model::Transaction]) -> std::collections::HashSet<String> {
    fetched
        .iter()
        .filter(|t| !t.booked)
        .map(|t| t.id.clone())
        .collect()
}

async fn sync(account: Option<String>, full: bool) -> Result<()> {
    let client = client()?;
    let mut store = Store::open()?;

    let targets = match &account {
        Some(needle) => vec![
            store
                .find_account(Scope::All, needle)?
                .with_context(|| format!("aucun compte ne correspond à « {needle} »"))?,
        ],
        None => store.accounts(Scope::All)?,
    };

    if targets.is_empty() {
        println!("Aucun compte à synchroniser. Connecte une banque avec `ecofin link new`.");
        return Ok(());
    }

    // Un consentement échu fait échouer chaque compte de la même banque avec la
    // même erreur ; autant le dire une fois, en amont.
    let expired: Vec<String> = store
        .links(Scope::All)?
        .into_iter()
        .filter(|l| l.status == LinkStatus::Expired || l.days_remaining().is_some_and(|d| d < 0))
        .map(|l| l.institution_name)
        .collect();
    if !expired.is_empty() {
        println!(
            "Consentement expiré pour : {}. Relance `ecofin link new \"<NOM>\"`.\n",
            expired.join(", ")
        );
    }

    let mut failures = 0usize;

    for account in &targets {
        println!("{} — {}", account.institution_name, account.display_name());

        // Dépasser le plafond DSP2 peut déclencher une demande
        // d'authentification forte, donc un parcours de consentement à refaire :
        // mieux vaut prévenir avant l'appel qu'après le refus.
        let recent = store.calls_last_24h(&account.id)? / 2;
        if recent >= UNATTENDED_ACCESS_LIMIT {
            println!(
                "  attention : {recent} synchronisations sur ce compte dans les \
                 dernières 24 h, pour un plafond DSP2 de {UNATTENDED_ACCESS_LIMIT} \
                 accès non assistés. Au-delà, ta banque peut exiger une nouvelle \
                 authentification."
            );
        }

        match client.balances(&account.id).await {
            Ok(balances) => {
                store.record_api_call(&account.id, "balances")?;
                store.record_balances(&balances)?;
                if let Some(primary) = store.primary_balance(Scope::All, &account.id)? {
                    println!("  solde : {}", ui::money(primary.amount, &primary.currency));
                }
            }
            Err(err) => {
                failures += 1;
                println!("  soldes indisponibles : {err:#}");
            }
        }

        // On reprend une semaine avant la dernière opération connue : les
        // opérations en attente sont réécrites lorsqu'elles se comptabilisent,
        // et leur date peut reculer.
        let since = if full {
            None
        } else {
            store
                .latest_transaction_date(&account.id)?
                .map(|d| d - Duration::days(7))
        };

        match client.transactions(&account.id, since).await {
            Ok(fetched) => {
                store.record_api_call(&account.id, "transactions")?;
                let total = fetched.len();
                let new = store.upsert_transactions(&fetched)?;
                // Ce que la banque ne signale plus comme en attente a été
                // comptabilisé sous un autre identifiant : sans ce retrait, la
                // provisoire resterait à côté de la définitive.
                let stale =
                    store.prune_stale_pending(&account.id, since, &pending_ids(&fetched))?;
                store.mark_synced(&account.id)?;
                println!(
                    "  {} reçues, {} {}{}",
                    ui::plural(total, "opération"),
                    new,
                    if new > 1 { "nouvelles" } else { "nouvelle" },
                    ui::settled(stale)
                );
            }
            Err(err) => {
                failures += 1;
                println!("  opérations indisponibles : {err:#}");
            }
        }
        println!();
    }

    if failures > 0 {
        println!(
            "{} en échec — voir les messages ci-dessus.",
            ui::plural(failures, "appel")
        );
    }
    println!("ecofin tx        pour voir les opérations");
    println!("ecofin balance   pour voir les soldes");
    Ok(())
}

// ---------------------------------------------------------------------------
// daemon
// ---------------------------------------------------------------------------

/// Synchronise à heures fixes, en rattrapant ce qui a été manqué.
async fn daemon(hours: Option<Vec<u32>>, once: bool) -> Result<()> {
    use ecofin_core::schedule::{Decision, Schedule};

    let schedule = match hours {
        Some(hours) => Schedule::new(hours),
        None => Schedule::default(),
    };

    println!(
        "{} par jour, soit {} accès quotidiens sur un plafond de {}.",
        ui::plural(schedule.daily_slots(), "passage"),
        // Les soldes ne coûtent qu'un accès par jour, les opérations un par
        // passage.
        schedule.daily_slots() + 1,
        ecofin_core::schedule::DAILY_ACCESS_LIMIT
    );

    loop {
        let last = Store::open()?.last_sync()?;
        let now = chrono::Utc::now();

        if let Decision::SyncNow { missed } = schedule.decide(last, now) {
            if missed > 1 {
                println!(
                    "\n{} manqués depuis la dernière synchronisation : rattrapage immédiat.",
                    ui::plural(missed, "créneau")
                );
            }
            // Une erreur ne doit pas arrêter le démon : la banque peut être
            // momentanément indisponible, et il reprendra au créneau suivant.
            if let Err(err) = run_scheduled_sync().await {
                eprintln!("\n\x1b[31mSynchronisation en échec\x1b[0m : {err:#}");
            }
        }

        if once {
            return Ok(());
        }

        // L'attente suit *toute* tentative, réussie ou non.
        //
        // La calculer à partir de la dernière synchronisation réussie serait
        // fautif : un échec ne l'enregistre pas, la décision resterait
        // « synchroniser », et le démon tournerait en boucle serrée sur la
        // banque jusqu'à épuiser le quota. Le rythme doit tenir au planning,
        // pas au succès.
        let next = schedule.next_slot(chrono::Utc::now());
        let delay = (next - chrono::Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(60));
        println!(
            "\nProchain passage le {}.",
            next.format("%d/%m/%Y à %H:%M UTC")
        );
        tokio::time::sleep(delay).await;
    }
}

/// Un passage automatique : opérations toujours, soldes une fois par jour.
///
/// C'est cette économie qui permet trois passages : trois relevés d'opérations
/// et un seul de soldes tiennent dans les quatre accès autorisés.
async fn run_scheduled_sync() -> Result<()> {
    let mut store = Store::open()?;

    // Le repérage des périodes perdues est un calcul purement local : il doit
    // précéder tout appel réseau. Sinon, une banque injoignable ou des
    // identifiants expirés priveraient l'utilisateur de l'information au
    // moment précis où elle lui serait la plus utile.
    record_fog_gaps(&store)?;

    let client = client()?;

    for account in store.accounts(Scope::All)? {
        println!(
            "\n{} — {}",
            account.institution_name,
            account.display_name()
        );

        // Un seul relevé de soldes par jour suffit : la courbe se reconstruit
        // depuis les opérations et ce point de référence.
        if store.calls_today(&account.id, "balances")? == 0 {
            match client.balances(&account.id).await {
                Ok(balances) => {
                    store.record_api_call(&account.id, "balances")?;
                    store.record_balances(&balances)?;
                    if let Some(primary) = store.primary_balance(Scope::All, &account.id)? {
                        println!("  solde : {}", ui::money(primary.amount, &primary.currency));
                    }
                }
                Err(err) => println!("  soldes indisponibles : {err:#}"),
            }
        }

        let since = store
            .latest_transaction_date(&account.id)?
            .map(|d| d - Duration::days(7));

        match client.transactions(&account.id, since).await {
            Ok(fetched) => {
                store.record_api_call(&account.id, "transactions")?;
                let new = store.upsert_transactions(&fetched)?;
                let stale =
                    store.prune_stale_pending(&account.id, since, &pending_ids(&fetched))?;
                store.mark_synced(&account.id)?;
                println!(
                    "  {} reçues, {} {}{}",
                    ui::plural(fetched.len(), "opération"),
                    new,
                    if new > 1 { "nouvelles" } else { "nouvelle" },
                    ui::settled(stale)
                );
            }
            Err(err) => println!("  opérations indisponibles : {err:#}"),
        }

        let used = store.calls_last_24h(&account.id)?;
        println!(
            "  {} accès utilisés sur 24 h, {} restants",
            used,
            ecofin_core::schedule::remaining_access(used)
        );
    }
    Ok(())
}

/// Consigne les périodes que la fenêtre de la banque a laissées derrière elle.
///
/// Purement local : aucun appel réseau, donc utilisable même API indisponible.
fn record_fog_gaps(store: &Store) -> Result<()> {
    use ecofin_core::schedule::{HISTORY_WINDOW_DAYS, detect_gap};

    let today = chrono::Utc::now().date_naive();
    for account in store.accounts(Scope::All)? {
        let newest = store.latest_transaction_date(&account.id)?;
        let Some(gap) = detect_gap(&account.id, newest, today, HISTORY_WINDOW_DAYS) else {
            continue;
        };
        store.record_gap(&gap)?;
        println!(
            "\n\x1b[33mZone de brouillard\x1b[0m — {} · {}",
            account.institution_name,
            account.display_name()
        );
        println!(
            "  du {} au {} ({}), hors de portée de l'API",
            gap.from.format("%d/%m/%Y"),
            gap.to.format("%d/%m/%Y"),
            ui::plural(gap.days() as usize, "jour")
        );
        println!("  ces opérations figurent sur tes relevés : ecofin import <dossier>");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// gaps
// ---------------------------------------------------------------------------

fn gaps() -> Result<()> {
    let store = Store::open()?;
    let open = store.open_gaps(Scope::All)?;

    if open.is_empty() {
        println!("Aucune période manquante. L'historique local rejoint la fenêtre de la banque.");
        return Ok(());
    }

    println!(
        "{} que l'API ne peut plus rendre :\n",
        ui::plural(open.len(), "période")
    );
    let rows: Vec<Vec<String>> = open
        .iter()
        .map(|gap| {
            vec![
                gap.from.format("%d/%m/%Y").to_string(),
                gap.to.format("%d/%m/%Y").to_string(),
                ui::plural(gap.days() as usize, "jour"),
            ]
        })
        .collect();
    ui::table(&["Du", "Au", "Durée"], rows);
    println!("\nCes opérations figurent sur tes relevés mensuels :");
    println!("  ecofin import <dossier-de-relevés>");
    Ok(())
}

// ---------------------------------------------------------------------------
// user
// ---------------------------------------------------------------------------

fn user(cmd: UserCommand) -> Result<()> {
    let store = Store::open()?;

    match cmd {
        UserCommand::Add { email } => {
            let password = read_new_password()?;
            let hash = ecofin_core::auth::hash_password(&password)?;
            let created = store.create_user(&email, &hash)?;

            // Sans secret, aucune session ne peut être signée : on le crée ici
            // puisque le serveur web, en lecture seule, en est incapable.
            store.ensure_session_secret()?;

            println!("Compte créé : {}", created.email);

            // Les données antérieures au cloisonnement n'ont pas de
            // propriétaire, et resteraient invisibles depuis le web. Le premier
            // compte créé est le seul candidat possible : on les lui attribue.
            let orphans = store.orphan_count()?;
            if orphans > 0 && store.users()?.len() == 1 {
                store.claim_orphans(created.id)?;
                println!(
                    "{} rattachés à ce compte (connexions bancaires et comptes \n\
                     antérieurs au cloisonnement).",
                    ui::plural(orphans, "élément")
                );
            } else if orphans > 0 {
                println!(
                    "\n\x1b[33mAttention\x1b[0m : {} sans propriétaire restent invisibles \n\
                     depuis le web. Rattache-les en recréant la connexion bancaire.",
                    ui::plural(orphans, "élément")
                );
            }
            println!("\nDémarre l'application avec : docker compose up -d web");
            Ok(())
        }

        UserCommand::List => {
            let users = store.users()?;
            if users.is_empty() {
                println!("Aucun compte. Crée-en un avec `ecofin user add <adresse>`.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = users
                .iter()
                .map(|u| {
                    vec![
                        u.email.clone(),
                        u.created_at.format("%d/%m/%Y").to_string(),
                        u.token_version.to_string(),
                    ]
                })
                .collect();
            ui::table(&["Adresse", "Créé le", "Version des jetons"], rows);
            Ok(())
        }

        UserCommand::Passwd { email } => {
            let password = read_new_password()?;
            let hash = ecofin_core::auth::hash_password(&password)?;
            store.set_password(&email, &hash)?;
            println!("Mot de passe changé. Les sessions ouvertes sont fermées.");
            Ok(())
        }

        UserCommand::Revoke { email } => {
            store.revoke_sessions(&email)?;
            println!("Sessions de {email} fermées.");
            Ok(())
        }

        UserCommand::Remove { email } => {
            store.delete_user(&email)?;
            println!("Compte {email} supprimé.");
            Ok(())
        }
    }
}

/// Détermine à quel compte de connexion rattacher des données bancaires.
///
/// Sans précision, le compte unique s'impose de lui-même. Dès qu'il y en a
/// plusieurs, il faut choisir : deviner reviendrait à risquer d'exposer les
/// comptes bancaires de l'un à l'autre.
fn resolve_owner(store: &Store, requested: Option<&str>) -> Result<i64> {
    if let Some(email) = requested {
        return store
            .user_by_email(email)?
            .map(|u| u.id)
            .with_context(|| format!("aucun compte de connexion pour « {email} »"));
    }

    let users = store.users()?;
    match users.len() {
        0 => bail!(
            "aucun compte de connexion : crée-en un avec `ecofin user add <adresse>`\n\
             avant de connecter une banque"
        ),
        1 => Ok(users[0].id),
        n => bail!(
            "{n} comptes de connexion : précise le propriétaire avec --user\n{}",
            users
                .iter()
                .map(|u| format!("  {}", u.email))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// Compare deux IBAN sans se soucier des espaces ni de la casse.
///
/// Les relevés les impriment par groupes de quatre, la banque les transmet d'un
/// seul tenant : comparés tels quels, deux IBAN identiques sembleraient
/// différer.
fn same_iban(left: &str, right: &str) -> bool {
    let normalize = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_uppercase()
    };
    normalize(left) == normalize(right)
}

/// Lit un secret, masqué quand un terminal le permet.
///
/// `rpassword` lit le terminal directement, ce qui empêche le mot de passe
/// d'apparaître à l'écran mais rend l'appel impossible sans TTY — dans un
/// script, ou un conteneur lancé sans `-t`. On se rabat alors sur l'entrée
/// standard, en clair : c'est le prix d'un usage non interactif, et l'appelant
/// choisit en connaissance de cause.
fn read_secret(prompt: &str) -> Result<String> {
    use std::io::IsTerminal;

    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password(prompt).context("lecture du mot de passe");
    }

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("lecture du mot de passe sur l'entrée standard")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Demande deux fois le mot de passe, sans écho à l'écran quand c'est possible.
///
/// La double saisie évite d'enregistrer une faute de frappe dans un mot de
/// passe qu'on ne peut plus relire.
fn read_new_password() -> Result<String> {
    let first = read_secret("Mot de passe : ")?;
    if first.chars().count() < ecofin_core::auth::MIN_PASSWORD_LENGTH {
        bail!(
            "le mot de passe doit faire au moins {} caractères",
            ecofin_core::auth::MIN_PASSWORD_LENGTH
        );
    }
    let second = read_secret("Confirmation : ")?;
    if first != second {
        bail!("les deux saisies diffèrent");
    }
    Ok(first)
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

fn import(file: PathBuf, account: Option<String>, dry_run: bool) -> Result<()> {
    let mut store = Store::open()?;

    // Sans compte précisé, l'ambiguïté n'existe que s'il y en a plusieurs.
    let account = match &account {
        Some(needle) => store
            .find_account(Scope::All, needle)?
            .with_context(|| format!("aucun compte ne correspond à « {needle} »"))?,
        None => {
            let accounts = store.accounts(Scope::All)?;
            match accounts.len() {
                0 => bail!("aucun compte en base — connecte une banque, ou précise --account"),
                1 => accounts.into_iter().next().expect("longueur vérifiée"),
                n => bail!(
                    "{n} comptes en base : précise lequel avec --account\n{}",
                    accounts
                        .iter()
                        .map(|a| format!("  {} — {}", a.institution_name, a.display_name()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }
        }
    };

    let files = ecofin_core::importer::collect_files(&file)?;
    println!("{} — {}", account.institution_name, account.display_name());
    println!("{} à lire\n", ui::plural(files.len(), "fichier"));

    let mut transactions = Vec::new();
    let mut failures = Vec::new();
    let mut unreconciled = Vec::new();
    let mut foreign = Vec::new();

    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match ecofin_core::importer::parse_file(path, &account.id, &account.currency) {
            Ok(parsed) => {
                // Le relevé porte l'IBAN du compte auquel il appartient :
                // s'en servir évite de verser des opérations dans le mauvais
                // compte, ce qu'aucun contrôle ultérieur ne rattraperait.
                if let (Some(statement_iban), Some(account_iban)) = (&parsed.iban, &account.iban)
                    && !same_iban(statement_iban, account_iban)
                {
                    foreign.push((name.to_string(), statement_iban.clone()));
                    continue;
                }

                // Le relevé imprime son propre solde : un écart signale une
                // lecture fautive, qu'il vaut mieux signaler que d'enregistrer.
                if !parsed.discrepancy.is_zero() {
                    unreconciled.push((name.to_string(), parsed.discrepancy));
                }
                transactions.extend(parsed.transactions);
            }
            // Un relevé illisible ne doit pas condamner les 48 autres.
            Err(err) => failures.push((name.to_string(), format!("{err:#}"))),
        }
    }

    if !failures.is_empty() {
        println!("{} illisibles :", ui::plural(failures.len(), "fichier"));
        for (name, err) in &failures {
            println!("  {name} — {err}");
        }
        println!();
    }

    if !foreign.is_empty() {
        bail!(
            "{} ne concernent pas ce compte et ont été écartés :\n{}\n\n\
             Compte visé : {} ({}).\n\
             Précise le bon compte avec --account, ou retire ces fichiers.",
            ui::plural(foreign.len(), "relevé"),
            foreign
                .iter()
                .map(|(name, iban)| format!("  {name} — IBAN {iban}"))
                .collect::<Vec<_>>()
                .join("\n"),
            account.display_name(),
            account.iban.as_deref().unwrap_or("sans IBAN"),
        );
    }

    if !unreconciled.is_empty() {
        println!(
            "\x1b[33m{} ne se réconcilient pas avec leur solde imprimé\x1b[0m :",
            ui::plural(unreconciled.len(), "relevé")
        );
        for (name, gap) in &unreconciled {
            println!("  {name} — écart de {}", ui::money(*gap, &account.currency));
        }
        println!("Ces relevés sont probablement mal lus ; vérifie-les avant de t'y fier.\n");
    }

    if transactions.is_empty() {
        bail!("aucune opération lue");
    }

    let first = transactions.iter().filter_map(|t| t.effective_date()).min();
    let last = transactions.iter().filter_map(|t| t.effective_date()).max();
    print!("Lu      : {}", ui::plural(transactions.len(), "opération"));
    match (first, last) {
        (Some(a), Some(b)) => println!(" du {} au {}", a.format("%d/%m/%Y"), b.format("%d/%m/%Y")),
        _ => println!(),
    }

    if dry_run {
        println!("\nAperçu des cinq plus récentes :");
        let mut preview = transactions.clone();
        preview.sort_by_key(|t| std::cmp::Reverse(t.effective_date()));
        let rows: Vec<Vec<String>> = preview
            .iter()
            .take(5)
            .map(|t| {
                vec![
                    t.effective_date()
                        .map(|d| d.format("%d/%m/%Y").to_string())
                        .unwrap_or_else(|| "—".into()),
                    ui::truncate(&ecofin_core::merchant::normalize(&t.description), 48),
                    ui::money(t.amount, &t.currency),
                ]
            })
            .collect();
        ui::table_right_aligned(&["Date", "Libellé", "Montant"], rows, &[2]);
        println!("\n--dry-run : rien n'a été écrit en base.");
        return Ok(());
    }

    let report = store.import_transactions(&account.id, &transactions)?;

    println!("Ajouté  : {}", ui::plural(report.inserted, "opération"));
    if report.duplicates > 0 {
        println!(
            "Ignoré  : {} déjà en base",
            ui::plural(report.duplicates, "opération")
        );
    }
    if report.undated > 0 {
        println!(
            "Écarté  : {} sans date exploitable",
            ui::plural(report.undated, "ligne")
        );
    }

    println!("\necofin tx --limit 500   pour parcourir l'historique complet");
    Ok(())
}

// ---------------------------------------------------------------------------
// balance
// ---------------------------------------------------------------------------

fn balance() -> Result<()> {
    let store = Store::open()?;
    let accounts = store.accounts(Scope::All)?;
    if accounts.is_empty() {
        println!("Aucun compte. Connecte une banque avec `ecofin link new \"<NOM>\"`.");
        return Ok(());
    }

    let mut rows = Vec::new();
    let mut total = rust_decimal::Decimal::ZERO;
    let mut mixed_currencies = false;

    for account in &accounts {
        let balance = store.primary_balance(Scope::All, &account.id)?;
        let (amount, date) = match &balance {
            Some(b) => {
                if b.currency == "EUR" {
                    total += b.amount;
                } else {
                    mixed_currencies = true;
                }
                (
                    ui::money(b.amount, &b.currency),
                    b.reference_date
                        .map(|d| d.format("%d/%m/%Y").to_string())
                        .unwrap_or_else(|| "—".into()),
                )
            }
            None => ("—".to_string(), "—".to_string()),
        };

        rows.push(vec![
            account.institution_name.clone(),
            account.display_name(),
            amount,
            date,
        ]);
    }

    ui::table_right_aligned(&["Banque", "Compte", "Solde", "Arrêté au"], rows, &[2]);
    println!("\nTotal (EUR) : {}", ui::money(total, "EUR"));
    if mixed_currencies {
        println!("Des comptes dans une autre devise ne sont pas inclus dans le total.");
    }
    Ok(())
}

/// Évolution du solde, telle que la banque l'a déclarée à chaque arrêté.
///
/// Une ligne par date connue. C'est le solde *déclaré par la banque*, à
/// distinguer de celui qu'impliquent les opérations enregistrées : confronter
/// les deux reste à faire, et c'est précisément ce que cet historique rend
/// possible.
fn balance_history(account: Option<String>) -> Result<()> {
    let store = Store::open()?;

    let accounts = match &account {
        Some(needle) => vec![
            store
                .find_account(Scope::All, needle)?
                .with_context(|| format!("aucun compte ne correspond à « {needle} »"))?,
        ],
        None => store.accounts(Scope::All)?,
    };

    for account in &accounts {
        let series = store.balance_history(Scope::All, &account.id)?;
        println!("{} — {}", account.institution_name, account.display_name());

        if series.is_empty() {
            println!("  aucun solde enregistré. Lance `ecofin sync`.\n");
            continue;
        }

        let mut rows = Vec::new();
        let mut previous: Option<&ecofin_core::model::Balance> = None;
        for balance in &series {
            let variation = match previous {
                Some(earlier) => ui::money(balance.amount - earlier.amount, &balance.currency),
                None => "—".to_string(),
            };
            rows.push(vec![
                balance
                    .reference_date
                    .map(|d| d.format("%d/%m/%Y").to_string())
                    .unwrap_or_else(|| "—".into()),
                ui::money(balance.amount, &balance.currency),
                variation,
            ]);
            previous = Some(balance);
        }

        ui::table_right_aligned(&["Arrêté au", "Solde", "Variation"], rows, &[1, 2]);
        println!(
            "{} sur {} — type de solde « {} »\n",
            ui::plural(series.len(), "relevé"),
            match (series.first(), series.last()) {
                (Some(a), Some(b)) if a.reference_date != b.reference_date => format!(
                    "{} → {}",
                    a.reference_date
                        .map(|d| d.format("%d/%m/%Y").to_string())
                        .unwrap_or_default(),
                    b.reference_date
                        .map(|d| d.format("%d/%m/%Y").to_string())
                        .unwrap_or_default()
                ),
                _ => "une seule date".to_string(),
            },
            series[0].kind
        );
    }

    if accounts.iter().all(|a| {
        store
            .balance_history(Scope::All, &a.id)
            .map(|s| s.len() < 2)
            .unwrap_or(true)
    }) {
        println!(
            "L'historique se construit à chaque `sync` : une ligne par date d'arrêté.\n\
             Les soldes antérieurs à cette version n'ont pas été conservés, mais restent \n\
             recalculables depuis les opérations."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// tx
// ---------------------------------------------------------------------------

fn transactions(
    account: Option<String>,
    since: Option<String>,
    search: Option<String>,
    limit: usize,
) -> Result<()> {
    let store = Store::open()?;

    let account = match &account {
        Some(needle) => Some(
            store
                .find_account(Scope::All, needle)?
                .with_context(|| format!("aucun compte ne correspond à « {needle} »"))?,
        ),
        None => None,
    };

    let since = match &since {
        Some(raw) => Some(
            NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .with_context(|| format!("date invalide « {raw} », format attendu AAAA-MM-JJ"))?,
        ),
        None => None,
    };

    let found = store.transactions(
        Scope::All,
        account.as_ref().map(|a| a.id.as_str()),
        since,
        search.as_deref(),
        limit,
    )?;

    if found.is_empty() {
        println!("Aucune opération. Lance `ecofin sync` pour en récupérer.");
        return Ok(());
    }

    // Les libellés bancaires sont longs et bruyants ; on les tronque pour que le
    // tableau reste lisible dans un terminal standard.
    let rows: Vec<Vec<String>> = found
        .iter()
        .map(|t| {
            vec![
                t.effective_date()
                    .map(|d| d.format("%d/%m/%Y").to_string())
                    .unwrap_or_else(|| "—".into()),
                ui::truncate(&ecofin_core::merchant::normalize(&t.description), 48),
                ui::money(t.amount, &t.currency),
                if t.booked { "" } else { "en attente" }.to_string(),
            ]
        })
        .collect();

    ui::table_right_aligned(&["Date", "Libellé", "Montant", ""], rows, &[2]);

    let debits: rust_decimal::Decimal = found
        .iter()
        .filter(|t| t.is_debit())
        .map(|t| t.amount)
        .sum();
    let credits: rust_decimal::Decimal = found
        .iter()
        .filter(|t| !t.is_debit())
        .map(|t| t.amount)
        .sum();

    println!(
        "\n{} · entrées {} · sorties {} · net {}",
        ui::plural(found.len(), "opération"),
        ui::money(credits, "EUR"),
        ui::money(debits, "EUR"),
        ui::money(credits + debits, "EUR"),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    std::io::stdout().flush()?;
    let mut buffer = String::new();
    std::io::stdin().read_line(&mut buffer)?;
    Ok(buffer.trim().to_string())
}

/// Développe un `~` en tête de chemin, que le shell n'a pas traité quand la
/// valeur vient d'une invite interactive.
fn expand_tilde(raw: &str) -> PathBuf {
    let Some(rest) = raw.strip_prefix("~/") else {
        return PathBuf::from(raw);
    };
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(rest),
        Err(_) => PathBuf::from(raw),
    }
}
