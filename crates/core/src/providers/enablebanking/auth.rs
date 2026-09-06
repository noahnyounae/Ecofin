//! Signature des JWT et réception du code d'autorisation.
//!
//! Enable Banking n'a pas de jeton à stocker : chaque requête porte un JWT
//! court, signé en RS256 avec la clé privée de l'application. Le `kid` de
//! l'en-tête identifie l'application.

use anyhow::{Context, Result, anyhow, bail};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Durée de validité du JWT. Court : il est régénéré à chaque commande.
const TOKEN_TTL_SECONDS: i64 = 3600;

/// Délai laissé à l'utilisateur pour s'authentifier auprès de sa banque.
///
/// Large à dessein : une authentification forte peut passer par un SMS ou une
/// validation dans l'application de la banque, et un dépassement oblige à
/// refaire tout le parcours.
const AUTH_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Serialize)]
struct Claims {
    iss: &'static str,
    aud: &'static str,
    iat: i64,
    exp: i64,
}

/// Fabrique un JWT signé pour l'application donnée.
pub fn sign_jwt(application_id: &str, private_key_pem: &[u8]) -> Result<String> {
    let key = EncodingKey::from_rsa_pem(private_key_pem).context(
        "la clé privée n'a pas pu être lue — attendu une clé RSA au format PEM, \
         telle que téléchargée depuis le portail Enable Banking",
    )?;

    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".to_string());
    header.kid = Some(application_id.to_string());

    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        iss: "enablebanking.com",
        aud: "api.enablebanking.com",
        iat: now,
        exp: now + TOKEN_TTL_SECONDS,
    };

    encode(&header, &claims, &key).context("signature du JWT")
}

/// Résultat de l'aller-retour dans le navigateur.
#[derive(Debug)]
pub struct AuthCallback {
    pub code: String,
    pub state: Option<String>,
}

/// Démarre un serveur HTTP minimal, attend la redirection de la banque, et
/// renvoie le code d'autorisation.
///
/// On n'utilise pas de framework : une seule requête à traiter, dont on ne lit
/// que la ligne de départ.
///
/// `addr` est une adresse `hôte:port`. En local, `127.0.0.1` suffit ; derrière
/// un tunnel ou dans un conteneur, il faut écouter sur `0.0.0.0` pour que la
/// requête venue de l'extérieur parvienne jusqu'ici.
pub async fn wait_for_callback(addr: &str, expected_state: &str) -> Result<AuthCallback> {
    let listener = TcpListener::bind(addr).await.with_context(|| {
        format!(
            "impossible d'écouter sur {addr}. \
             Un autre programme utilise peut-être déjà ce port."
        )
    })?;

    let accept = async {
        loop {
            let (mut stream, _) = listener.accept().await.context("connexion entrante")?;

            let mut buffer = [0u8; 4096];
            let read = stream
                .read(&mut buffer)
                .await
                .context("lecture de la requête")?;
            let request = String::from_utf8_lossy(&buffer[..read]);

            let Some(target) = request_target(&request) else {
                respond(&mut stream, "Requête invalide.").await;
                continue;
            };

            let params = query_params(target);

            // Les navigateurs demandent /favicon.ico en parallèle : on ignore
            // tout ce qui ne porte pas de code ni d'erreur.
            if let Some(error) = params.get("error") {
                let description = params
                    .get("error_description")
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default();
                respond(
                    &mut stream,
                    "Autorisation refusée. Tu peux fermer cet onglet.",
                )
                .await;
                bail!("la banque a refusé l'autorisation : {error}{description}");
            }

            let Some(code) = params.get("code") else {
                respond(&mut stream, "En attente de l'autorisation…").await;
                continue;
            };

            respond(
                &mut stream,
                "Compte connecté. Tu peux fermer cet onglet et revenir au terminal.",
            )
            .await;

            return Ok(AuthCallback {
                code: code.clone(),
                state: params.get("state").cloned(),
            });
        }
    };

    let callback = tokio::time::timeout(AUTH_TIMEOUT, accept)
        .await
        .map_err(|_| {
            anyhow!(
                "aucune réponse après {} minutes. Relance `ecofin link new`.",
                AUTH_TIMEOUT.as_secs() / 60
            )
        })??;

    // Le `state` protège contre une réponse qui ne viendrait pas de la demande
    // qu'on vient d'émettre.
    if let Some(returned) = &callback.state
        && returned != expected_state
    {
        bail!("le paramètre `state` renvoyé ne correspond pas à la demande émise");
    }

    Ok(callback)
}

