//! Cœur d'ecofin : modèle, persistance, fournisseurs, import et analyse.
//!
//! Cette bibliothèque n'a pas d'interface — ni terminal, ni HTTP. Le CLI et le
//! serveur web en sont deux consommateurs, et une fonctionnalité ajoutée ici
//! devient disponible des deux côtés sans duplication.

pub mod analysis;
pub mod auth;
pub mod category;
pub mod config;
pub mod importer;
pub mod merchant;
pub mod model;
pub mod preferences;
pub mod providers;
pub mod schedule;
pub mod store;
pub mod subscriptions;
pub mod wallets;

pub use store::Store;
