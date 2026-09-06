//! Emplacements sur disque et stockage des identifiants API.
//!
//! La clé privée qui signe les JWT est copiée dans le répertoire de
//! configuration en 0600, et on refuse de la lire si ses droits sont plus
//! permissifs que ça.

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const QUALIFIER: &str = "";
const ORGANIZATION: &str = "";
const APPLICATION: &str = "ecofin";

/// URL de retour proposée par défaut.
///
/// Enable Banking refuse les URL en `http://` : cette valeur ne conviendra donc
/// que pour un fournisseur qui accepte une boucle locale. Avec Enable Banking,
/// déclare une URL `https://` que tu contrôles et indique-la à `auth login` ;
/// le CLI demandera alors de recopier l'URL de retour au lieu de l'intercepter.
pub const DEFAULT_REDIRECT_URL: &str = "http://localhost:8484/callback";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub enable_banking: Option<EnableBankingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnableBankingConfig {
    /// Identifiant d'application, qui sert de `kid` dans l'en-tête JWT.
    pub application_id: String,
    /// Chemin vers la clé privée RSA au format PEM.
    pub private_key_path: PathBuf,
    /// URL de retour déclarée côté Enable Banking.
    #[serde(default = "default_redirect")]
    pub redirect_url: String,

    /// Adresse `hôte:port` sur laquelle écouter le retour d'autorisation.
    ///
    /// À renseigner quand l'URL de retour est publique mais aboutit malgré tout
    /// à cette machine — un tunnel Cloudflare vers `localhost`, par exemple.
    /// Sans elle, une URL de retour publique fait basculer en saisie manuelle.
    ///
    /// Dans un conteneur, il faut écouter sur `0.0.0.0:8484` pour que le port
    /// publié soit joignable : `127.0.0.1` ne verrait rien venir de l'extérieur.
    #[serde(default)]
    pub listen: Option<String>,
}

fn default_redirect() -> String {
    DEFAULT_REDIRECT_URL.to_string()
}

/// Lit une variable d'environnement, en traitant la chaîne vide comme absente.
///
/// Docker Compose transmet une variable non renseignée comme chaîne vide plutôt
/// que de l'omettre : sans ce filtre, un `.env` incomplet écraserait le fichier
/// de configuration avec du vide.
fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl EnableBankingConfig {
    /// Lit la clé privée depuis le disque, en vérifiant ses droits.
    ///
    /// Les JWT sont de courte durée et signés à chaque commande : il n'y a
    /// aucun jeton à mettre en cache, donc rien à réécrire dans la config.
    pub fn private_key(&self) -> Result<Vec<u8>> {
        ensure_private(&self.private_key_path)?;
        fs::read(&self.private_key_path).with_context(|| {
            format!(
                "lecture de la clé privée {}",
                self.private_key_path.display()
            )
        })
    }

    /// Port sur lequel écouter pour recevoir le code d'autorisation, déduit de
    /// l'URL de retour.
    pub fn redirect_port(&self) -> Result<u16> {
        let after_scheme = self
            .redirect_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&self.redirect_url);
        let authority = after_scheme.split('/').next().unwrap_or_default();
        match authority.rsplit_once(':') {
            Some((_, port)) => port
                .parse()
                .with_context(|| format!("port illisible dans {}", self.redirect_url)),
            // Pas de port explicite : les valeurs par défaut du schéma.
            None if self.redirect_url.starts_with("https://") => Ok(443),
            None => Ok(80),
        }
    }

    /// Vrai si l'URL de retour pointe explicitement vers la machine locale.
    pub fn redirect_is_local(&self) -> bool {
        let url = self.redirect_url.to_lowercase();
        url.contains("://localhost") || url.contains("://127.0.0.1") || url.contains("://[::1]")
    }

    /// Adresse d'écoute pour intercepter le retour, si c'est possible.
    ///
    /// `listen` l'emporte : c'est le cas d'un tunnel, où l'URL de retour est
    /// publique mais où la requête finit bien ici. Sinon on ne peut écouter que
    /// si la banque redirige elle-même vers la boucle locale.
    pub fn listen_addr(&self) -> Result<Option<String>> {
        if let Some(explicit) = &self.listen {
            return Ok(Some(explicit.clone()));
        }
        match self.redirect_is_local() {
            true => Ok(Some(format!("127.0.0.1:{}", self.redirect_port()?))),
            false => Ok(None),
        }
    }
}

