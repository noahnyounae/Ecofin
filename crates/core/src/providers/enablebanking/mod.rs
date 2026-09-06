//! Fournisseur Enable Banking.
//!
//! API DSP2 avec inscription libre en self-service. Le mode *Restricted
//! Production* couvre gratuitement la lecture de ses propres comptes, sous
//! l'agrément AISP d'Enable Banking — donc sans certificat eIDAS ni démarche
//! ACPR.
//!
//! Documentation : <https://enablebanking.com/docs/api/reference/>

pub mod auth;
pub mod client;
pub mod types;

pub use client::{EnableBankingClient, consent_deadline};
