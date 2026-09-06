//! Serveur HTTP d'ecofin.
//!
//! Sert l'application web et une API JSON en lecture seule sur la base locale.
//! Aucun appel à la banque n'est déclenché ici : la synchronisation reste le
//! travail du CLI, ce qui garde le serveur inoffensif vis-à-vis du quota DSP2
//! et le rend consultable même banque indisponible.
//!
//! L'accès est protégé par un compte et un mot de passe : voir [`access`].
//! Les comptes se créent depuis le CLI — le serveur, en lecture seule, ne peut
//! pas écrire dans la base.

mod access;
mod api;
mod state;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use std::net::SocketAddr;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use state::AppState;

/// Adresse d'écoute par défaut.
///
/// `0.0.0.0` parce que le serveur vit dans un conteneur : sur la boucle locale,
/// il serait injoignable depuis le tunnel comme depuis le port publié.
const DEFAULT_BIND: &str = "0.0.0.0:8080";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ecofin_web=info,tower_http=warn".into()),
        )
        .init();

    let state = AppState::new()?;

    // Le front est un ensemble de fichiers statiques ; toute route inconnue
    // renvoie index.html pour que la navigation côté client fonctionne.
    let assets = std::env::var("ECOFIN_WEB_ROOT").unwrap_or_else(|_| "/srv/web".to_string());
    let index = std::path::Path::new(&assets).join("index.html");

    let app = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/accounts", get(api::accounts))
        .route("/api/capital", get(api::capital))
        .route("/api/subscriptions", get(api::subscriptions))
        .route("/api/gaps", get(api::gaps))
        .route(
            "/api/categories",
            get(api::categories).post(api::add_category),
        )
        .route(
            "/api/categories/{key}",
            axum::routing::put(api::rename_category).delete(api::remove_category),
        )
        .route(
            "/api/categories/{key}/parent",
            axum::routing::put(api::set_parent),
        )
        .route("/api/wallets", get(api::wallets).post(api::add_wallet))
        .route(
            "/api/wallets/{id}",
            axum::routing::put(api::update_wallet).delete(api::remove_wallet),
        )
        .route(
            "/api/wallets/{id}/contributions",
            axum::routing::post(api::add_contribution),
        )
        .route(
            "/api/contributions/{id}",
            axum::routing::delete(api::remove_contribution),
        )
        .route("/api/rules", get(api::rules).put(api::set_rule))
        .route(
            "/api/rules/{label}",
            axum::routing::delete(api::delete_rule),
        )
        .route("/api/transactions/{id}", get(api::transaction))
        .route("/api/auth/login", axum::routing::post(api::login))
        .route("/api/auth/logout", axum::routing::post(api::logout))
        .route("/api/auth/me", get(api::me))
        // Toute route inconnue rend index.html : la navigation côté client
        // fonctionne alors sur une URL ouverte directement.
        .fallback_service(ServeDir::new(&assets).fallback(ServeFile::new(index)))
        // La barrière vient après le fallback, donc couvre aussi les fichiers
        // statiques. Placée plus haut, elle ne protégerait que les routes
        // déclarées avant elle et laisserait l'application servie en clair.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            access::guard,
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind: SocketAddr = std::env::var("ECOFIN_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .context("ECOFIN_BIND doit être une adresse hôte:port")?;

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("écoute sur {bind}"))?;

    tracing::info!("ecofin-web écoute sur http://{bind}");
    // `into_make_service_with_connect_info` expose l'adresse de l'appelant, dont
    // le compteur d'échecs de connexion a besoin.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("serveur HTTP")?;
    Ok(())
}