/// Répertoire de configuration, `~/.config/ecofin` sous Linux et macOS.
pub fn config_dir() -> Result<PathBuf> {
    // `ECOFIN_HOME` permet d'isoler un environnement de test sans toucher à la
    // configuration réelle.
    if let Ok(custom) = std::env::var("ECOFIN_HOME") {
        return Ok(PathBuf::from(custom));
    }
    let dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .context("impossible de déterminer le répertoire de configuration")?;
    Ok(dirs.config_dir().to_path_buf())
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn database_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("ecofin.db"))
}

/// Emplacement où la clé privée est recopiée lors de `auth login`.
pub fn key_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("enablebanking.pem"))
}

impl Config {
    /// Charge la configuration : le fichier, puis les variables
    /// d'environnement qui le surchargent.
    pub fn load() -> Result<Self> {
        let mut config = Self::from_file()?;
        config.apply_env();
        Ok(config)
    }

    fn from_file() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("lecture de {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing de {}", path.display()))
    }

    /// Applique les variables d'environnement par-dessus le fichier.
    ///
    /// Elles priment, et suffisent à elles seules : avec
    /// `ECOFIN_APPLICATION_ID` et `ECOFIN_PRIVATE_KEY`, le CLI fonctionne sans
    /// jamais avoir lancé `auth login`. C'est ce qui permet de tout décrire
    /// dans un `.env` et de garder les conteneurs jetables.
    fn apply_env(&mut self) {
        self.apply_overrides(
            env_value("ECOFIN_APPLICATION_ID"),
            env_value("ECOFIN_PRIVATE_KEY").map(PathBuf::from),
            env_value("ECOFIN_REDIRECT_URL"),
            env_value("ECOFIN_LISTEN"),
        );
    }

    /// Cœur testable de `apply_env`, séparé pour ne pas dépendre de variables
    /// d'environnement globales au processus, que des tests parallèles se
    /// disputeraient.
    fn apply_overrides(
        &mut self,
        application_id: Option<String>,
        private_key: Option<PathBuf>,
        redirect_url: Option<String>,
        listen: Option<String>,
    ) {
        match &mut self.enable_banking {
            Some(existing) => {
                if let Some(v) = application_id {
                    existing.application_id = v;
                }
                if let Some(v) = private_key {
                    existing.private_key_path = v;
                }
                if let Some(v) = redirect_url {
                    existing.redirect_url = v;
                }
                if let Some(v) = listen {
                    existing.listen = Some(v);
                }
            }
            // Sans fichier de configuration, il faut au minimum l'identifiant
            // et la clé ; le reste a des valeurs par défaut utilisables.
            None => {
                if let (Some(application_id), Some(private_key_path)) =
                    (application_id, private_key)
                {
                    self.enable_banking = Some(EnableBankingConfig {
                        application_id,
                        private_key_path,
                        redirect_url: redirect_url.unwrap_or_else(default_redirect),
                        listen,
                    });
                }
            }
        }
    }

    /// Écrit la configuration en 0600, en créant le répertoire au besoin.
    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self).context("sérialisation de la configuration")?;
        fs::write(&path, raw).with_context(|| format!("écriture de {}", path.display()))?;
        set_private(&path)?;
        Ok(())
    }

    /// Accès aux identifiants, avec un message utile si l'utilisateur n'a pas
    /// encore lancé `ecofin auth login`.
    pub fn require_enable_banking(&self) -> Result<&EnableBankingConfig> {
        self.enable_banking
            .as_ref()
            .context("aucune application Enable Banking enregistrée — lance `ecofin auth login`")
    }
}

/// Copie la clé privée dans le répertoire de configuration, en 0600.
pub fn install_private_key(source: &Path) -> Result<PathBuf> {
    let pem = fs::read(source)
        .with_context(|| format!("lecture de la clé privée {}", source.display()))?;

    // Un fichier qui n'est pas une clé PEM produirait plus tard une erreur de
    // signature incompréhensible ; autant refuser tout de suite.
    if !pem.starts_with(b"-----BEGIN") {
        bail!(
            "{} ne ressemble pas à une clé PEM (elle doit commencer par -----BEGIN)",
            source.display()
        );
    }

    let destination = key_path()?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("création de {}", parent.display()))?;
    }
    fs::write(&destination, pem)
        .with_context(|| format!("écriture de {}", destination.display()))?;
    set_private(&destination)?;
    Ok(destination)
}

#[cfg(unix)]
fn set_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restriction des droits sur {}", path.display()))
}

