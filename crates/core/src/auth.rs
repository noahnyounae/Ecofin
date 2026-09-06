//! Comptes utilisateurs et sessions.
//!
//! Le serveur web monte la base **en lecture seule** : il ne peut donc ni créer
//! de compte, ni enregistrer de session. C'est une propriété qu'on tient à
//! garder — un serveur exposé qui ne peut rien écrire ne peut rien corrompre.
//! Il en découle deux choix :
//!
//! - les comptes se créent depuis le CLI, seul à écrire dans la base ;
//! - les sessions sont des jetons signés, vérifiables sans rien stocker.
//!
//! Contrepartie assumée : une session ne peut pas être révoquée côté serveur
//! avant son échéance. Elle est donc courte, et [`User::token_version`] permet
//! d'invalider d'un coup toutes les sessions d'un compte depuis le CLI.

use anyhow::{Context, Result, bail};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// Durée de validité d'une session.
///
/// Courte à dessein : sans état côté serveur, c'est l'échéance qui borne la
/// durée de vie d'un jeton volé.
pub const SESSION_LIFETIME: Duration = Duration::hours(12);

/// Longueur minimale d'un mot de passe.
///
/// Un compte unique protégeant quatre ans d'opérations bancaires justifie
/// d'être exigeant. La longueur prime sur la complexité imposée.
pub const MIN_PASSWORD_LENGTH: usize = 12;

/// Un compte pouvant ouvrir une session sur l'application web.
#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    /// Empreinte Argon2id, au format PHC. Le sel y est inclus.
    pub password_hash: String,
    /// Incrémenté pour invalider d'un coup les sessions en cours.
    pub token_version: i64,
    pub created_at: DateTime<Utc>,
}

/// Contenu d'un jeton de session.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Identifiant du compte.
    pub sub: String,
    pub email: String,
    /// Version des jetons au moment de l'émission ; un décalage invalide.
    pub ver: i64,
    pub exp: i64,
    pub iat: i64,
}

/// Calcule l'empreinte d'un mot de passe.
///
/// Argon2id est délibéré : conçu pour être coûteux en mémoire, il résiste aux
/// attaques par matériel dédié là où un simple SHA ne freine personne.
pub fn hash_password(password: &str) -> Result<String> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        bail!("le mot de passe doit faire au moins {MIN_PASSWORD_LENGTH} caractères");
    }
    // Le sel est tiré au sort par la bibliothèque, un par mot de passe, et
    // conservé dans l'empreinte au format PHC.
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("calcul de l'empreinte : {e}"))
}

/// Vérifie un mot de passe contre une empreinte.
///
/// La comparaison est faite par Argon2, en temps constant : la durée de la
/// vérification ne renseigne pas sur la justesse du mot de passe.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Fabrique un secret de signature aléatoire, encodé en hexadécimal.
///
/// 32 octets tirés directement du générateur du système d'exploitation : c'est
/// ce secret qui rend un jeton de session impossible à forger.
///
/// Un échec du générateur n'est pas rattrapable — se rabattre sur une valeur
/// prévisible reviendrait à laisser signer n'importe qui.
pub fn generate_session_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("le générateur aléatoire du système est indisponible");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Émet un jeton de session pour un compte.
pub fn issue_session(user: &User, secret: &str) -> Result<String> {
    let now = Utc::now();
    let claims = SessionClaims {
        sub: user.id.to_string(),
        email: user.email.clone(),
        ver: user.token_version,
        iat: now.timestamp(),
        exp: (now + SESSION_LIFETIME).timestamp(),
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .context("signature du jeton de session")
}

/// Vérifie un jeton et en extrait le contenu.
///
/// L'échéance est contrôlée par la bibliothèque ; la version des jetons ne peut
/// l'être qu'en confrontant au compte, ce que fait l'appelant.
pub fn verify_session(token: &str, secret: &str) -> Result<SessionClaims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    // Ces jetons ne circulent qu'entre le serveur et le navigateur : ni
    // émetteur ni audience à contrôler.
    validation.required_spec_claims.clear();

    decode::<SessionClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .context("jeton de session invalide ou expiré")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(hash: &str) -> User {
        User {
            id: 1,
            email: "moi@exemple.fr".into(),
            password_hash: hash.into(),
            token_version: 3,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn accepts_the_right_password() {
        let hash = hash_password("motdepasse-assez-long").unwrap();
        assert!(verify_password("motdepasse-assez-long", &hash));
    }

    #[test]
    fn rejects_a_wrong_password() {
        let hash = hash_password("motdepasse-assez-long").unwrap();
        assert!(!verify_password("motdepasse-assez-lonh", &hash));
        assert!(!verify_password("", &hash));
    }

    /// Deux comptes au même mot de passe ne doivent pas partager d'empreinte :
    /// c'est le rôle du sel, et ce qui rend les tables précalculées inutiles.
    #[test]
    fn the_same_password_yields_different_hashes() {
        let a = hash_password("motdepasse-assez-long").unwrap();
        let b = hash_password("motdepasse-assez-long").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("motdepasse-assez-long", &a));
        assert!(verify_password("motdepasse-assez-long", &b));
    }

    #[test]
    fn refuses_a_short_password() {
        let error = hash_password("court").unwrap_err().to_string();
        assert!(error.contains("12"), "message inattendu : {error}");
    }

    #[test]
    fn an_unparsable_hash_never_validates() {
        assert!(!verify_password("peu importe", "pas-une-empreinte"));
    }

    #[test]
    fn a_session_round_trips() {
        let secret = generate_session_secret();
        let token = issue_session(&user("x"), &secret).unwrap();
        let claims = verify_session(&token, &secret).unwrap();
        assert_eq!(claims.sub, "1");
        assert_eq!(claims.ver, 3);
    }

    /// Un jeton signé avec un autre secret doit être rejeté : c'est ce qui
    /// empêche d'en forger un.
    #[test]
    fn a_token_signed_elsewhere_is_rejected() {
        let token = issue_session(&user("x"), &generate_session_secret()).unwrap();
        assert!(verify_session(&token, &generate_session_secret()).is_err());
    }

    #[test]
    fn a_tampered_token_is_rejected() {
        let secret = generate_session_secret();
        let token = issue_session(&user("x"), &secret).unwrap();
        let mangled = format!("{}x", &token[..token.len() - 1]);
        assert!(verify_session(&mangled, &secret).is_err());
    }

    #[test]
    fn secrets_are_unpredictable() {
        let a = generate_session_secret();
        assert_eq!(a.len(), 64, "32 octets en hexadécimal");
        assert_ne!(a, generate_session_secret());
    }
}
