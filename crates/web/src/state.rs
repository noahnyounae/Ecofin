//! État partagé entre les gestionnaires de requêtes.

use anyhow::Result;
use ecofin_core::Store;
use ecofin_core::preferences::Preferences;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::access::Throttle;

/// La base SQLite est ouverte une fois et partagée.
///
/// Une seule connexion sous mutex : le trafic est celui d'un usage personnel,
/// et les requêtes sont brèves. Un pool ne s'imposera que si des traitements
/// longs apparaissent.
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    /// Secret de signature des sessions, lu au démarrage.
    ///
    /// Le serveur monte la base en lecture seule : il ne peut pas le créer.
    /// C'est `ecofin user add`, côté CLI, qui s'en charge.
    session_secret: Option<String>,
    /// Marquer les cookies `Secure` suppose HTTPS ; en local, cela empêcherait
    /// le navigateur de les renvoyer et donc de se connecter.
    secure_cookies: bool,
    throttle: Throttle,
    /// Accès en écriture aux seules préférences de classement.
    ///
    /// Distinct du magasin des opérations, qui reste en lecture seule : ce
    /// type n'expose aucune méthode capable de toucher aux données bancaires.
    preferences: Arc<Mutex<Preferences>>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        // En lecture seule : le serveur ne doit pouvoir corrompre aucune
        // donnée, quelle que soit la faute de programmation.
        let store = Store::open_read_only()?;
        let session_secret = store.session_secret()?;

        if session_secret.is_none() {
            tracing::warn!(
                "aucun secret de session en base : la connexion échouera. \
                 Crée un compte avec `ecofin user add <adresse>` depuis le CLI."
            );
        }

        // Derrière le tunnel, le trafic est en HTTPS. En développement local,
        // il ne l'est pas : d'où ce réglage explicite.
        let secure_cookies = std::env::var("ECOFIN_INSECURE_COOKIES")
            .map(|v| !matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(true);

        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            session_secret,
            secure_cookies,
            throttle: Throttle::default(),
            preferences: Arc::new(Mutex::new(Preferences::open()?)),
        })
    }

    /// Emprunte la base le temps d'une opération.
    ///
    /// Un mutex empoisonné signale qu'un gestionnaire a paniqué en le tenant.
    /// La base n'en est pas corrompue pour autant — SQLite gère ses propres
    /// transactions — donc on reprend la main plutôt que de propager la panique
    /// à toutes les requêtes suivantes.
    pub fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut guard = self.store.lock().unwrap_or_else(|poisoned| {
            tracing::error!("verrou de la base empoisonné par une panique antérieure");
            poisoned.into_inner()
        });
        f(&mut guard)
    }

    pub fn session_secret(&self) -> Option<String> {
        self.session_secret.clone()
    }

    pub fn secure_cookies(&self) -> bool {
        self.secure_cookies
    }

    pub fn throttle(&self) -> &Throttle {
        &self.throttle
    }

    /// Emprunte les préférences le temps d'une opération.
    pub fn with_preferences<T>(&self, f: impl FnOnce(&Preferences) -> Result<T>) -> Result<T> {
        let guard = self
            .preferences
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&guard)
    }

    /// Règles de classement d'un utilisateur, prêtes à être appliquées.
    ///
    /// Libellé replié → clé de catégorie ; une clé peut désigner une catégorie
    /// créée par l'utilisateur, qu'aucune variante interne ne représente.
    pub fn rules_for(&self, user_id: i64) -> Result<HashMap<String, String>> {
        self.with_preferences(|prefs| prefs.rules_for(user_id))
    }
}