#[cfg(not(unix))]
fn set_private(_path: &Path) -> Result<()> {
    Ok(())
}

/// Refuse de lire un secret lisible par d'autres utilisateurs.
#[cfg(unix)]
fn ensure_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .with_context(|| format!("lecture des droits de {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{} est accessible à d'autres utilisateurs (droits {:o}). \
             Corrige avec : chmod 600 {}",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(redirect: &str) -> EnableBankingConfig {
        EnableBankingConfig {
            application_id: "app".into(),
            private_key_path: PathBuf::from("/dev/null"),
            redirect_url: redirect.into(),
            listen: None,
        }
    }

    #[test]
    fn extracts_explicit_port() {
        assert_eq!(
            config_with("http://localhost:8484/callback")
                .redirect_port()
                .unwrap(),
            8484
        );
    }

    #[test]
    fn falls_back_to_scheme_default_port() {
        assert_eq!(
            config_with("https://example.com/cb")
                .redirect_port()
                .unwrap(),
            443
        );
    }

    #[test]
    fn detects_local_redirects() {
        assert!(config_with("http://localhost:8484/callback").redirect_is_local());
        assert!(config_with("http://127.0.0.1:9000/cb").redirect_is_local());
        assert!(!config_with("https://example.com/cb").redirect_is_local());
    }

    #[test]
    fn listens_on_loopback_for_a_local_redirect() {
        let addr = config_with("http://localhost:8484/callback")
            .listen_addr()
            .unwrap();
        assert_eq!(addr.as_deref(), Some("127.0.0.1:8484"));
    }

    /// Sans `listen`, une URL publique impose la saisie manuelle.
    #[test]
    fn does_not_listen_for_a_public_redirect() {
        let addr = config_with("https://ecofin.example.eu/callback")
            .listen_addr()
            .unwrap();
        assert_eq!(addr, None);
    }

    /// Cas du tunnel : l'URL est publique, mais la requête aboutit bien ici.
    #[test]
    fn explicit_listen_overrides_a_public_redirect() {
        let mut config = config_with("https://ecofin.example.eu/callback");
        config.listen = Some("0.0.0.0:8484".into());
        assert_eq!(
            config.listen_addr().unwrap().as_deref(),
            Some("0.0.0.0:8484")
        );
    }

    /// Un `.env` complet suffit : aucun `auth login` préalable n'est requis.
    #[test]
    fn env_alone_builds_a_usable_config() {
        let mut config = Config::default();
        config.apply_overrides(
            Some("app-42".into()),
            Some(PathBuf::from("/run/secrets/k.pem")),
            Some("https://ecofin.example.eu/callback".into()),
            Some("0.0.0.0:8484".into()),
        );

        let credentials = config.require_enable_banking().unwrap();
        assert_eq!(credentials.application_id, "app-42");
        assert_eq!(
            credentials.private_key_path,
            PathBuf::from("/run/secrets/k.pem")
        );
        assert_eq!(credentials.listen.as_deref(), Some("0.0.0.0:8484"));
    }

    /// L'identifiant seul ne permet pas de signer : sans clé, on ne fabrique
    /// pas de configuration à moitié valide.
    #[test]
    fn env_without_key_builds_nothing() {
        let mut config = Config::default();
        config.apply_overrides(Some("app-42".into()), None, None, None);
        assert!(config.enable_banking.is_none());
    }

    #[test]
    fn env_overrides_the_saved_file() {
        let mut config = Config {
            enable_banking: Some(config_with("https://ancienne.example/cb")),
        };
        config.apply_overrides(None, None, Some("https://nouvelle.example/cb".into()), None);

        let credentials = config.require_enable_banking().unwrap();
        assert_eq!(credentials.redirect_url, "https://nouvelle.example/cb");
        // Ce qui n'est pas surchargé doit survivre.
        assert_eq!(credentials.application_id, "app");
    }

    /// Compose transmet une variable non renseignée comme chaîne vide : elle ne
    /// doit pas écraser une valeur enregistrée.
    #[test]
    fn blank_env_values_are_ignored() {
        assert_eq!(env_value_from(Some("   ")), None);
        assert_eq!(env_value_from(Some("")), None);
        assert_eq!(env_value_from(Some(" x ")), Some("x".to_string()));
        assert_eq!(env_value_from(None), None);
    }

    /// Même normalisation que `env_value`, sur une valeur fournie.
    fn env_value_from(raw: Option<&str>) -> Option<String> {
        raw.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }
}
