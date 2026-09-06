//! Persistance locale, en SQLite.
//!
//! Les données bancaires vivent en local et les commandes de lecture ne
//! touchent jamais au réseau : `sync` est la seule à appeler l'API. Cela évite
//! de dépendre de la disponibilité de la banque pour consulter ses comptes, et
//! garde l'historique au-delà de la fenêtre que la banque accepte d'exposer —
//! le consentement expire tous les 90 jours, les données restent.
//!
//! Les montants sont stockés en `TEXT` : SQLite n'a pas de type décimal, et
//! passer par un flottant fait perdre des centimes.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::auth::User;
use crate::config::database_path;
use crate::model::{Account, Balance, Institution, Link, LinkStatus, Transaction};

/// Périmètre de visibilité des données bancaires.
///
/// Chaque requête sur des données bancaires exige un périmètre : c'est le
/// compilateur, et non la vigilance, qui garantit qu'aucune ne l'oublie. Une
/// requête non filtrée serait une fuite d'un utilisateur vers un autre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Toutes les données, sans distinction de propriétaire.
    ///
    /// Réservé au CLI, qui s'exécute sur la machine et pour son propriétaire.
    /// Le serveur web ne doit jamais s'en servir.
    All,
    /// Les seules données appartenant à un utilisateur.
    User(i64),
}

impl Scope {
    /// Fragment de clause `WHERE` restreignant à ce périmètre.
    ///
    /// `alias` est celui de la table portant `user_id`. Les données non
    /// rattachées — antérieures au cloisonnement — restent invisibles d'un
    /// utilisateur tant qu'elles n'ont pas été attribuées : mieux vaut ne rien
    /// montrer que montrer à tort.
    fn clause(&self, alias: &str) -> String {
        match self {
            Scope::All => "1 = 1".to_string(),
            Scope::User(id) => format!("{alias}.user_id = {id}"),
        }
    }
}

pub struct Store {
    conn: Connection,
}

/// Résultat d'un import de relevé.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Opérations réellement ajoutées.
    pub inserted: usize,
    /// Opérations déjà présentes, écartées.
    pub duplicates: usize,
    /// Lignes sans date exploitable, qu'on ne sait pas dédupliquer.
    pub undated: usize,
}

/// Version de schéma attendue par ce code.
///
/// Les migrations sont l'affaire du CLI ; un consommateur en lecture seule
/// vérifie seulement qu'il tombe sur la version qu'il sait lire.
const SCHEMA_VERSION: i64 = 7;

