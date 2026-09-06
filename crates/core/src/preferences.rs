//! Réglages modifiables depuis l'application web.
//!
//! Le serveur web ouvre la base des opérations **en lecture seule** : quatre
//! ans d'historique financier ne doivent pas être à la merci d'une faute de
//! programmation dans un service exposé.
//!
//! Classer une dépense ou régler un portefeuille demande pourtant d'écrire.
//! D'où ce magasin distinct, ouvert en écriture mais n'exposant **que** les
//! réglages de l'utilisateur : catégories, règles de classement, enveloppes.
//! Ce n'est pas une convention à respecter — les méthodes d'écriture sur les
//! opérations n'existent tout simplement pas ici, et le compilateur refuse de
//! les inventer.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::category::Category;
use crate::config::database_path;
use crate::wallets::Month;

/// Accès en écriture aux seules préférences de classement.
pub struct Preferences {
    conn: Connection,
}

/// Une catégorie telle que l'utilisateur l'a réglée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryDef {
    /// Identifiant stable. Ne change jamais, même si le nom change.
    pub key: String,
    pub label: String,
    /// Faux pour les mouvements qui ne consomment rien.
    pub is_spending: bool,
    /// Vraie pour les seize catégories d'origine, que des motifs automatiques
    /// désignent et qu'on ne peut donc pas supprimer sèchement.
    pub builtin: bool,
    /// Catégorie dont celle-ci relève, quand elle en relève d'une.
    ///
    /// « Essence » a « Transport » pour parent : chercher ou totaliser
    /// « Transport » embrasse alors « Essence », sans compter deux fois.
    pub parent: Option<String>,
}

/// Un portefeuille tel qu'il est réglé en base.
///
/// Distinct de [`crate::wallets::Wallet`], qui est la forme attendue par le
/// moteur de report : les catégories sont ici celles que l'utilisateur a
/// désignées, avant extension à leurs sous-catégories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletDef {
    pub id: i64,
    pub name: String,
    /// Somme dont l'enveloppe est dotée chaque mois.
    pub allocation: Decimal,
    /// Catégories désignées, telles quelles.
    pub categories: Vec<String>,
    /// Enveloppe qui reçoit le reliquat de fin de mois.
    pub carry_to: Option<i64>,
    /// Enveloppe qui contient celle-ci, et sur la dotation de laquelle elle
    /// est prise.
    pub parent: Option<i64>,
    /// Premier mois doté.
    pub start: Month,
}

/// Une ligne de la table des portefeuilles, avant mise en forme.
///
/// Nommée plutôt que laissée en tuple : six colonnes anonymes ne disent rien
/// de ce qu'elles portent.
struct WalletRow {
    id: i64,
    name: String,
    allocation: String,
    carry_to: Option<i64>,
    start: String,
    parent: Option<i64>,
}

/// Un apport ponctuel versé à une enveloppe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionDef {
    pub id: i64,
    pub wallet_id: i64,
    pub month: Month,
    /// Signé : négatif, l'apport est un retrait.
    pub amount: Decimal,
    pub note: Option<String>,
}

/// Une règle posée par l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryRule {
    /// Libellé nettoyé du bénéficiaire.
    pub label: String,
    /// Clé de la catégorie choisie.
    pub category: String,
}

/// Profondeur maximale d'emboîtement parcourue.
///
/// Assez pour toute organisation réelle, et borne les parcours ascendants
/// contre une boucle qu'une base abîmée pourrait contenir.
const MAX_DEPTH: usize = 8;

impl Preferences {
    /// Base en mémoire portant les seules tables que ce module manipule.
    ///
    /// Le schéma est recopié de la migration v4 plutôt que rejoué depuis
    /// elle : `Store` fait évoluer un fichier, pas une connexion fournie.
    /// Base en mémoire, au schéma réel.
    ///
    /// Le schéma est rejoué depuis les migrations, jamais recopié : une copie
    /// continuerait de passer le jour où le schéma change sous elle.
    #[cfg(test)]
    fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        crate::store::apply_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn open() -> Result<Self> {
        let path = database_path()?;
        let conn =
            Connection::open(&path).with_context(|| format!("ouverture de {}", path.display()))?;
        Ok(Self { conn })
    }