/// Extrait le code d'autorisation d'une saisie de l'utilisateur.
///
/// Beaucoup de fournisseurs, dont Enable Banking, refusent une URL de retour en
/// `http://` et interdisent donc au CLI d'intercepter lui-même la redirection.
/// Dans ce cas l'utilisateur recopie l'URL sur laquelle sa banque l'a envoyé ;
/// on accepte aussi le code seul, pour qui préfère l'extraire à la main.
pub fn code_from_input(input: &str, expected_state: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("aucune valeur saisie");
    }

    // Sans chaîne de requête, c'est le code seul : rien à analyser.
    let Some(question_mark) = input.find('?') else {
        return Ok(input.to_string());
    };

    let params = query_params(&input[question_mark..]);

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        bail!("la banque a refusé l'autorisation : {error}{description}");
    }

    let code = params.get("code").context(
        "cette URL ne contient pas de paramètre `code` — \
         vérifie que tu as bien copié l'URL complète après l'authentification",
    )?;

    // Le `state` protège contre une réponse qui ne viendrait pas de la demande
    // qu'on vient d'émettre. Toutes les banques ne le renvoient pas.
    if let Some(returned) = params.get("state")
        && returned != expected_state
    {
        bail!("le paramètre `state` renvoyé ne correspond pas à la demande émise");
    }

    Ok(code.clone())
}

/// Extrait la cible d'une ligne `GET /callback?code=... HTTP/1.1`.
fn request_target(request: &str) -> Option<&str> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    if method != "GET" {
        return None;
    }
    parts.next()
}

/// Décompose la chaîne de requête d'une cible HTTP.
fn query_params(target: &str) -> HashMap<String, String> {
    let Some((_, query)) = target.split_once('?') else {
        return HashMap::new();
    };
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((percent_decode(key), percent_decode(value)))
        })
        .collect()
}

