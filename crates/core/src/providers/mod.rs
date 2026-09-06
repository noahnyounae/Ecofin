//! Fournisseurs de données bancaires.
//!
//! Un seul fournisseur pour l'instant. Les modules sont séparés dès maintenant
//! pour qu'ajouter un import CSV ou un agrégateur payant ne demande pas de
//! toucher au reste de l'application : `model`, `store` et `ui` ignorent tout
//! de la DSP2.

pub mod enablebanking;