    /// Sème les catégories d'origine pour un utilisateur qui n'en a pas.
    ///
    /// L'ensemencement est paresseux plutôt que fait à la création du compte :
    /// un compte créé avant cette version n'aurait rien, et le rattraper
    /// demanderait une migration qui devine les utilisateurs.
    fn seed(&self, user_id: i64) -> Result<()> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM categories WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Ok(());
        }

        for (position, category) in Category::all().into_iter().enumerate() {
            self.conn.execute(
                "INSERT OR IGNORE INTO categories
                    (user_id, key, label, is_spending, builtin, position)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![
                    user_id,
                    category_key(category),
                    category.label(),
                    category.is_spending() as i32,
                    position as i64,
                ],
            )?;
        }
        Ok(())
    }

    /// Catégories visibles d'un utilisateur, dans l'ordre d'affichage.
    pub fn categories(&self, user_id: i64) -> Result<Vec<CategoryDef>> {
        self.seed(user_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT key, label, is_spending, builtin, parent FROM categories
             WHERE user_id = ?1 AND merged_into IS NULL
             ORDER BY position, label COLLATE NOCASE",
        )?;
        Ok(stmt
            .query_map([user_id], |row| {
                Ok(CategoryDef {
                    key: row.get(0)?,
                    label: row.get(1)?,
                    is_spending: row.get::<_, i32>(2)? != 0,
                    builtin: row.get::<_, i32>(3)? != 0,
                    parent: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Redirections en vigueur : catégorie retirée → catégorie d'accueil.
    ///
    /// Le classement automatique les applique après coup, ce qui évite de
    /// réécrire les motifs quand une catégorie est retirée.
    pub fn redirects(&self, user_id: i64) -> Result<HashMap<String, String>> {
        self.seed(user_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT key, merged_into FROM categories
             WHERE user_id = ?1 AND merged_into IS NOT NULL",
        )?;
        Ok(stmt
            .query_map([user_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?)
    }

    /// Crée une catégorie propre à l'utilisateur.
    ///
    /// Aucun motif automatique ne la désignera : elle ne s'obtient qu'en
    /// classant une opération à la main.
    pub fn add_category(
        &self,
        user_id: i64,
        label: &str,
        is_spending: bool,
        parent: Option<&str>,
    ) -> Result<String> {
        self.seed(user_id)?;
        let label = label.trim();
        if label.is_empty() {
            anyhow::bail!("le nom de la catégorie est vide");
        }

        let key = derive_key(label);
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM categories WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
            |row| row.get(0),
        )?;
        if exists > 0 {
            anyhow::bail!("une catégorie « {label} » existe déjà");
        }

        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM categories WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        let parent = parent.filter(|p| !p.is_empty());
        if let Some(parent) = parent
            && !self.category_exists(user_id, parent)?
        {
            anyhow::bail!("catégorie parente « {parent} » inconnue");
        }

        self.conn.execute(
            "INSERT INTO categories (user_id, key, label, is_spending, builtin, position, parent)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
            params![user_id, key, label, is_spending as i32, next, parent],
        )?;
        Ok(key)
    }

    /// Renomme une catégorie. L'identifiant reste inchangé, donc les règles
    /// déjà posées continuent de fonctionner.
    pub fn rename_category(&self, user_id: i64, key: &str, label: &str) -> Result<()> {
        let label = label.trim();
        if label.is_empty() {
            anyhow::bail!("le nom de la catégorie est vide");
        }
        let changed = self.conn.execute(
            "UPDATE categories SET label = ?3 WHERE user_id = ?1 AND key = ?2",
            params![user_id, key, label],
        )?;
        match changed {
            0 => anyhow::bail!("catégorie « {key} » inconnue"),
            _ => Ok(()),
        }
    }

    /// Rattache une catégorie à une autre, ou la ramène à la racine.
    ///
    /// « Essence » rattachée à « Transport » reste une catégorie à part
    /// entière : les opérations gardent leur classement précis, et c'est le
    /// parcours ascendant qui les fait aussi compter dans « Transport ».
    pub fn set_parent(&self, user_id: i64, key: &str, parent: Option<&str>) -> Result<()> {
        self.seed(user_id)?;
        if !self.category_exists(user_id, key)? {
            anyhow::bail!("catégorie « {key} » inconnue");
        }

        let parent = parent.filter(|p| !p.is_empty());
        if let Some(parent) = parent {
            if parent == key {
                anyhow::bail!("une catégorie ne peut pas relever d'elle-même");
            }
            if !self.category_exists(user_id, parent)? {
                anyhow::bail!("catégorie parente « {parent} » inconnue");
            }
            // Sans ce refus, la branche formerait une boucle : ses opérations
            // deviendraient introuvables et tout parcours ascendant tournerait
            // jusqu'à sa borne, à chaque affichage.
            if self.ancestors(user_id, parent)?.iter().any(|a| a == key) {
                anyhow::bail!("« {parent} » relève déjà de « {key} »");
            }
        }

        self.conn.execute(
            "UPDATE categories SET parent = ?3 WHERE user_id = ?1 AND key = ?2",
            params![user_id, key, parent],
        )?;
        Ok(())
    }

    /// Chaîne ascendante d'une catégorie, du parent direct vers la racine.
    ///
    /// Bornée en profondeur : une base abîmée par une écriture concurrente ne
    /// doit pas pouvoir faire tourner le serveur indéfiniment.
    fn ancestors(&self, user_id: i64, key: &str) -> Result<Vec<String>> {
        let mut chain: Vec<String> = Vec::new();
        let mut current = key.to_string();
        for _ in 0..MAX_DEPTH {
            let parent: Option<String> = self
                .conn
                .query_row(
                    "SELECT parent FROM categories WHERE user_id = ?1 AND key = ?2",
                    params![user_id, current],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            let Some(parent) = parent else { break };
            let seen = parent == key || chain.contains(&parent);
            chain.push(parent.clone());
            if seen {
                break;
            }
            current = parent;
        }
        Ok(chain)
    }

    /// Retire une catégorie.
    ///
    /// Une catégorie d'origine est **redirigée** vers `into` : ses motifs
    /// automatiques continuent de classer, mais aboutissent ailleurs. La
    /// supprimer sèchement ferait taire jusqu'à vingt-quatre motifs sans que
    /// rien ne le signale.
    ///
    /// Une catégorie créée par l'utilisateur n'a pas de motif : elle disparaît
    /// avec les règles qui la désignaient.
    pub fn remove_category(&self, user_id: i64, key: &str, into: Option<&str>) -> Result<()> {
        self.seed(user_id)?;

        // Les sous-catégories survivent au retrait de leur parent : les
        // laisser pointer sur une catégorie disparue les détacherait de
        // l'arbre, et leurs opérations cesseraient d'être totalisées.
        let inherited: Option<String> = self
            .conn
            .query_row(
                "SELECT parent FROM categories WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        let builtin: Option<i64> = self
            .conn
            .query_row(
                "SELECT builtin FROM categories WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
                |row| row.get(0),
            )
            .optional()?;

        let Some(builtin) = builtin else {
            anyhow::bail!("catégorie « {key} » inconnue");
        };

        if builtin != 0 {
            let into = into.filter(|t| *t != key).ok_or_else(|| {
                anyhow::anyhow!(
                    "cette catégorie porte des motifs automatiques : désigne celle qui doit les recevoir"
                )
            })?;
            let target: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM categories
                 WHERE user_id = ?1 AND key = ?2 AND merged_into IS NULL",
                params![user_id, into],
                |row| row.get(0),
            )?;
            if target == 0 {
                anyhow::bail!("catégorie d'accueil « {into} » inconnue");
            }

            self.conn.execute(
                "UPDATE categories SET merged_into = ?3 WHERE user_id = ?1 AND key = ?2",
                params![user_id, key, into],
            )?;
            // Les règles pointant sur la catégorie retirée suivent, sans quoi
            // elles désigneraient une catégorie invisible.
            self.conn.execute(
                "UPDATE category_rules SET category = ?3 WHERE user_id = ?1 AND category = ?2",
                params![user_id, key, into],
            )?;
            self.reparent_children(user_id, key, Some(into))?;
            return Ok(());
        }

        // Une catégorie créée ne porte aucun motif automatique : rien ne
        // subsiste qui aurait besoin d'être redirigé, elle disparaît vraiment.
        // Ses règles, elles, sont un travail manuel : les transférer quand une
        // catégorie d'accueil est désignée, les rendre au classement
        // automatique sinon.
        match into.filter(|t| *t != key) {
            Some(into) => {
                let target: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM categories
                     WHERE user_id = ?1 AND key = ?2 AND merged_into IS NULL",
                    params![user_id, into],
                    |row| row.get(0),
                )?;
                if target == 0 {
                    anyhow::bail!("catégorie d'accueil « {into} » inconnue");
                }
                self.conn.execute(
                    "UPDATE category_rules SET category = ?3 WHERE user_id = ?1 AND category = ?2",
                    params![user_id, key, into],
                )?;
            }
            None => {
                self.conn.execute(
                    "DELETE FROM category_rules WHERE user_id = ?1 AND category = ?2",
                    params![user_id, key],
                )?;
            }
        }
        // Faute de catégorie d'accueil, les enfants remontent d'un cran :
        // celle de leur grand-parent, ou la racine.
        self.reparent_children(user_id, key, into.or(inherited.as_deref()))?;
        self.conn.execute(
            "DELETE FROM categories WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
        )?;
        Ok(())
    }

    /// Reloge les sous-catégories d'une catégorie qui disparaît.
    fn reparent_children(&self, user_id: i64, key: &str, onto: Option<&str>) -> Result<()> {
        // `onto == key` arriverait si la catégorie d'accueil était la
        // catégorie retirée elle-même ; les enfants iraient alors à la racine.
        let onto = onto.filter(|t| *t != key);
        self.conn.execute(
            "UPDATE categories SET parent = ?3 WHERE user_id = ?1 AND parent = ?2",
            params![user_id, key, onto],
        )?;
        Ok(())
    }

    /// Règles d'un utilisateur : libellé replié → clé de catégorie.
    ///
    /// Le repli sert de clé pour que « Burger King » et « BURGER KING »
    /// partagent la même règle.
    ///
    /// La valeur est une **clé**, non une variante du type figé : sans quoi
    /// une catégorie créée par l'utilisateur ne pourrait pas être désignée.
    pub fn rules_for(&self, user_id: i64) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT label, category FROM category_rules WHERE user_id = ?1")?;
        let rows = stmt.query_map([user_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut rules = HashMap::new();
        for row in rows {
            let (label, category) = row?;
            rules.insert(crate::merchant::fold(&label), category);
        }
        Ok(rules)
    }

    /// Vrai si cette catégorie existe pour cet utilisateur et n'est pas retirée.
    ///
    /// Sert à valider une règle : accepter une clé inconnue laisserait une
    /// règle inapplicable en base, invisible et sans effet.
    pub fn category_exists(&self, user_id: i64, key: &str) -> Result<bool> {
        self.seed(user_id)?;
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM categories
             WHERE user_id = ?1 AND key = ?2 AND merged_into IS NULL",
            params![user_id, key],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Règles d'un utilisateur, dans l'ordre alphabétique, pour l'affichage.
    pub fn list(&self, user_id: i64) -> Result<Vec<CategoryRule>> {
        let mut stmt = self.conn.prepare(
            "SELECT label, category FROM category_rules
             WHERE user_id = ?1 ORDER BY label COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([user_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows
            .into_iter()
            .map(|(label, category)| CategoryRule { label, category })
            .collect())
    }

    /// Pose ou remplace la règle d'un libellé.
    pub fn set(&self, user_id: i64, label: &str, category: &str) -> Result<()> {
        let label = label.trim();
        if label.is_empty() {
            anyhow::bail!("libellé vide");
        }
        if !self.category_exists(user_id, category)? {
            anyhow::bail!("catégorie « {category} » inconnue");
        }
        self.conn.execute(
            "INSERT INTO category_rules (user_id, label, category, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, label) DO UPDATE SET
                category   = excluded.category,
                updated_at = excluded.updated_at",
            params![user_id, label, category, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Retire une règle, ce qui rend l'opération au classement automatique.
    pub fn remove(&self, user_id: i64, label: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM category_rules WHERE user_id = ?1 AND label = ?2",
            params![user_id, label.trim()],
        )?;
        Ok(())
    }
}

/// Fabrique un identifiant à partir d'un nom.
///
/// Sans accents ni espaces, pour rester lisible dans les échanges et les URL.
/// Le préfixe évite qu'une catégorie nommée « Logement » entre en collision
/// avec la catégorie d'origine du même nom.
fn derive_key(label: &str) -> String {
    let simplified: String = crate::merchant::fold(label)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("u_{}", simplified.trim_matches('_').to_lowercase())
}

/// Nom stable d'une catégorie en base.
///
/// Volontairement découplé de l'affichage : renommer « Logement » en français
/// ne doit pas invalider les règles déjà enregistrées.
pub fn category_key(category: Category) -> &'static str {
    match category {
        Category::Housing => "housing",
        Category::Groceries => "groceries",
        Category::Dining => "dining",
        Category::Transport => "transport",
        Category::Health => "health",
        Category::Shopping => "shopping",
        Category::Subscriptions => "subscriptions",
        Category::Leisure => "leisure",
        Category::Gambling => "gambling",
        Category::Banking => "banking",
        Category::Credit => "credit",
        Category::Cash => "cash",
        Category::Transfer => "transfer",
        Category::Investment => "investment",
        Category::Income => "income",
        Category::Uncategorised => "uncategorised",
    }
}

impl Preferences {
    /// Portefeuilles d'un utilisateur, dans l'ordre d'affichage.
    pub fn wallets(&self, user_id: i64) -> Result<Vec<WalletDef>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, allocation, carry_to, start_month, parent FROM wallets
             WHERE user_id = ?1
             ORDER BY position, name COLLATE NOCASE",
        )?;
        let rows: Vec<WalletRow> = stmt
            .query_map([user_id], |row| {
                Ok(WalletRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    allocation: row.get(2)?,
                    carry_to: row.get(3)?,
                    start: row.get(4)?,
                    parent: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut wallets = Vec::with_capacity(rows.len());
        for row in rows {
            let WalletRow {
                id,
                name,
                allocation,
                carry_to,
                start,
                parent,
            } = row;
            wallets.push(WalletDef {
                id,
                name,
                // Un montant illisible vaut zéro plutôt que de faire échouer
                // tout l'écran : l'enveloppe reste visible et corrigeable.
                allocation: Decimal::from_str(&allocation).unwrap_or(Decimal::ZERO),
                categories: self.wallet_categories(id)?,
                carry_to,
                parent,
                start: Month::parse(&start).unwrap_or_else(|| Month::new(1970, 1)),
            });
        }
        Ok(wallets)
    }

    /// Catégories rattachées à un portefeuille.
    fn wallet_categories(&self, wallet_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT category FROM wallet_categories WHERE wallet_id = ?1 ORDER BY category",
        )?;
        Ok(stmt
            .query_map([wallet_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Crée un portefeuille et rend son identifiant.
    ///
    /// Il est doté à partir du mois en cours : le doter rétroactivement
    /// inventerait des reliquats qui n'ont jamais existé.
    pub fn add_wallet(
        &self,
        user_id: i64,
        name: &str,
        allocation: Decimal,
        start: Month,
    ) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("le nom du portefeuille est vide");
        }

        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM wallets WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        self.conn.execute(
            "INSERT INTO wallets (user_id, name, allocation, carry_to, start_month, position)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![
                user_id,
                name,
                allocation.to_string(),
                start.to_string(),
                next
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Modifie le nom, la dotation et la destination du reliquat.
    pub fn update_wallet(
        &self,
        user_id: i64,
        id: i64,
        name: &str,
        allocation: Decimal,
        carry_to: Option<i64>,
    ) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("le nom du portefeuille est vide");
        }
        // Se désigner soi-même est sans effet — le reliquat reste sur place —
        // mais l'enregistrer laisserait croire à une redirection active.
        let carry_to = carry_to.filter(|target| *target != id);
        if let Some(target) = carry_to
            && !self.wallet_exists(user_id, target)?
        {
            anyhow::bail!("portefeuille destinataire inconnu");
        }

        let changed = self.conn.execute(
            "UPDATE wallets SET name = ?3, allocation = ?4, carry_to = ?5
             WHERE user_id = ?1 AND id = ?2",
            params![user_id, id, name, allocation.to_string(), carry_to],
        )?;
        match changed {
            0 => anyhow::bail!("portefeuille inconnu"),
            _ => Ok(()),
        }
    }

    /// Remplace les catégories rattachées à un portefeuille.
    pub fn set_wallet_categories(
        &self,
        user_id: i64,
        id: i64,
        categories: &[String],
    ) -> Result<()> {
        if !self.wallet_exists(user_id, id)? {
            anyhow::bail!("portefeuille inconnu");
        }
        for category in categories {
            if !self.category_exists(user_id, category)? {
                anyhow::bail!("catégorie « {category} » inconnue");
            }
        }

        self.conn.execute(
            "DELETE FROM wallet_categories WHERE wallet_id = ?1",
            params![id],
        )?;
        for category in categories {
            self.conn.execute(
                "INSERT OR IGNORE INTO wallet_categories (wallet_id, category) VALUES (?1, ?2)",
                params![id, category],
            )?;
        }
        Ok(())
    }

    /// Supprime un portefeuille, ses rattachements et les redirections vers lui.
    pub fn remove_wallet(&self, user_id: i64, id: i64) -> Result<()> {
        if !self.wallet_exists(user_id, id)? {
            anyhow::bail!("portefeuille inconnu");
        }
        // Sans cela, une enveloppe continuerait de diriger son reliquat vers
        // une destination disparue : il resterait sur place, mais l'écran
        // afficherait une redirection qui n'existe plus.
        self.conn.execute(
            "UPDATE wallets SET carry_to = NULL WHERE user_id = ?1 AND carry_to = ?2",
            params![user_id, id],
        )?;
        // Les filles remontent d'un cran plutôt que de désigner une mère
        // disparue : leur dotation cesserait sinon d'être prise sur quoi que
        // ce soit, et la branche perdrait son budget.
        let grand_parent: Option<i64> = self
            .conn
            .query_row(
                "SELECT parent FROM wallets WHERE user_id = ?1 AND id = ?2",
                params![user_id, id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        self.conn.execute(
            "UPDATE wallets SET parent = ?3 WHERE user_id = ?1 AND parent = ?2",
            params![user_id, id, grand_parent],
        )?;
        self.conn.execute(
            "DELETE FROM wallet_categories WHERE wallet_id = ?1",
            params![id],
        )?;
        // Les apports partent avec elle : conservés, ils resteraient invisibles
        // tout en continuant de peser sur les totaux.
        self.conn.execute(
            "DELETE FROM wallet_contributions WHERE user_id = ?1 AND wallet_id = ?2",
            params![user_id, id],
        )?;
        self.conn.execute(
            "DELETE FROM wallets WHERE user_id = ?1 AND id = ?2",
            params![user_id, id],
        )?;
        Ok(())
    }

    /// Range une enveloppe dans une autre, ou la ramène au premier rang.
    ///
    /// La mère dote alors sa fille : sa dotation devient le budget de la
    /// branche. Rien n'est vérifié sur les montants — une mère peut se laisser
    /// déborder par ses filles, et l'affichage le signale plutôt que
    /// d'interdire une répartition que l'utilisateur ajustera après coup.
    pub fn set_wallet_parent(&self, user_id: i64, id: i64, parent: Option<i64>) -> Result<()> {
        if !self.wallet_exists(user_id, id)? {
            anyhow::bail!("portefeuille inconnu");
        }

        if let Some(parent) = parent {
            if parent == id {
                anyhow::bail!("un portefeuille ne peut pas se contenir lui-même");
            }
            if !self.wallet_exists(user_id, parent)? {
                anyhow::bail!("portefeuille contenant inconnu");
            }
            // Une boucle rendrait la dotation de la branche indéfinissable :
            // chaque enveloppe se doterait à partir de l'autre.
            if self.wallet_ancestors(user_id, parent)?.contains(&id) {
                anyhow::bail!("ce portefeuille est déjà contenu dans l'autre");
            }
        }

        self.conn.execute(
            "UPDATE wallets SET parent = ?3 WHERE user_id = ?1 AND id = ?2",
            params![user_id, id, parent],
        )?;
        Ok(())
    }

    /// Chaîne des enveloppes contenantes, de la plus proche vers le premier rang.
    fn wallet_ancestors(&self, user_id: i64, id: i64) -> Result<Vec<i64>> {
        let mut chain = Vec::new();
        let mut current = id;
        for _ in 0..MAX_DEPTH {
            let parent: Option<i64> = self
                .conn
                .query_row(
                    "SELECT parent FROM wallets WHERE user_id = ?1 AND id = ?2",
                    params![user_id, current],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            let Some(parent) = parent else { break };
            let seen = parent == id || chain.contains(&parent);
            chain.push(parent);
            if seen {
                break;
            }
            current = parent;
        }
        Ok(chain)
    }

    /// Apports ponctuels d'un utilisateur, du plus récent au plus ancien.
    pub fn contributions(&self, user_id: i64) -> Result<Vec<ContributionDef>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, wallet_id, month, amount, note FROM wallet_contributions
             WHERE user_id = ?1
             ORDER BY month DESC, id DESC",
        )?;
        let rows: Vec<(i64, i64, String, String, Option<String>)> = stmt
            .query_map([user_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows
            .into_iter()
            .filter_map(|(id, wallet_id, month, amount, note)| {
                // Une ligne illisible est écartée plutôt que de fausser un
                // solde en silence : mieux vaut un apport manquant, visible,
                // qu'un total juste en apparence.
                Some(ContributionDef {
                    id,
                    wallet_id,
                    month: Month::parse(&month)?,
                    amount: Decimal::from_str(&amount).ok()?,
                    note,
                })
            })
            .collect())
    }

    /// Verse un apport ponctuel à une enveloppe, et rend son identifiant.
    pub fn add_contribution(
        &self,
        user_id: i64,
        wallet_id: i64,
        month: Month,
        amount: Decimal,
        note: Option<&str>,
    ) -> Result<i64> {
        if !self.wallet_exists(user_id, wallet_id)? {
            anyhow::bail!("portefeuille inconnu");
        }
        // Un apport nul n'apporte rien et encombrerait l'historique.
        if amount.is_zero() {
            anyhow::bail!("un apport ne peut pas être nul");
        }

        let note = note.map(str::trim).filter(|n| !n.is_empty());
        self.conn.execute(
            "INSERT INTO wallet_contributions
                (user_id, wallet_id, month, amount, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                user_id,
                wallet_id,
                month.to_string(),
                amount.to_string(),
                note,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Annule un apport ponctuel.
    pub fn remove_contribution(&self, user_id: i64, id: i64) -> Result<()> {
        let removed = self.conn.execute(
            "DELETE FROM wallet_contributions WHERE user_id = ?1 AND id = ?2",
            params![user_id, id],
        )?;
        match removed {
            0 => anyhow::bail!("apport inconnu"),
            _ => Ok(()),
        }
    }

    fn wallet_exists(&self, user_id: i64, id: i64) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM wallets WHERE user_id = ?1 AND id = ?2",
            params![user_id, id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Étend un ensemble de catégories à tout ce qu'elles contiennent.
    ///
    /// Une enveloppe rattachée à « Transport » doit compter « Essence » et
    /// « Péage » : sans cette extension, classer plus finement ferait
    /// silencieusement sortir des dépenses de l'enveloppe qui les surveillait.
    pub fn branch_of(&self, user_id: i64, keys: &[String]) -> Result<HashSet<String>> {
        let wanted: HashSet<&String> = keys.iter().collect();
        let categories = self.categories(user_id)?;
        let parents: HashMap<String, String> = categories
            .iter()
            .filter_map(|c| c.parent.clone().map(|p| (c.key.clone(), p)))
            .collect();

        let mut branch: HashSet<String> = keys.iter().cloned().collect();
        for category in &categories {
            let mut current = category.key.clone();
            // Bornée comme les autres parcours ascendants : une base abîmée ne
            // doit pas faire tourner le serveur à chaque affichage.
            for _ in 0..MAX_DEPTH {
                let Some(parent) = parents.get(&current) else {
                    break;
                };
                if wanted.contains(parent) {
                    branch.insert(category.key.clone());
                    break;
                }
                current = parent.clone();
            }
        }
        Ok(branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOI: i64 = 1;

    /// Le défaut qui a fait échouer l'association : une règle doit pouvoir
    /// désigner une catégorie créée, qu'aucune variante interne ne représente.
    #[test]
    fn a_rule_can_target_a_user_created_category() {
        let prefs = Preferences::in_memory().unwrap();
        let cle = prefs.add_category(MOI, "Cadeaux", true, None).unwrap();
        prefs.set(MOI, "CDIL", &cle).unwrap();

        let regles = prefs.rules_for(MOI).unwrap();
        assert_eq!(regles.get(&crate::merchant::fold("CDIL")), Some(&cle));
    }

    /// Une catégorie inconnue est refusée : l'accepter laisserait en base une
    /// règle sans effet, invisible à l'utilisateur qui la croirait posée.
    #[test]
    fn a_rule_towards_an_unknown_category_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        assert!(prefs.set(MOI, "CDIL", "charabia").is_err());
    }

    /// Retirer une catégorie créée en désignant une catégorie d'accueil
    /// transfère ses règles : elles sont un travail manuel, pas un résidu.
    #[test]
    fn removing_a_user_category_into_another_carries_its_rules() {
        let prefs = Preferences::in_memory().unwrap();
        let cle = prefs.add_category(MOI, "Cadeaux", true, None).unwrap();
        prefs.set(MOI, "CDIL", &cle).unwrap();

        prefs.remove_category(MOI, &cle, Some("shopping")).unwrap();

        let regles = prefs.rules_for(MOI).unwrap();
        assert_eq!(
            regles.get(&crate::merchant::fold("CDIL")),
            Some(&"shopping".to_string())
        );
    }

    /// Sans catégorie d'accueil, le libellé revient au classement automatique.
    #[test]
    fn removing_a_user_category_outright_drops_its_rules() {
        let prefs = Preferences::in_memory().unwrap();
        let cle = prefs.add_category(MOI, "Cadeaux", true, None).unwrap();
        prefs.set(MOI, "CDIL", &cle).unwrap();

        prefs.remove_category(MOI, &cle, None).unwrap();

        assert!(prefs.rules_for(MOI).unwrap().is_empty());
    }

    /// Une catégorie d'accueil inexistante ne doit pas emporter les règles
    /// avec elle : le retrait échoue, et rien n'est perdu.
    #[test]
    fn removing_into_an_unknown_category_keeps_everything() {
        let prefs = Preferences::in_memory().unwrap();
        let cle = prefs.add_category(MOI, "Cadeaux", true, None).unwrap();
        prefs.set(MOI, "CDIL", &cle).unwrap();

        assert!(prefs.remove_category(MOI, &cle, Some("charabia")).is_err());
        assert_eq!(prefs.rules_for(MOI).unwrap().len(), 1);
    }

    fn parent_de(prefs: &Preferences, cle: &str) -> Option<String> {
        prefs
            .categories(MOI)
            .unwrap()
            .into_iter()
            .find(|c| c.key == cle)
            .and_then(|c| c.parent)
    }

    /// Une catégorie créée peut relever d'une autre.
    #[test]
    fn a_category_can_be_nested_under_another() {
        let prefs = Preferences::in_memory().unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some("transport"))
            .unwrap();
        assert_eq!(parent_de(&prefs, &essence).as_deref(), Some("transport"));
    }

    /// Un parent inexistant est refusé : l'accepter détacherait la catégorie
    /// de l'arbre sans que rien ne le signale.
    #[test]
    fn nesting_under_an_unknown_category_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        assert!(
            prefs
                .add_category(MOI, "Essence", true, Some("charabia"))
                .is_err()
        );
    }

    /// Le rattachement se change après coup, et se défait.
    #[test]
    fn nesting_can_be_changed_and_undone() {
        let prefs = Preferences::in_memory().unwrap();
        let essence = prefs.add_category(MOI, "Essence", true, None).unwrap();

        prefs.set_parent(MOI, &essence, Some("transport")).unwrap();
        assert_eq!(parent_de(&prefs, &essence).as_deref(), Some("transport"));

        prefs.set_parent(MOI, &essence, None).unwrap();
        assert_eq!(parent_de(&prefs, &essence), None);
    }

    /// Une boucle rendrait les opérations de la branche introuvables.
    #[test]
    fn a_cycle_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some("transport"))
            .unwrap();

        assert!(prefs.set_parent(MOI, "transport", Some(&essence)).is_err());
        assert!(prefs.set_parent(MOI, &essence, Some(&essence)).is_err());
    }

    /// Y compris une boucle indirecte, à trois niveaux.
    #[test]
    fn an_indirect_cycle_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some("transport"))
            .unwrap();
        let diesel = prefs
            .add_category(MOI, "Diesel", true, Some(&essence))
            .unwrap();

        assert!(prefs.set_parent(MOI, "transport", Some(&diesel)).is_err());
    }

    /// Retirer un parent ne doit pas détacher ses sous-catégories : elles
    /// rejoignent la catégorie d'accueil.
    #[test]
    fn removing_a_parent_rehomes_its_children() {
        let prefs = Preferences::in_memory().unwrap();
        let carburant = prefs.add_category(MOI, "Carburant", true, None).unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some(&carburant))
            .unwrap();

        prefs
            .remove_category(MOI, &carburant, Some("transport"))
            .unwrap();
        assert_eq!(parent_de(&prefs, &essence).as_deref(), Some("transport"));
    }

    /// Sans catégorie d'accueil, elles remontent d'un cran plutôt que de
    /// désigner une catégorie disparue.
    #[test]
    fn without_a_target_children_move_up_one_level() {
        let prefs = Preferences::in_memory().unwrap();
        let carburant = prefs
            .add_category(MOI, "Carburant", true, Some("transport"))
            .unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some(&carburant))
            .unwrap();

        prefs.remove_category(MOI, &carburant, None).unwrap();
        assert_eq!(parent_de(&prefs, &essence).as_deref(), Some("transport"));
    }

    fn portefeuille(prefs: &Preferences, nom: &str, dotation: &str) -> i64 {
        prefs
            .add_wallet(
                MOI,
                nom,
                Decimal::from_str(dotation).unwrap(),
                Month::parse("2026-01").unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn a_wallet_is_created_and_read_back() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Alimentation", "300");
        prefs
            .set_wallet_categories(MOI, id, &["groceries".to_string(), "dining".to_string()])
            .unwrap();

        let wallets = prefs.wallets(MOI).unwrap();
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].name, "Alimentation");
        assert_eq!(wallets[0].allocation, Decimal::from_str("300").unwrap());
        assert_eq!(wallets[0].categories, vec!["dining", "groceries"]);
        assert_eq!(wallets[0].carry_to, None);
    }

    /// Rattacher à une catégorie inexistante laisserait une enveloppe qui ne
    /// verrait jamais passer la moindre dépense.
    #[test]
    fn attaching_an_unknown_category_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Alimentation", "300");
        assert!(
            prefs
                .set_wallet_categories(MOI, id, &["charabia".to_string()])
                .is_err()
        );
    }

    #[test]
    fn the_categories_of_a_wallet_are_replaced_wholesale() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Alimentation", "300");
        prefs
            .set_wallet_categories(MOI, id, &["groceries".to_string()])
            .unwrap();
        prefs
            .set_wallet_categories(MOI, id, &["dining".to_string()])
            .unwrap();

        assert_eq!(prefs.wallets(MOI).unwrap()[0].categories, vec!["dining"]);
    }

    /// Se désigner soi-même n'est pas une redirection : l'enregistrer ferait
    /// afficher un renvoi qui n'a aucun effet.
    #[test]
    fn a_wallet_cannot_redirect_to_itself() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "A", "300");
        prefs
            .update_wallet(MOI, id, "A", Decimal::from_str("300").unwrap(), Some(id))
            .unwrap();

        assert_eq!(prefs.wallets(MOI).unwrap()[0].carry_to, None);
    }

    #[test]
    fn redirecting_to_an_unknown_wallet_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "A", "300");
        assert!(
            prefs
                .update_wallet(MOI, id, "A", Decimal::from_str("300").unwrap(), Some(404))
                .is_err()
        );
    }

    /// Supprimer une destination ne doit pas laisser un renvoi pendant.
    #[test]
    fn removing_a_wallet_clears_the_redirects_towards_it() {
        let prefs = Preferences::in_memory().unwrap();
        let a = portefeuille(&prefs, "A", "300");
        let b = portefeuille(&prefs, "B", "200");
        prefs
            .update_wallet(MOI, a, "A", Decimal::from_str("300").unwrap(), Some(b))
            .unwrap();

        prefs.remove_wallet(MOI, b).unwrap();

        let wallets = prefs.wallets(MOI).unwrap();
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].carry_to, None);
    }

    #[test]
    fn a_wallet_can_be_nested_in_another() {
        let prefs = Preferences::in_memory().unwrap();
        let mere = portefeuille(&prefs, "Vie courante", "500");
        let fille = portefeuille(&prefs, "Alimentation", "300");

        prefs.set_wallet_parent(MOI, fille, Some(mere)).unwrap();

        let wallets = prefs.wallets(MOI).unwrap();
        let fille = wallets.iter().find(|w| w.id == fille).unwrap();
        assert_eq!(fille.parent, Some(mere));
    }

    #[test]
    fn a_wallet_cannot_contain_itself() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "A", "300");
        assert!(prefs.set_wallet_parent(MOI, id, Some(id)).is_err());
    }

    /// Une boucle rendrait la dotation de la branche indéfinissable : chaque
    /// enveloppe se doterait à partir de l'autre.
    #[test]
    fn a_nesting_cycle_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let a = portefeuille(&prefs, "A", "500");
        let b = portefeuille(&prefs, "B", "300");
        prefs.set_wallet_parent(MOI, b, Some(a)).unwrap();

        assert!(prefs.set_wallet_parent(MOI, a, Some(b)).is_err());
    }

    #[test]
    fn an_indirect_nesting_cycle_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let a = portefeuille(&prefs, "A", "500");
        let b = portefeuille(&prefs, "B", "300");
        let c = portefeuille(&prefs, "C", "100");
        prefs.set_wallet_parent(MOI, b, Some(a)).unwrap();
        prefs.set_wallet_parent(MOI, c, Some(b)).unwrap();

        assert!(prefs.set_wallet_parent(MOI, a, Some(c)).is_err());
    }

    #[test]
    fn nesting_in_an_unknown_wallet_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "A", "300");
        assert!(prefs.set_wallet_parent(MOI, id, Some(404)).is_err());
    }

    /// Supprimer une mère ne doit pas détacher ses filles : leur dotation
    /// cesserait d'être prise sur quoi que ce soit.
    #[test]
    fn removing_a_mother_lifts_her_children_one_level() {
        let prefs = Preferences::in_memory().unwrap();
        let grand = portefeuille(&prefs, "Tout", "800");
        let mere = portefeuille(&prefs, "Vie courante", "500");
        let fille = portefeuille(&prefs, "Alimentation", "300");
        prefs.set_wallet_parent(MOI, mere, Some(grand)).unwrap();
        prefs.set_wallet_parent(MOI, fille, Some(mere)).unwrap();

        prefs.remove_wallet(MOI, mere).unwrap();

        let wallets = prefs.wallets(MOI).unwrap();
        let fille = wallets.iter().find(|w| w.id == fille).unwrap();
        assert_eq!(fille.parent, Some(grand));
    }

    #[test]
    fn removing_a_first_rank_mother_leaves_her_children_at_the_top() {
        let prefs = Preferences::in_memory().unwrap();
        let mere = portefeuille(&prefs, "Vie courante", "500");
        let fille = portefeuille(&prefs, "Alimentation", "300");
        prefs.set_wallet_parent(MOI, fille, Some(mere)).unwrap();

        prefs.remove_wallet(MOI, mere).unwrap();

        let wallets = prefs.wallets(MOI).unwrap();
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].parent, None);
    }

    #[test]
    fn a_contribution_is_recorded_and_read_back() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Vacances", "100");

        prefs
            .add_contribution(
                MOI,
                id,
                Month::parse("2026-01").unwrap(),
                Decimal::from_str("500").unwrap(),
                Some("prime"),
            )
            .unwrap();

        let apports = prefs.contributions(MOI).unwrap();
        assert_eq!(apports.len(), 1);
        assert_eq!(apports[0].wallet_id, id);
        assert_eq!(apports[0].amount, Decimal::from_str("500").unwrap());
        assert_eq!(apports[0].note.as_deref(), Some("prime"));
    }

    #[test]
    fn a_contribution_to_an_unknown_wallet_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        assert!(
            prefs
                .add_contribution(
                    MOI,
                    404,
                    Month::parse("2026-01").unwrap(),
                    Decimal::from_str("500").unwrap(),
                    None,
                )
                .is_err()
        );
    }

    /// Un apport nul n'apporte rien et encombrerait l'historique.
    #[test]
    fn a_null_contribution_is_refused() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Vacances", "100");
        assert!(
            prefs
                .add_contribution(MOI, id, Month::parse("2026-01").unwrap(), Decimal::ZERO, None)
                .is_err()
        );
    }

    #[test]
    fn a_contribution_can_be_cancelled() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Vacances", "100");
        let apport = prefs
            .add_contribution(
                MOI,
                id,
                Month::parse("2026-01").unwrap(),
                Decimal::from_str("500").unwrap(),
                None,
            )
            .unwrap();

        prefs.remove_contribution(MOI, apport).unwrap();

        assert!(prefs.contributions(MOI).unwrap().is_empty());
        assert!(prefs.remove_contribution(MOI, apport).is_err());
    }

    /// Conservés après la disparition de leur enveloppe, les apports
    /// resteraient invisibles tout en pesant sur les totaux.
    #[test]
    fn removing_a_wallet_takes_its_contributions_with_it() {
        let prefs = Preferences::in_memory().unwrap();
        let id = portefeuille(&prefs, "Vacances", "100");
        prefs
            .add_contribution(
                MOI,
                id,
                Month::parse("2026-01").unwrap(),
                Decimal::from_str("500").unwrap(),
                None,
            )
            .unwrap();

        prefs.remove_wallet(MOI, id).unwrap();

        assert!(prefs.contributions(MOI).unwrap().is_empty());
    }

    /// Une enveloppe rattachée à « Transport » doit compter « Essence » : sans
    /// cela, classer plus finement ferait sortir des dépenses de l'enveloppe.
    #[test]
    fn a_branch_covers_the_subcategories() {
        let prefs = Preferences::in_memory().unwrap();
        let essence = prefs
            .add_category(MOI, "Essence", true, Some("transport"))
            .unwrap();
        let diesel = prefs
            .add_category(MOI, "Diesel", true, Some(&essence))
            .unwrap();

        let branch = prefs.branch_of(MOI, &["transport".to_string()]).unwrap();

        assert!(branch.contains("transport"));
        assert!(branch.contains(&essence));
        assert!(branch.contains(&diesel));
        assert!(!branch.contains("groceries"));
    }

    #[test]
    fn a_branch_of_a_leaf_is_itself() {
        let prefs = Preferences::in_memory().unwrap();
        prefs.seed(MOI).unwrap();
        let branch = prefs.branch_of(MOI, &["groceries".to_string()]).unwrap();
        assert_eq!(branch, HashSet::from(["groceries".to_string()]));
    }

    /// Retirer une catégorie d'origine la redirige au lieu de l'effacer : ses
    /// 158 motifs automatiques restent écrits en Rust et continuent de classer.
    #[test]
    fn removing_a_built_in_category_redirects_it() {
        let prefs = Preferences::in_memory().unwrap();
        prefs
            .remove_category(MOI, "dining", Some("groceries"))
            .unwrap();

        assert_eq!(
            prefs.redirects(MOI).unwrap().get("dining"),
            Some(&"groceries".to_string())
        );
    }

    /// Chaque catégorie d'origine a une clé, et deux n'en partagent jamais
    /// une : une collision ferait silencieusement classer sous la mauvaise.
    #[test]
    fn every_built_in_category_has_a_distinct_key() {
        let mut cles: Vec<&str> = Category::all().into_iter().map(category_key).collect();
        let total = cles.len();
        cles.sort_unstable();
        cles.dedup();
        assert_eq!(cles.len(), total);
    }

    /// Les clés en base ne doivent pas suivre les libellés d'affichage : les
    /// traduire invaliderait les règles enregistrées.
    #[test]
    fn keys_are_stable_identifiers() {
        assert_eq!(category_key(Category::Housing), "housing");
        assert_ne!(category_key(Category::Housing), Category::Housing.label());
    }
}
