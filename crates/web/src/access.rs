//! Barrière d'accès : sessions par cookie signé.
//!
//! L'application est servie par un tunnel public. Sans cette barrière, l'URL
//! suffirait à lire quatre ans d'opérations bancaires.
//!
//! Le jeton de session voyage dans un cookie `HttpOnly` : inaccessible au
//! JavaScript de la page, il ne peut pas être exfiltré par une injection de
//! script. `SameSite=Lax` empêche qu'un autre site le fasse envoyer à sa place.
//!
//! Les échecs d'authentification sont comptés en mémoire, par adresse. Ce
//! compteur disparaît au redémarrage — insuffisant contre un attaquant
//! déterminé, mais il transforme une attaque par dictionnaire en entreprise
//! très lente, et le serveur n'a pas le droit d'écrire dans la base pour faire
//! mieux.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, SameSite};
use ecofin_core::auth::{self, SESSION_LIFETIME};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::AppState;

/// Nom du cookie de session.
pub const SESSION_COOKIE: &str = "ecofin_session";

/// Nombre d'échecs tolérés avant blocage temporaire d'une adresse.
const MAX_FAILURES: u32 = 5;

/// Durée du blocage après trop d'échecs.
const LOCKOUT: Duration = Duration::from_secs(300);

/// Compteur d'échecs d'authentification, par adresse.
#[derive(Clone, Default)]
pub struct Throttle {
    failures: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}

impl Throttle {
    /// Vrai si l'adresse est actuellement bloquée.
    pub fn is_locked(&self, key: &str) -> bool {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match failures.get(key) {
            Some((count, since)) if *count >= MAX_FAILURES => {
                if since.elapsed() < LOCKOUT {
                    return true;
                }
                // Blocage expiré : on repart de zéro.
                failures.remove(key);
                false
            }
            _ => false,
        }
    }

    pub fn record_failure(&self, key: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let entry = failures
            .entry(key.to_string())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
    }

    /// Une authentification réussie efface l'ardoise.
    pub fn record_success(&self, key: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

/// Fabrique le cookie de session.
///
/// `secure` n'est posé que derrière HTTPS : sur `http://localhost`, un cookie
/// marqué `Secure` ne serait jamais renvoyé et empêcherait de se connecter en
/// développement.
pub fn session_cookie(token: String, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE, token);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(secure);
    cookie.set_path("/");
    cookie.set_max_age(Some(time::Duration::seconds(
        SESSION_LIFETIME.num_seconds(),
    )));
    cookie
}

/// Cookie vidé, pour la déconnexion.
pub fn expired_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie = session_cookie(String::new(), secure);
    cookie.set_max_age(Some(time::Duration::ZERO));
    cookie
}

/// Refuse les requêtes sans session valide.
///
/// Les routes de connexion et les ressources nécessaires à l'affichage de
/// l'écran de connexion échappent au contrôle : sans cela, impossible de se
/// connecter.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if is_public(path) {
        return next.run(request).await;
    }

    match authenticated(&state, &request) {
        Some(user) => {
            // L'utilisateur est déposé dans la requête : les gestionnaires y
            // puisent leur périmètre sans revalider le jeton.
            let mut request = request;
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None => {
            // 401 plutôt que 403 : le front sait alors qu'il doit afficher
            // l'écran de connexion, et non signaler un droit manquant.
            (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "error": "session requise" })),
            )
                .into_response()
        }
    }
}

/// Utilisateur authentifié par la requête.
///
/// Porte l'identifiant autant que l'adresse : c'est lui qui détermine le
/// périmètre des données visibles.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub id: i64,
    pub email: String,
}

impl Authenticated {
    /// Périmètre des données de cet utilisateur.
    ///
    /// Jamais `Scope::All` : le serveur web n'a aucune raison de voir les
    /// comptes bancaires d'un autre.
    pub fn scope(&self) -> ecofin_core::store::Scope {
        ecofin_core::store::Scope::User(self.id)
    }
}

/// Retrouve le compte associé à la requête, si sa session est valide.
pub fn authenticated(state: &AppState, request: &Request) -> Option<Authenticated> {
    let token = request
        .headers()
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse_encoded)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == SESSION_COOKIE)?
        .value()
        .to_string();

    let secret = state.session_secret()?;
    let claims = auth::verify_session(&token, &secret).ok()?;

    // Le jeton est authentique et non expiré ; reste à vérifier que le compte
    // existe encore et que ses sessions n'ont pas été révoquées entre-temps.
    let id: i64 = claims.sub.parse().ok()?;
    let user = state.with_store(|store| store.user_by_id(id)).ok()??;
    (user.token_version == claims.ver).then_some(Authenticated {
        id: user.id,
        email: user.email,
    })
}

/// Chemins accessibles sans session.
///
/// Volontairement restreint : l'écran de connexion et ses ressources, rien de
/// plus. Toute donnée bancaire passe par `/api/`, qui reste fermé.
fn is_public(path: &str) -> bool {
    matches!(path, "/api/auth/login" | "/api/health")
        || path == "/"
        || path.starts_with("/assets/")
        || path == "/favicon.ico"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_route_stays_reachable() {
        assert!(is_public("/api/auth/login"));
    }

    #[test]
    fn the_shell_and_its_assets_stay_reachable() {
        assert!(is_public("/"));
        assert!(is_public("/assets/index-abc123.js"));
        assert!(is_public("/favicon.ico"));
    }

    /// Tout ce qui porte des données bancaires doit rester fermé.
    #[test]
    fn banking_data_is_never_public() {
        assert!(!is_public("/api/accounts"));
        assert!(!is_public("/api/capital"));
        assert!(!is_public("/api/transactions/abc"));
        assert!(!is_public("/api/auth/me"));
    }

    #[test]
    fn locks_out_after_repeated_failures() {
        let throttle = Throttle::default();
        assert!(!throttle.is_locked("1.2.3.4"));

        for _ in 0..MAX_FAILURES {
            throttle.record_failure("1.2.3.4");
        }
        assert!(throttle.is_locked("1.2.3.4"));
        // Une autre adresse n'est pas affectée.
        assert!(!throttle.is_locked("5.6.7.8"));
    }

    #[test]
    fn a_success_clears_the_counter() {
        let throttle = Throttle::default();
        for _ in 0..MAX_FAILURES {
            throttle.record_failure("1.2.3.4");
        }
        throttle.record_success("1.2.3.4");
        assert!(!throttle.is_locked("1.2.3.4"));
    }

    #[test]
    fn the_session_cookie_is_not_readable_by_scripts() {
        let cookie = session_cookie("jeton".into(), true);
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    }

    /// Sans HTTPS, un cookie `Secure` ne serait jamais renvoyé : la connexion
    /// en développement local deviendrait impossible.
    #[test]
    fn the_cookie_drops_secure_outside_https() {
        assert_eq!(session_cookie("jeton".into(), false).secure(), Some(false));
    }

    #[test]
    fn logging_out_empties_the_cookie() {
        let cookie = expired_cookie(true);
        assert_eq!(cookie.value(), "");
        assert_eq!(cookie.max_age(), Some(time::Duration::ZERO));
    }
}