impl Store {
    /// Base en mémoire, au schéma réel.
    ///
    /// Les migrations sont rejouées plutôt que le schéma recopié : un test qui
    /// s'appuierait sur une copie continuerait de passer le jour où le schéma
    /// change sous lui.
    #[cfg(test)]
    fn in_memory() -> Result<Self> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    /// Ouvre la base et applique le schéma. Idempotent.
    pub fn open() -> Result<Self> {
        let path = database_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let conn =
            Connection::open(&path).with_context(|| format!("ouverture de {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Ouvre la base sans jamais pouvoir y écrire.
    ///
    /// Le serveur web s'en sert : une connexion en lecture seule rend toute
    /// écriture impossible, y compris par accident, ce qu'un simple « ne pas
    /// appeler les méthodes d'écriture » ne garantirait pas.
    ///
    /// Le fichier reste accessible en écriture sur le disque, et ce n'est pas
    /// une contradiction : SQLite en mode WAL doit pouvoir créer son fichier
    /// d'index partagé, même pour lire. Monter le volume en lecture seule
    /// empêcherait la base de s'ouvrir du tout.
    pub fn open_read_only() -> Result<Self> {
        use rusqlite::OpenFlags;

        let path = database_path()?;
        if !path.exists() {
            anyhow::bail!(
                "aucune base à {} — lance `ecofin user add <adresse>` depuis le CLI \n\
                 pour la créer",
                path.display()
            );
        }

        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("ouverture en lecture de {}", path.display()))?;

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("lecture de la version du schéma")?;

        // Migrer exigerait d'écrire : on refuse plutôt que de lire un schéma
        // qu'on ne comprend qu'à moitié.
        if version != SCHEMA_VERSION {
            anyhow::bail!(
                "schéma de base en version {version}, attendu {SCHEMA_VERSION}.\n\
                 Lance n'importe quelle commande du CLI (`ecofin accounts`) pour \n\
                 appliquer la migration : seul lui peut écrire."
            );
        }

        Ok(Self { conn })
    }

    fn migrate(&self) -> Result<()> {
        apply_schema(&self.conn)
    }

    /// Attribue à un utilisateur toutes les connexions et comptes orphelins.
    ///
    /// Sert au passage au cloisonnement : les données antérieures n'ont pas de
    /// propriétaire, et il n'y a qu'un candidat possible — le premier compte
    /// créé sur une installation existante.
    pub fn claim_orphans(&self, user_id: i64) -> Result<usize> {
        let links = self.conn.execute(
            "UPDATE links SET user_id = ?1 WHERE user_id IS NULL",
            params![user_id],
        )?;
        let accounts = self.conn.execute(
            "UPDATE accounts SET user_id = ?1 WHERE user_id IS NULL",
            params![user_id],
        )?;
        Ok(links + accounts)
    }

    /// Nombre de connexions et comptes sans propriétaire.
    pub fn orphan_count(&self) -> Result<usize> {
        let links: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM links WHERE user_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        let accounts: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE user_id IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok((links + accounts) as usize)
    }

    // -----------------------------------------------------------------------
    // Sessions de consentement
    // -----------------------------------------------------------------------

    /// Enregistre une session de consentement au nom d'un utilisateur.
    pub fn upsert_link(&self, link: &Link, owner: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO links
                    (id, institution_name, institution_country, status, valid_until, created_at, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    status      = excluded.status,
                    valid_until = COALESCE(excluded.valid_until, links.valid_until)",
                params![
                    link.id,
                    link.institution_name,
                    link.institution_country,
                    status_to_str(link.status),
                    link.valid_until.map(|d| d.to_rfc3339()),
                    link.created_at.to_rfc3339(),
                    owner,
                ],
            )
            .context("enregistrement de la session")?;
        Ok(())
    }

    pub fn set_link_status(&self, link_id: &str, status: LinkStatus) -> Result<()> {
        self.conn
            .execute(
                "UPDATE links SET status = ?2 WHERE id = ?1",
                params![link_id, status_to_str(status)],
            )
            .context("mise à jour de l'état de la session")?;
        Ok(())
    }

    pub fn links(&self, scope: Scope) -> Result<Vec<Link>> {
        let sql = format!(
            "SELECT id, institution_name, institution_country, status, valid_until, created_at
             FROM links WHERE {} ORDER BY created_at DESC",
            scope.clause("links")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Link {
                    id: row.get(0)?,
                    institution_name: row.get(1)?,
                    institution_country: row.get(2)?,
                    status: status_from_str(&row.get::<_, String>(3)?),
                    valid_until: row.get::<_, Option<String>>(4)?.map(|s| parse_datetime(&s)),
                    created_at: parse_datetime(&row.get::<_, String>(5)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Retrouve une session par identifiant, préfixe d'identifiant, ou nom de
    /// banque. Les identifiants sont des UUID, personne ne veut les taper en
    /// entier.
    pub fn find_link(&self, scope: Scope, needle: &str) -> Result<Option<Link>> {
        let lowered = needle.to_lowercase();
        let links = self.links(scope)?;
        let matches: Vec<_> = links
            .into_iter()
            .filter(|l| {
                l.id.starts_with(needle) || l.institution_name.to_lowercase().contains(&lowered)
            })
            .collect();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            n => anyhow::bail!("« {needle} » correspond à {n} sessions, précise davantage"),
        }
    }

    // -----------------------------------------------------------------------
    // Comptes
    // -----------------------------------------------------------------------

    /// Enregistre ou met à jour un compte.
    ///
    /// Le `session_id` est écrasé à chaque fois : reconnecter une banque crée
    /// une nouvelle session pour les mêmes comptes, et c'est la plus récente
    /// qui donne accès aux données.
    pub fn upsert_account(&self, account: &Account, owner: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO accounts
                    (id, session_id, institution_name, institution_country,
                     name, iban, currency, last_synced_at, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    session_id       = excluded.session_id,
                    institution_name = excluded.institution_name,
                    name             = COALESCE(excluded.name, accounts.name),
                    iban             = COALESCE(excluded.iban, accounts.iban),
                    currency         = excluded.currency",
                params![
                    account.id,
                    account.session_id,
                    account.institution_name,
                    account.institution_country,
                    account.name,
                    account.iban,
                    account.currency,
                    account.last_synced_at.map(|d| d.to_rfc3339()),
                    owner,
                ],
            )
            .context("enregistrement du compte")?;
        Ok(())
    }

    pub fn mark_synced(&self, account_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET last_synced_at = ?2 WHERE id = ?1",
            params![account_id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn accounts(&self, scope: Scope) -> Result<Vec<Account>> {
        let sql = format!(
            "SELECT id, session_id, institution_name, institution_country,
                    name, iban, currency, last_synced_at
             FROM accounts WHERE {} ORDER BY institution_name, name",
            scope.clause("accounts")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], row_to_account)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Retrouve un compte par identifiant, préfixe d'identifiant, ou fragment
    /// de libellé.
    pub fn find_account(&self, scope: Scope, needle: &str) -> Result<Option<Account>> {
        let lowered = needle.to_lowercase();
        let accounts = self.accounts(scope)?;
        let matches: Vec<_> = accounts
            .into_iter()
            .filter(|a| {
                a.id.starts_with(needle)
                    || a.iban.as_deref().is_some_and(|i| i.ends_with(needle))
                    || a.display_name().to_lowercase().contains(&lowered)
            })
            .collect();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            n => anyhow::bail!("« {needle} » correspond à {n} comptes, précise davantage"),
        }
    }

    // -----------------------------------------------------------------------
    // Soldes
    // -----------------------------------------------------------------------

    /// Enregistre les soldes lus, sans effacer les précédents.
    ///
    /// Un même arrêté relu deux fois met à jour la ligne existante plutôt que
    /// d'en créer une seconde ; deux arrêtés à des dates différentes coexistent.
    /// C'est ce qui donne la courbe de solde dans le temps, et permet de
    /// confronter ce que déclare la banque à ce qu'impliquent les opérations.
    pub fn record_balances(&self, balances: &[Balance]) -> Result<()> {
        let now = Utc::now();
        for balance in balances {
            // Sans date d'arrêté, on date du jour de lecture : une ligne sans
            // date ne pourrait pas prendre place dans un historique.
            let reference = balance
                .reference_date
                .unwrap_or_else(|| now.date_naive())
                .to_string();

            self.conn.execute(
                "INSERT INTO balances
                    (account_id, kind, amount, currency, reference_date, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(account_id, kind, reference_date) DO UPDATE SET
                    amount     = excluded.amount,
                    fetched_at = excluded.fetched_at",
                params![
                    balance.account_id,
                    balance.kind,
                    balance.amount.to_string(),
                    balance.currency,
                    reference,
                    now.to_rfc3339(),
                ],
            )?;
        }
        Ok(())
    }

    /// Tous les soldes connus d'un compte, du plus récent au plus ancien.
    pub fn balances(&self, scope: Scope, account_id: &str) -> Result<Vec<Balance>> {
        // La jointure sur `accounts` est ce qui rend le périmètre effectif :
        // un compte qui n'appartient pas à l'utilisateur ne remonte rien.
        let sql = format!(
            "SELECT b.account_id, b.kind, b.amount, b.currency, b.reference_date
             FROM balances b
             JOIN accounts ON accounts.id = b.account_id
             WHERE b.account_id = ?1 AND {}
             ORDER BY b.reference_date DESC",
            scope.clause("accounts")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([account_id], |row| {
                Ok(Balance {
                    account_id: row.get(0)?,
                    kind: row.get(1)?,
                    amount: parse_decimal(&row.get::<_, String>(2)?),
                    currency: row.get(3)?,
                    reference_date: NaiveDate::from_str(&row.get::<_, String>(4)?).ok(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Types de solde, du plus représentatif au moins.
    ///
    /// Une banque en renvoie plusieurs pour le même compte. Les codes Berlin
    /// Group (`CLBD`...) et leurs équivalents littéraux coexistent selon les
    /// établissements.
    const BALANCE_PREFERENCE: [&'static str; 8] = [
        "CLBD",
        "closingBooked",
        "ITBD",
        "interimBooked",
        "ITAV",
        "interimAvailable",
        "XPCD",
        "expected",
    ];

    /// Solde le plus représentatif d'un compte, à la date la plus récente.
    pub fn primary_balance(&self, scope: Scope, account_id: &str) -> Result<Option<Balance>> {
        let balances = self.balances(scope, account_id)?;
        for kind in Self::BALANCE_PREFERENCE {
            // `balances` est déjà trié par date décroissante : le premier trouvé
            // est le plus récent de ce type.
            if let Some(found) = balances.iter().find(|b| b.kind == kind) {
                return Ok(Some(found.clone()));
            }
        }
        Ok(balances.into_iter().next())
    }

    /// Courbe de solde d'un compte : une valeur par date d'arrêté, du plus
    /// ancien au plus récent.
    ///
    /// On ne mélange pas les types de solde, qui ne mesurent pas la même chose :
    /// la série est construite sur le type le plus représentatif dont on
    /// dispose.
    pub fn balance_history(&self, scope: Scope, account_id: &str) -> Result<Vec<Balance>> {
        let balances = self.balances(scope, account_id)?;
        let kind: String = match Self::BALANCE_PREFERENCE
            .iter()
            .find(|kind| balances.iter().any(|b| b.kind == **kind))
        {
            Some(preferred) => (*preferred).to_string(),
            None => match balances.first() {
                Some(first) => first.kind.clone(),
                None => return Ok(Vec::new()),
            },
        };

        let mut series: Vec<Balance> = balances.into_iter().filter(|b| b.kind == kind).collect();
        series.sort_by_key(|b| b.reference_date);
        Ok(series)
    }

    // -----------------------------------------------------------------------
    // Transactions
    // -----------------------------------------------------------------------

    /// Insère les transactions manquantes et renvoie le nombre de nouvelles.
    ///
    /// Une opération en attente devient comptabilisée quelques jours plus tard,
    /// parfois avec un identifiant différent : on met à jour l'existant plutôt
    /// que d'empiler des doublons.
    pub fn upsert_transactions(&mut self, transactions: &[Transaction]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut inserted = 0usize;
        {
            let mut exists =
                tx.prepare("SELECT 1 FROM transactions WHERE id = ?1 AND account_id = ?2")?;
            let mut insert = tx.prepare(
                "INSERT INTO transactions
                    (id, account_id, amount, currency, booking_date, value_date,
                     description, counterparty, bank_category, booked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id, account_id) DO UPDATE SET
                    booked       = excluded.booked,
                    booking_date = COALESCE(excluded.booking_date, transactions.booking_date),
                    description  = excluded.description",
            )?;

            for t in transactions {
                let known = exists
                    .query_row(params![t.id, t.account_id], |_| Ok(()))
                    .optional()?
                    .is_some();
                insert.execute(params![
                    t.id,
                    t.account_id,
                    t.amount.to_string(),
                    t.currency,
                    t.booking_date.map(|d| d.to_string()),
                    t.value_date.map(|d| d.to_string()),
                    t.description,
                    t.counterparty,
                    t.bank_category,
                    t.booked as i32,
                ])?;
                if !known {
                    inserted += 1;
                }
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Retire les opérations en attente que la banque ne signale plus.
    ///
    /// Une opération en attente est un **instantané**, pas un fait acquis. La
    /// banque la remplace au bout de quelques jours par une écriture
    /// définitive portant un autre identifiant, souvent un autre libellé — LCL
    /// préfixe « CARTE 0602685 » — et parfois une autre date. Rien ne permet
    /// alors de rapprocher les deux lignes : la provisoire subsiste à côté de
    /// la définitive, et le paiement compte double.
    ///
    /// La liste que la banque vient de renvoyer fait donc autorité : toute
    /// attente que nous détenons et qu'elle ne mentionne plus a été
    /// comptabilisée sous un autre identifiant, ou annulée. Dans les deux cas
    /// elle n'a plus à figurer.
    ///
    /// `since` borne le nettoyage à la fenêtre réellement interrogée : au-delà,
    /// la banque n'a rien dit, et son silence ne prouve rien. `None` vaut pour
    /// une synchronisation complète, qui a tout couvert.
    pub fn prune_stale_pending(
        &self,
        account_id: &str,
        since: Option<NaiveDate>,
        fresh: &HashSet<String>,
    ) -> Result<usize> {
        let bound = since.map(|d| d.to_string());
        let mut stmt = self.conn.prepare(
            "SELECT id FROM transactions
             WHERE account_id = ?1 AND booked = 0
               AND (?2 IS NULL OR COALESCE(booking_date, value_date) >= ?2)",
        )?;
        let held: Vec<String> = stmt
            .query_map(params![account_id, bound], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let stale: Vec<String> = held.into_iter().filter(|id| !fresh.contains(id)).collect();

        let mut removed = 0usize;
        for id in &stale {
            removed += self.conn.execute(
                "DELETE FROM transactions WHERE id = ?1 AND account_id = ?2 AND booked = 0",
                params![id, account_id],
            )?;
        }
        Ok(removed)
    }

    /// Transactions filtrées, les plus récentes d'abord.
    pub fn transactions(
        &self,
        scope: Scope,
        account_id: Option<&str>,
        since: Option<NaiveDate>,
        search: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Transaction>> {
        let mut sql = format!(
            "SELECT t.id, t.account_id, t.amount, t.currency, t.booking_date, t.value_date,
                    t.description, t.counterparty, t.bank_category, t.booked
             FROM transactions t
             JOIN accounts ON accounts.id = t.account_id
             WHERE {}",
            scope.clause("accounts")
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(id) = account_id {
            sql.push_str(" AND t.account_id = ?");
            args.push(Box::new(id.to_string()));
        }
        if let Some(date) = since {
            sql.push_str(" AND COALESCE(t.booking_date, t.value_date) >= ?");
            args.push(Box::new(date.to_string()));
        }
        if let Some(term) = search {
            // `LIKE` est insensible à la casse en ASCII sous SQLite ; suffisant
            // pour chercher un commerçant dans un libellé.
            sql.push_str(" AND (t.description LIKE ? OR COALESCE(t.counterparty, '') LIKE ?)");
            let pattern = format!("%{term}%");
            args.push(Box::new(pattern.clone()));
            args.push(Box::new(pattern));
        }
        sql.push_str(" ORDER BY COALESCE(t.booking_date, t.value_date) DESC, t.id DESC LIMIT ?");
        args.push(Box::new(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), row_to_transaction)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insère des opérations venues d'un export, en écartant celles que l'API a
    /// déjà rapportées.
    ///
    /// Un export ne porte pas les références d'écriture de la banque : la
    /// déduplication par identifiant ne peut donc rien. On compare sur le
    /// couple (date, montant), en comptant les occurrences plutôt qu'en
    /// cherchant une simple présence — deux achats identiques le même jour
    /// restent deux opérations, alors qu'un test d'existence en perdrait une.
    pub fn import_transactions(
        &mut self,
        account_id: &str,
        incoming: &[Transaction],
    ) -> Result<ImportReport> {
        let mut known = self.date_amount_counts(account_id)?;
        let mut to_insert = Vec::new();
        let mut duplicates = 0usize;
        let mut undated = 0usize;

        for transaction in incoming {
            let Some(date) = transaction.effective_date() else {
                undated += 1;
                continue;
            };
            let key = (date, transaction.amount);
            match known.get_mut(&key) {
                // Déjà couverte par une opération existante : on consomme
                // l'occurrence pour que la suivante, elle, soit insérée.
                Some(remaining) if *remaining > 0 => {
                    *remaining -= 1;
                    duplicates += 1;
                }
                _ => to_insert.push(transaction.clone()),
            }
        }

        let inserted = self.upsert_transactions(&to_insert)?;
        Ok(ImportReport {
            inserted,
            duplicates,
            undated,
        })
    }

    /// Compte les opérations existantes par couple (date, montant).
    ///
    /// Les montants sont relus en `Decimal` plutôt que comparés sous forme de
    /// chaîne : « -12.5 » et « -12.50 » désignent la même somme et doivent se
    /// reconnaître.
    fn date_amount_counts(&self, account_id: &str) -> Result<HashMap<(NaiveDate, Decimal), usize>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(booking_date, value_date), amount
             FROM transactions
             WHERE account_id = ?1 AND COALESCE(booking_date, value_date) IS NOT NULL",
        )?;

        let mut counts: HashMap<(NaiveDate, Decimal), usize> = HashMap::new();
        let rows = stmt.query_map([account_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (date, amount) = row?;
            if let Ok(date) = NaiveDate::from_str(&date) {
                *counts.entry((date, parse_decimal(&amount))).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    /// Une opération par son identifiant.
    pub fn transaction(&self, scope: Scope, id: &str) -> Result<Option<Transaction>> {
        let sql = format!(
            "SELECT t.id, t.account_id, t.amount, t.currency, t.booking_date, t.value_date,
                    t.description, t.counterparty, t.bank_category, t.booked
             FROM transactions t
             JOIN accounts ON accounts.id = t.account_id
             WHERE t.id = ?1 AND {}",
            scope.clause("accounts")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt.query_row([id], row_to_transaction).optional()?)
    }

    /// Première et dernière dates d'opération connues pour un compte.
    ///
    /// Sert à borner les sélecteurs de période : proposer des dates hors de
    /// l'historique n'aurait aucun sens.
    pub fn transaction_span(
        &self,
        scope: Scope,
        account_id: &str,
    ) -> Result<Option<(NaiveDate, NaiveDate)>> {
        let sql = format!(
            "SELECT MIN(COALESCE(t.booking_date, t.value_date)),
                    MAX(COALESCE(t.booking_date, t.value_date))
             FROM transactions t
             JOIN accounts ON accounts.id = t.account_id
             WHERE t.account_id = ?1 AND {}",
            scope.clause("accounts")
        );
        let row: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(&sql, params![account_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;

        Ok(match row {
            Some((Some(first), Some(last))) => {
                match (NaiveDate::from_str(&first), NaiveDate::from_str(&last)) {
                    (Ok(first), Ok(last)) => Some((first, last)),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    /// Date de l'opération la plus récente connue pour un compte, qui sert de
    /// point de départ à la synchronisation incrémentale.
    pub fn latest_transaction_date(&self, account_id: &str) -> Result<Option<NaiveDate>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT MAX(COALESCE(booking_date, value_date))
                 FROM transactions WHERE account_id = ?1",
                params![account_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(raw.and_then(|s| NaiveDate::from_str(&s).ok()))
    }

    // -----------------------------------------------------------------------
    // Zones de brouillard
    // -----------------------------------------------------------------------

    /// Consigne une période devenue inaccessible.
    ///
    /// Une même période détectée deux fois ne crée qu'une entrée : la date de
    /// détection est rafraîchie, le trou reste unique.
    pub fn record_gap(&self, gap: &crate::schedule::FogGap) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fog_gaps (account_id, from_date, to_date, detected_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, from_date) DO UPDATE SET
                to_date     = excluded.to_date,
                detected_at = excluded.detected_at",
            params![
                gap.account_id,
                gap.from.to_string(),
                gap.to.to_string(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Zones de brouillard encore béantes.
    ///
    /// Une zone comblée par un import de relevés cesse d'en être une : plutôt
    /// que d'exiger un marquage manuel, on vérifie si des opérations couvrent
    /// désormais la période. C'est la donnée qui fait foi, pas un drapeau.
    pub fn open_gaps(&self, scope: Scope) -> Result<Vec<crate::schedule::FogGap>> {
        let sql = format!(
            "SELECT g.account_id, g.from_date, g.to_date
             FROM fog_gaps g
             JOIN accounts ON accounts.id = g.account_id
             WHERE {}
             ORDER BY g.from_date DESC",
            scope.clause("accounts")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let candidates: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut open = Vec::new();
        for (account_id, from, to) in candidates {
            let (Ok(from), Ok(to)) = (NaiveDate::from_str(&from), NaiveDate::from_str(&to)) else {
                continue;
            };
            if self.covers(&account_id, from, to)? {
                continue;
            }
            open.push(crate::schedule::FogGap {
                account_id,
                from,
                to,
            });
        }
        Ok(open)
    }

    /// Vrai si des opérations existent dans la période donnée.
    fn covers(&self, account_id: &str, from: NaiveDate, to: NaiveDate) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM transactions
             WHERE account_id = ?1
               AND COALESCE(booking_date, value_date) BETWEEN ?2 AND ?3",
            params![account_id, from.to_string(), to.to_string()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    // -----------------------------------------------------------------------
    // Comptes utilisateurs et sessions
    // -----------------------------------------------------------------------

    /// Crée un compte. Le mot de passe est haché avant d'arriver ici.
    pub fn create_user(&self, email: &str, password_hash: &str) -> Result<User> {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            anyhow::bail!("adresse vide");
        }

        let created_at = Utc::now();
        self.conn
            .execute(
                "INSERT INTO users (email, password_hash, token_version, created_at)
                 VALUES (?1, ?2, 1, ?3)",
                params![email, password_hash, created_at.to_rfc3339()],
            )
            .map_err(|err| match err {
                // La contrainte d'unicité est le cas courant : mieux vaut un
                // message clair qu'une erreur SQLite brute.
                rusqlite::Error::SqliteFailure(e, _)
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    anyhow::anyhow!("un compte existe déjà pour {email}")
                }
                other => anyhow::Error::new(other).context("création du compte"),
            })?;

        Ok(User {
            id: self.conn.last_insert_rowid(),
            email,
            password_hash: password_hash.to_string(),
            token_version: 1,
            created_at,
        })
    }

    /// Retrouve un compte par son adresse, insensible à la casse.
    pub fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email, password_hash, token_version, created_at
             FROM users WHERE email = ?1 COLLATE NOCASE",
        )?;
        Ok(stmt.query_row([email.trim()], row_to_user).optional()?)
    }

    pub fn user_by_id(&self, id: i64) -> Result<Option<User>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email, password_hash, token_version, created_at
             FROM users WHERE id = ?1",
        )?;
        Ok(stmt.query_row([id], row_to_user).optional()?)
    }

    pub fn users(&self) -> Result<Vec<User>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, email, password_hash, token_version, created_at
             FROM users ORDER BY email",
        )?;
        Ok(stmt
            .query_map([], row_to_user)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Change le mot de passe et invalide les sessions en cours.
    ///
    /// Les deux vont ensemble : un mot de passe changé parce qu'on le croit
    /// compromis ne doit pas laisser vivre les sessions ouvertes avec l'ancien.
    pub fn set_password(&self, email: &str, password_hash: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE users
             SET password_hash = ?2, token_version = token_version + 1
             WHERE email = ?1 COLLATE NOCASE",
            params![email.trim(), password_hash],
        )?;
        match changed {
            0 => anyhow::bail!("aucun compte pour {email}"),
            _ => Ok(()),
        }
    }

    /// Invalide toutes les sessions en cours d'un compte.
    pub fn revoke_sessions(&self, email: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE users SET token_version = token_version + 1
             WHERE email = ?1 COLLATE NOCASE",
            params![email.trim()],
        )?;
        match changed {
            0 => anyhow::bail!("aucun compte pour {email}"),
            _ => Ok(()),
        }
    }

    pub fn delete_user(&self, email: &str) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM users WHERE email = ?1 COLLATE NOCASE",
            params![email.trim()],
        )?;
        match changed {
            0 => anyhow::bail!("aucun compte pour {email}"),
            _ => Ok(()),
        }
    }

    /// Secret de signature des sessions, créé au premier appel.
    ///
    /// Écrit par le CLI : le serveur web, en lecture seule, ne peut que le lire
    /// — d'où [`Store::session_secret`], qui ne crée rien.
    pub fn ensure_session_secret(&self) -> Result<String> {
        if let Some(existing) = self.session_secret()? {
            return Ok(existing);
        }
        let secret = crate::auth::generate_session_secret();
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('session_secret', ?1)",
            params![secret],
        )?;
        Ok(secret)
    }

    /// Libellés désignant les comptes de l'utilisateur.
    ///
    /// Sert à distinguer un virement interne d'une dépense. Stockés séparés
    /// par des virgules dans les réglages.
    pub fn own_account_names(&self) -> Result<Vec<String>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'own_account_names'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        Ok(raw
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub fn set_own_account_names(&self, names: &[String]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('own_account_names', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![names.join(",")],
        )?;
        Ok(())
    }

    /// Lit le secret de signature, sans le créer.
    pub fn session_secret(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'session_secret'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    // -----------------------------------------------------------------------
    // Établissements
    // -----------------------------------------------------------------------

    /// Met en cache la liste des établissements d'un pays.
    ///
    /// La liste est remplacée en bloc : une banque retirée de la couverture
    /// doit disparaître du cache, pas y survivre indéfiniment.
    pub fn cache_institutions(
        &mut self,
        country: &str,
        institutions: &[Institution],
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM institutions WHERE country = ?1",
            params![country.to_uppercase()],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO institutions
                    (name, country, max_consent_validity, psu_types, beta, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for institution in institutions {
                insert.execute(params![
                    institution.name,
                    institution.country,
                    institution.max_consent_validity,
                    institution.psu_types.join(","),
                    institution.beta as i32,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Établissements en cache, si celui-ci est encore frais.
    ///
    /// `None` déclenche un appel réseau : cache absent ou périmé.
    pub fn cached_institutions(
        &self,
        country: &str,
        max_age: chrono::Duration,
    ) -> Result<Option<Vec<Institution>>> {
        let cutoff = (Utc::now() - max_age).to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT name, country, max_consent_validity, psu_types, beta
             FROM institutions
             WHERE country = ?1 AND cached_at >= ?2
             ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![country.to_uppercase(), cutoff], |row| {
                Ok(Institution {
                    name: row.get(0)?,
                    country: row.get(1)?,
                    max_consent_validity: row.get(2)?,
                    psu_types: row
                        .get::<_, String>(3)?
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect(),
                    beta: row.get::<_, i32>(4)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(match rows.is_empty() {
            true => None,
            false => Some(rows),
        })
    }

    // -----------------------------------------------------------------------
    // Quota d'appels
    // -----------------------------------------------------------------------

    pub fn record_api_call(&self, account_id: &str, endpoint: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO api_calls (account_id, endpoint, called_at) VALUES (?1, ?2, ?3)",
            params![account_id, endpoint, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Instant du dernier appel réseau, tous comptes confondus.
    ///
    /// Sert de repère au planificateur : c'est de là qu'il compte les créneaux
    /// écoulés.
    pub fn last_sync(&self) -> Result<Option<DateTime<Utc>>> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT MAX(called_at) FROM api_calls", [], |row| row.get(0))
            .optional()?
            .flatten();
        Ok(raw.map(|s| parse_datetime(&s)))
    }

    /// Appels passés aujourd'hui sur un endpoint donné, pour un compte.
    ///
    /// La journée est celle du calendrier UTC : c'est ce qui permet de ne
    /// relever les soldes qu'une fois par jour.
    pub fn calls_today(&self, account_id: &str, endpoint: &str) -> Result<u32> {
        let today = Utc::now().date_naive().to_string();
        let count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM api_calls
             WHERE account_id = ?1 AND endpoint = ?2 AND substr(called_at, 1, 10) = ?3",
            params![account_id, endpoint, today],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Nombre d'appels passés pour un compte dans les dernières 24 h, tous
    /// endpoints confondus.
    ///
    /// Enable Banking n'impose pas de quota journalier chiffré, mais applique
    /// une politique d'usage raisonnable : ce compteur sert à repérer une
    /// synchronisation lancée en boucle.
    pub fn calls_last_24h(&self, account_id: &str) -> Result<u32> {
        let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM api_calls
             WHERE account_id = ?1 AND called_at >= ?2",
            params![account_id, cutoff],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn row_to_user(row: &Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        password_hash: row.get(2)?,
        token_version: row.get(3)?,
        created_at: parse_datetime(&row.get::<_, String>(4)?),
    })
}

fn row_to_account(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        session_id: row.get(1)?,
        institution_name: row.get(2)?,
        institution_country: row.get(3)?,
        name: row.get(4)?,
        iban: row.get(5)?,
        currency: row.get(6)?,
        last_synced_at: row.get::<_, Option<String>>(7)?.map(|s| parse_datetime(&s)),
    })
}

fn row_to_transaction(row: &Row<'_>) -> rusqlite::Result<Transaction> {
    Ok(Transaction {
        id: row.get(0)?,
        account_id: row.get(1)?,
        amount: parse_decimal(&row.get::<_, String>(2)?),
        currency: row.get(3)?,
        booking_date: row
            .get::<_, Option<String>>(4)?
            .and_then(|s| NaiveDate::from_str(&s).ok()),
        value_date: row
            .get::<_, Option<String>>(5)?
            .and_then(|s| NaiveDate::from_str(&s).ok()),
        description: row.get(6)?,
        counterparty: row.get(7)?,
        bank_category: row.get(8)?,
        booked: row.get::<_, i32>(9)? != 0,
    })
}

/// Les montants viennent d'une colonne que nous avons nous-mêmes écrite depuis
/// un `Decimal` : un échec de parsing signalerait une base corrompue, on retombe
/// sur zéro plutôt que de faire échouer toute la commande.
fn parse_decimal(raw: &str) -> Decimal {
    Decimal::from_str(raw).unwrap_or_default()
}

fn parse_datetime(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn status_to_str(status: LinkStatus) -> &'static str {
    match status {
        LinkStatus::Pending => "pending",
        LinkStatus::Linked => "linked",
        LinkStatus::Expired => "expired",
        LinkStatus::Rejected => "rejected",
    }
}

fn status_from_str(raw: &str) -> LinkStatus {
    match raw {
        "linked" => LinkStatus::Linked,
        "expired" => LinkStatus::Expired,
        "rejected" => LinkStatus::Rejected,
        _ => LinkStatus::Pending,
    }
}

/// Applique le schéma complet à une connexion, puis les migrations.
///
/// Extrait de [`Store`] pour que les bases de test rejouent le schéma réel
/// plutôt qu'une copie : une copie continuerait de passer le jour où le
/// schéma change sous elle, et c'est précisément ce qui est arrivé.
pub(crate) fn apply_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS links (
                id                  TEXT PRIMARY KEY,
                institution_name    TEXT NOT NULL,
                institution_country TEXT NOT NULL,
                status              TEXT NOT NULL,
                valid_until         TEXT,
                created_at          TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS accounts (
                id                  TEXT PRIMARY KEY,
                session_id          TEXT NOT NULL,
                institution_name    TEXT NOT NULL,
                institution_country TEXT NOT NULL,
                name                TEXT,
                iban                TEXT,
                currency            TEXT NOT NULL,
                last_synced_at      TEXT
            );

            -- Un relevé de solde par compte, par type et par date d'arrêté.
            -- La date fait partie de la clé : c'est ce qui conserve
            -- l'historique au lieu de n'garder que le dernier instantané.
            CREATE TABLE IF NOT EXISTS balances (
                account_id        TEXT NOT NULL,
                kind              TEXT NOT NULL,
                amount            TEXT NOT NULL,
                currency          TEXT NOT NULL,
                reference_date    TEXT NOT NULL,
                fetched_at        TEXT NOT NULL,
                PRIMARY KEY (account_id, kind, reference_date)
            );

            -- Cache de la liste des établissements : elle ne bouge que
            -- rarement, et la retélécharger empêche de préparer une
            -- connexion hors ligne.
            CREATE TABLE IF NOT EXISTS institutions (
                name                 TEXT NOT NULL,
                country              TEXT NOT NULL,
                max_consent_validity INTEGER,
                psu_types            TEXT NOT NULL,
                beta                 INTEGER NOT NULL,
                cached_at            TEXT NOT NULL,
                PRIMARY KEY (name, country)
            );

            -- Comptes autorisés à ouvrir une session sur l'application
            -- web. Créés depuis le CLI : le serveur web, monté en lecture
            -- seule, ne peut pas écrire ici.
            CREATE TABLE IF NOT EXISTS users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                email         TEXT NOT NULL UNIQUE COLLATE NOCASE,
                password_hash TEXT NOT NULL,
                token_version INTEGER NOT NULL DEFAULT 1,
                created_at    TEXT NOT NULL
            );

            -- Réglages internes, dont le secret de signature des sessions.
            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS transactions (
                id                TEXT NOT NULL,
                account_id        TEXT NOT NULL,
                amount            TEXT NOT NULL,
                currency          TEXT NOT NULL,
                booking_date      TEXT,
                value_date        TEXT,
                description       TEXT NOT NULL,
                counterparty      TEXT,
                bank_category     TEXT,
                booked            INTEGER NOT NULL,
                PRIMARY KEY (id, account_id)
            );

            CREATE INDEX IF NOT EXISTS idx_tx_dedup
                ON transactions (account_id, booking_date, amount);

            CREATE INDEX IF NOT EXISTS idx_tx_account_date
                ON transactions (account_id, booking_date DESC);

            -- Journal des appels réseau. Enable Banking n'impose pas de
            -- quota journalier strict, mais garder la trace permet de
            -- diagnostiquer un 429 et d'éviter les synchronisations
            -- inutilement rapprochées.
            CREATE TABLE IF NOT EXISTS api_calls (
                account_id        TEXT NOT NULL,
                endpoint          TEXT NOT NULL,
                called_at         TEXT NOT NULL
            );

            -- Périodes que l'API ne peut plus rendre, faute de
            -- synchronisation dans sa fenêtre glissante. Consignées pour
            -- que l'utilisateur sache quels relevés aller chercher.
            CREATE TABLE IF NOT EXISTS fog_gaps (
                account_id  TEXT NOT NULL,
                from_date   TEXT NOT NULL,
                to_date     TEXT NOT NULL,
                detected_at TEXT NOT NULL,
                PRIMARY KEY (account_id, from_date)
            );

            -- Catégories choisies par l'utilisateur, qui priment sur les
            -- motifs internes. Rattachées au libellé nettoyé : classer une
            -- opération classe donc toutes celles du même bénéficiaire.
            CREATE TABLE IF NOT EXISTS category_rules (
                user_id    INTEGER NOT NULL,
                label      TEXT NOT NULL,
                category   TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, label)
            );

            CREATE INDEX IF NOT EXISTS idx_api_calls
                ON api_calls (account_id, endpoint, called_at DESC);
            "#,
    )
    .context("application du schéma SQLite")?;

    upgrade(conn)
}

/// Fait évoluer une base créée par une version antérieure.
///
/// `CREATE TABLE IF NOT EXISTS` ne touche pas à une table déjà présente :
/// un changement de clé primaire demande donc une reconstruction explicite,
/// pilotée par `PRAGMA user_version`.
fn upgrade(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("lecture de la version du schéma")?;

    if version < 1 {
        // v1 : les soldes deviennent un historique. L'ancienne table avait
        // pour clé (compte, type) et ne gardait donc qu'un instantané,
        // effacé à chaque synchronisation.
        conn.execute_batch(
            r#"
                BEGIN;

                CREATE TABLE IF NOT EXISTS balances_v1 (
                    account_id        TEXT NOT NULL,
                    kind              TEXT NOT NULL,
                    amount            TEXT NOT NULL,
                    currency          TEXT NOT NULL,
                    reference_date    TEXT NOT NULL,
                    fetched_at        TEXT NOT NULL,
                    PRIMARY KEY (account_id, kind, reference_date)
                );

                -- Les soldes sans date d'arrêté sont datés du jour où ils
                -- ont été lus : sans date, ils ne peuvent pas entrer dans
                -- un historique.
                INSERT OR IGNORE INTO balances_v1
                SELECT account_id, kind, amount, currency,
                       COALESCE(reference_date, substr(fetched_at, 1, 10)),
                       fetched_at
                FROM balances;

                DROP TABLE balances;
                ALTER TABLE balances_v1 RENAME TO balances;

                PRAGMA user_version = 1;
                COMMIT;
                "#,
        )
        .context("migration des soldes vers un historique")?;
    }

    if version < 2 {
        // v2 : les connexions bancaires et les comptes appartiennent à un
        // utilisateur. La colonne est nullable : les données antérieures
        // n'ont pas de propriétaire tant qu'elles n'ont pas été attribuées,
        // et restent alors invisibles depuis le web.
        conn.execute_batch(
            r#"
                BEGIN;
                ALTER TABLE links    ADD COLUMN user_id INTEGER;
                ALTER TABLE accounts ADD COLUMN user_id INTEGER;
                CREATE INDEX IF NOT EXISTS idx_accounts_user ON accounts (user_id);
                CREATE INDEX IF NOT EXISTS idx_links_user    ON links (user_id);
                PRAGMA user_version = 2;
                COMMIT;
                "#,
        )
        .context("rattachement des comptes à un utilisateur")?;
    }

    if version < 3 {
        // v3 : règles de classement posées par l'utilisateur.
        //
        // Une table ajoutée au schéma de base sans incrémenter la version
        // resterait invisible du contrôle : un serveur en lecture seule
        // passerait la vérification tout en butant sur la table absente.
        // Toute nouvelle table doit donc s'accompagner d'une version.
        conn.execute_batch(
            r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS category_rules (
                    user_id    INTEGER NOT NULL,
                    label      TEXT NOT NULL,
                    category   TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (user_id, label)
                );
                PRAGMA user_version = 3;
                COMMIT;
                "#,
        )
        .context("création des règles de classement")?;
    }

    if version < 4 {
        // v4 : les catégories deviennent des données.
        //
        // Les 158 motifs automatiques restent écrits en Rust et désignent
        // les catégories d'origine. Ce que cette table ajoute, c'est un
        // nom modifiable, des catégories propres à l'utilisateur, et une
        // redirection : « supprimer » une catégorie d'origine revient à
        // renvoyer ses motifs ailleurs, sans les réécrire ni les perdre.
        conn.execute_batch(
            r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS categories (
                    user_id     INTEGER NOT NULL,
                    key         TEXT NOT NULL,
                    label       TEXT NOT NULL,
                    is_spending INTEGER NOT NULL,
                    builtin     INTEGER NOT NULL,
                    -- Non nul : la catégorie est retirée, et tout ce qui
                    -- la désignait est redirigé ici.
                    merged_into TEXT,
                    position    INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (user_id, key)
                );
                PRAGMA user_version = 4;
                COMMIT;
                "#,
        )
        .context("création des catégories modifiables")?;
    }

    if version < 5 {
        // v5 : les catégories s'emboîtent.
        //
        // « Essence » et « Péage » relèvent de « Transport » : le
        // rattachement permet de chercher et de totaliser un secteur avec
        // tout ce qu'il contient, sans dupliquer les opérations. Nul par
        // défaut, donc les catégories existantes restent à la racine.
        conn.execute_batch(
            r#"
                BEGIN;
                ALTER TABLE categories ADD COLUMN parent TEXT;
                PRAGMA user_version = 5;
                COMMIT;
                "#,
        )
        .context("emboîtement des catégories")?;
    }

    if version < 6 {
        // v6 : les portefeuilles virtuels.
        //
        // Une enveloppe est une consigne, pas un compte : sa dotation
        // mensuelle et ses catégories suffisent à rejouer tout
        // l'historique. Rien n'est donc figé mois par mois — une opération
        // arrivée en retard se répercute d'elle-même sur la suite.
        //
        // `carry_to` porte la redirection du reliquat en fin de mois ;
        // nul, l'enveloppe garde le sien.
        conn.execute_batch(
            r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS wallets (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id     INTEGER NOT NULL,
                    name        TEXT NOT NULL,
                    -- Comme les montants bancaires : en texte, pour que
                    -- les décimales traversent sans perte.
                    allocation  TEXT NOT NULL,
                    carry_to    INTEGER,
                    start_month TEXT NOT NULL,
                    position    INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS wallet_categories (
                    wallet_id INTEGER NOT NULL,
                    category  TEXT NOT NULL,
                    PRIMARY KEY (wallet_id, category)
                );
                PRAGMA user_version = 6;
                COMMIT;
                "#,
        )
        .context("création des portefeuilles")?;
    }

    if version < 7 {
        // v7 : les enveloppes s'emboîtent.
        //
        // Une mère dote ses filles : sa dotation devient le budget de toute la
        // branche. Nul par défaut, donc les enveloppes existantes restent au
        // premier rang et leur dotation garde exactement le sens qu'elle avait.
        conn.execute_batch(
            r#"
                BEGIN;
                ALTER TABLE wallets ADD COLUMN parent INTEGER;
                PRAGMA user_version = 7;
                COMMIT;
                "#,
        )
        .context("emboîtement des portefeuilles")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPTE: &str = "compte-1";

    fn operation(id: &str, date: &str, montant: &str, booked: bool) -> Transaction {
        Transaction {
            id: id.to_string(),
            account_id: COMPTE.to_string(),
            amount: Decimal::from_str(montant).unwrap(),
            currency: "EUR".to_string(),
            booking_date: Some(NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap()),
            value_date: None,
            description: format!("libellé {id}"),
            counterparty: None,
            bank_category: None,
            booked,
        }
    }

    fn ids(store: &Store) -> Vec<String> {
        let mut stmt = store
            .conn
            .prepare("SELECT id FROM transactions ORDER BY id")
            .unwrap();
        let found: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        found
    }

    fn fresh(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Le défaut constaté : la banque comptabilise une attente sous un autre
    /// identifiant, et les deux lignes se retrouvent côte à côte.
    #[test]
    fn a_pending_the_bank_no_longer_reports_is_removed() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_transactions(&[
                operation("synth:2026-08-30:DBIT:140", "2026-08-30", "-140", false),
                operation("uuid-comptabilise", "2026-08-30", "-140", true),
            ])
            .unwrap();

        let retires = store
            .prune_stale_pending(
                COMPTE,
                Some(NaiveDate::parse_from_str("2026-08-27", "%Y-%m-%d").unwrap()),
                &fresh(&[]),
            )
            .unwrap();

        assert_eq!(retires, 1);
        assert_eq!(ids(&store), vec!["uuid-comptabilise"]);
    }

    /// Une attente que la banque signale encore est un fait présent, pas un
    /// résidu : la retirer ferait disparaître un paiement à venir.
    #[test]
    fn a_pending_the_bank_still_reports_is_kept() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_transactions(&[operation("synth:paypal", "2026-09-03", "-12.14", false)])
            .unwrap();

        let retires = store
            .prune_stale_pending(COMPTE, None, &fresh(&["synth:paypal"]))
            .unwrap();

        assert_eq!(retires, 0);
        assert_eq!(ids(&store), vec!["synth:paypal"]);
    }

    /// Une écriture définitive ne se retire jamais, même absente de la liste
    /// des attentes — elle n'y a par définition pas sa place.
    #[test]
    fn a_booked_transaction_is_never_removed() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_transactions(&[operation("uuid-1", "2026-08-30", "-140", true)])
            .unwrap();

        assert_eq!(
            store
                .prune_stale_pending(COMPTE, None, &fresh(&[]))
                .unwrap(),
            0
        );
        assert_eq!(ids(&store), vec!["uuid-1"]);
    }

    /// Hors de la fenêtre interrogée, la banque n'a rien dit : son silence ne
    /// prouve pas que l'attente a été comptabilisée.
    #[test]
    fn a_pending_outside_the_queried_window_is_left_alone() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_transactions(&[operation("synth:ancien", "2026-01-05", "-10", false)])
            .unwrap();

        let retires = store
            .prune_stale_pending(
                COMPTE,
                Some(NaiveDate::parse_from_str("2026-08-27", "%Y-%m-%d").unwrap()),
                &fresh(&[]),
            )
            .unwrap();

        assert_eq!(retires, 0);
        assert_eq!(ids(&store), vec!["synth:ancien"]);
    }

    /// Une synchronisation complète a tout couvert : le nettoyage aussi.
    #[test]
    fn a_full_sync_prunes_the_whole_history() {
        let mut store = Store::in_memory().unwrap();
        store
            .upsert_transactions(&[operation("synth:ancien", "2026-01-05", "-10", false)])
            .unwrap();

        assert_eq!(
            store
                .prune_stale_pending(COMPTE, None, &fresh(&[]))
                .unwrap(),
            1
        );
        assert!(ids(&store).is_empty());
    }

    /// Le nettoyage d'un compte ne touche pas aux attentes d'un autre.
    #[test]
    fn pruning_one_account_leaves_the_others_alone() {
        let mut store = Store::in_memory().unwrap();
        let mut autre = operation("synth:autre", "2026-08-30", "-5", false);
        autre.account_id = "compte-2".to_string();
        store
            .upsert_transactions(&[operation("synth:mien", "2026-08-30", "-5", false), autre])
            .unwrap();

        assert_eq!(
            store
                .prune_stale_pending(COMPTE, None, &fresh(&[]))
                .unwrap(),
            1
        );
        assert_eq!(ids(&store), vec!["synth:autre"]);
    }
}