/// Décodage `application/x-www-form-urlencoded`.
///
/// Suffisant ici : on ne traite que des codes et des états, qui sont des jetons
/// ASCII, mais les descriptions d'erreur peuvent contenir des espaces encodées.
fn percent_decode(input: &str) -> String {
    let bytes = input.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Renvoie une page minimale au navigateur. Une erreur d'écriture ici n'a pas
/// à faire échouer l'autorisation : le code est déjà entre nos mains.
async fn respond(stream: &mut tokio::net::TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\">\
         <title>ecofin</title>\
         <body style=\"font-family:system-ui;padding:3rem;max-width:32rem;margin:auto\">\
         <h1 style=\"font-size:1.25rem\">ecofin</h1><p>{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_target() {
        let request = "GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(
            request_target(request),
            Some("/callback?code=abc&state=xyz")
        );
    }

    #[test]
    fn ignores_non_get_requests() {
        assert_eq!(request_target("POST /callback HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn extracts_query_params() {
        let params = query_params("/callback?code=abc123&state=xyz");
        assert_eq!(params.get("code"), Some(&"abc123".to_string()));
        assert_eq!(params.get("state"), Some(&"xyz".to_string()));
    }

    #[test]
    fn returns_no_params_without_query() {
        assert!(query_params("/callback").is_empty());
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("acc%C3%A8s+refus%C3%A9"), "accès refusé");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn reads_code_from_pasted_url() {
        let code =
            code_from_input("https://example.com/cb?code=abc123&state=st-1", "st-1").unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn accepts_bare_code() {
        assert_eq!(code_from_input("  abc123  ", "st-1").unwrap(), "abc123");
    }

    /// Certaines banques ne renvoient pas le `state` ; on ne peut pas refuser
    /// pour autant, sous peine de bloquer l'utilisateur.
    #[test]
    fn accepts_url_without_state() {
        let code = code_from_input("https://example.com/cb?code=abc123", "st-1").unwrap();
        assert_eq!(code, "abc123");
    }

    #[test]
    fn rejects_pasted_url_with_wrong_state() {
        let error = code_from_input("https://example.com/cb?code=a&state=forged", "st-1")
            .unwrap_err()
            .to_string();
        assert!(error.contains("`state`"), "message inattendu : {error}");
    }

    #[test]
    fn surfaces_refusal_in_pasted_url() {
        let error = code_from_input(
            "https://example.com/cb?error=access_denied&error_description=Refus%C3%A9",
            "st-1",
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("access_denied"),
            "message inattendu : {error}"
        );
        assert!(error.contains("Refusé"), "message inattendu : {error}");
    }

    #[test]
    fn rejects_url_without_code() {
        let error = code_from_input("https://example.com/cb?foo=bar", "st-1")
            .unwrap_err()
            .to_string();
        assert!(error.contains("`code`"), "message inattendu : {error}");
    }

    #[test]
    fn rejects_empty_input() {
        assert!(code_from_input("   ", "st-1").is_err());
    }

    /// Demande au système un port libre, pour que les tests ne se marchent pas
    /// dessus et n'échouent pas sur une machine où le port est déjà pris.
    async fn free_port() -> u16 {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Joue le rôle du navigateur : émet la requête de redirection et lit la
    /// page renvoyée.
    async fn call(port: u16, target: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut response)
            .await
            .unwrap();
        response
    }

    #[tokio::test]
    async fn receives_code_from_redirect() {
        let port = free_port().await;
        let addr = format!("127.0.0.1:{port}");
        let server = tokio::spawn(async move { wait_for_callback(&addr, "state-123").await });

        // Laisse le serveur se lier avant d'émettre la requête.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let response = call(port, "/callback?code=the-code&state=state-123").await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));

        let callback = server.await.unwrap().unwrap();
        assert_eq!(callback.code, "the-code");
        assert_eq!(callback.state.as_deref(), Some("state-123"));
    }

    #[tokio::test]
    async fn rejects_mismatched_state() {
        let port = free_port().await;
        let addr = format!("127.0.0.1:{port}");
        let server = tokio::spawn(async move { wait_for_callback(&addr, "expected").await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        call(port, "/callback?code=the-code&state=forged").await;

        let error = server.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("`state`"), "message inattendu : {error}");
    }

    #[tokio::test]
    async fn surfaces_bank_refusal() {
        let port = free_port().await;
        let addr = format!("127.0.0.1:{port}");
        let server = tokio::spawn(async move { wait_for_callback(&addr, "state").await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        call(
            port,
            "/callback?error=access_denied&error_description=Refus%C3%A9+par+l%27utilisateur",
        )
        .await;

        let error = server.await.unwrap().unwrap_err().to_string();
        assert!(
            error.contains("access_denied"),
            "message inattendu : {error}"
        );
        assert!(
            error.contains("Refusé par l'utilisateur"),
            "message inattendu : {error}"
        );
    }

    /// Les navigateurs demandent /favicon.ico en même temps que la redirection :
    /// le serveur doit continuer à écouter au lieu de prendre ça pour la réponse.
    #[tokio::test]
    async fn ignores_requests_without_code() {
        let port = free_port().await;
        let addr = format!("127.0.0.1:{port}");
        let server = tokio::spawn(async move { wait_for_callback(&addr, "state").await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        call(port, "/favicon.ico").await;
        call(port, "/callback?code=real-code&state=state").await;

        let callback = server.await.unwrap().unwrap();
        assert_eq!(callback.code, "real-code");
    }
}
