//! Synchronisation automatique, et repérage des périodes perdues.
//!
//! # Le plafond, et comment il est tenu
//!
//! La DSP2 limite à quatre les accès non assistés par compte et par 24 heures.
//! Une synchronisation complète en consomme deux — un pour les soldes, un pour
//! les opérations — ce qui interdirait trois passages quotidiens.
//!
//! Le solde n'est pourtant utile qu'une fois par jour : la courbe de capital se
//! reconstruit à partir des opérations et d'un unique solde de référence. Une
//! journée coûte donc :
//!
//! ```text
//! 1 solde + 3 relevés d'opérations = 4 accès
//! ```
//!
//! # Les rendez-vous manqués
//!
//! Une machine éteinte, un conteneur arrêté, et un créneau passe. Le planning
//! ne se contente donc pas d'attendre le prochain : au démarrage, il regarde en
//! arrière et rattrape immédiatement si un créneau a été manqué.
//!
//! # La zone de brouillard
//!
//! Une banque n'expose qu'une fenêtre glissante — 90 jours chez LCL. Passé ce
//! délai sans synchronisation, les opérations plus anciennes que la fenêtre
//! deviennent **définitivement hors de portée de l'API**. Elles ne sont pas
//! perdues pour autant : les relevés mensuels les contiennent. Encore faut-il
//! savoir qu'il y a un trou, et lequel — c'est ce que consigne une zone de
//! brouillard.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// Heures de synchronisation, en temps universel.
///
/// Trois passages espacés couvrent la journée sans se marcher dessus. Les
/// opérations bancaires étant comptabilisées par lots, un rythme plus serré
/// n'apporterait rien de plus que des appels consommés.
pub const DEFAULT_SLOTS: [u32; 3] = [6, 12, 19];

/// Fenêtre d'historique que la banque accepte d'exposer.
///
/// Valeur observée chez LCL. Au-delà, l'API ne renvoie rien, quelle que soit
/// la date demandée.
pub const HISTORY_WINDOW_DAYS: i64 = 90;

/// Ce qu'une journée peut consommer d'accès sans authentification forte.
pub const DAILY_ACCESS_LIMIT: u32 = 4;

/// Décision du planificateur pour l'instant présent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Synchroniser sans attendre : un créneau a été manqué, ou c'est l'heure.
    SyncNow {
        /// Nombre de créneaux passés depuis la dernière synchronisation.
        ///
        /// Au-delà de un, la machine était arrêtée ; l'information est
        /// conservée pour le dire à l'utilisateur.
        missed: usize,
    },
    /// Rien à faire avant cet instant.
    WaitUntil(DateTime<Utc>),
}

/// Planning de synchronisation.
#[derive(Debug, Clone)]
pub struct Schedule {
    hours: Vec<u32>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            hours: DEFAULT_SLOTS.to_vec(),
        }
    }
}

impl Schedule {
    /// Construit un planning à partir d'heures données.
    ///
    /// Les heures sont ordonnées et dédoublonnées : deux créneaux identiques
    /// gaspilleraient un accès pour rien.
    pub fn new(mut hours: Vec<u32>) -> Self {
        hours.retain(|h| *h < 24);
        hours.sort_unstable();
        hours.dedup();
        match hours.is_empty() {
            true => Self::default(),
            false => Self { hours },
        }
    }

    /// Nombre de créneaux par jour.
    pub fn daily_slots(&self) -> usize {
        self.hours.len()
    }

    /// Instant du premier créneau strictement postérieur à `moment`.
    pub fn next_slot(&self, moment: DateTime<Utc>) -> DateTime<Utc> {
        let today = moment.date_naive();
        let hour = moment.time().hour24();

        match self.hours.iter().find(|h| **h > hour) {
            Some(h) => at(today, *h),
            None => at(
                today + Duration::days(1),
                *self.hours.first().expect("non vide"),
            ),
        }
    }

    /// Nombre de créneaux tombés entre deux instants, borne haute comprise.
    pub fn slots_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> usize {
        if to <= from {
            return 0;
        }
        let mut count = 0;
        let mut cursor = self.next_slot(from);
        while cursor <= to {
            count += 1;
            cursor = self.next_slot(cursor);
        }
        count
    }

    /// Que faire maintenant, compte tenu de la dernière synchronisation.
    ///
    /// Sans synchronisation antérieure, on part immédiatement : mieux vaut des
    /// données tout de suite qu'attendre le prochain créneau.
    pub fn decide(&self, last_sync: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Decision {
        let Some(last) = last_sync else {
            return Decision::SyncNow { missed: 0 };
        };

        let missed = self.slots_between(last, now);
        match missed {
            0 => Decision::WaitUntil(self.next_slot(now)),
            _ => Decision::SyncNow { missed },
        }
    }
}

/// Une période que l'API ne peut plus rendre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogGap {
    pub account_id: String,
    /// Premier jour manquant.
    pub from: NaiveDate,
    /// Dernier jour manquant.
    pub to: NaiveDate,
}

impl FogGap {
    pub fn days(&self) -> i64 {
        (self.to - self.from).num_days() + 1
    }
}

/// Repère la période devenue inaccessible faute de synchronisation.
///
/// `newest_known` est la date de l'opération la plus récente déjà en base. La
/// banque ne rendant que les `window_days` derniers jours, tout ce qui sépare
/// cette date du début de la fenêtre est hors d'atteinte.
///
/// Renvoie `None` quand l'historique local rejoint la fenêtre — le cas normal,
/// où rien n'est perdu.
pub fn detect_gap(
    account_id: &str,
    newest_known: Option<NaiveDate>,
    today: NaiveDate,
    window_days: i64,
) -> Option<FogGap> {
    let window_start = today - Duration::days(window_days);

    // Sans historique, il n'y a pas de trou : il n'y a rien, et l'import de
    // relevés reste la voie normale pour remonter le temps.
    let newest = newest_known?;
    let first_missing = newest + Duration::days(1);
    let last_missing = window_start - Duration::days(1);

    (first_missing <= last_missing).then(|| FogGap {
        account_id: account_id.to_string(),
        from: first_missing,
        to: last_missing,
    })
}

/// Accès restants avant le plafond quotidien.
pub fn remaining_access(used_today: u32) -> u32 {
    DAILY_ACCESS_LIMIT.saturating_sub(used_today)
}

fn at(day: NaiveDate, hour: u32) -> DateTime<Utc> {
    let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default();
    Utc.from_utc_datetime(&day.and_time(time))
}

/// Petit confort : l'heure d'un `NaiveTime`, sur 24 heures.
trait Hour24 {
    fn hour24(&self) -> u32;
}

impl Hour24 for NaiveTime {
    fn hour24(&self) -> u32 {
        use chrono::Timelike;
        self.hour()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn day(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    #[test]
    fn three_slots_a_day_by_default() {
        assert_eq!(Schedule::default().daily_slots(), 3);
    }

    /// Trois relevés d'opérations et un solde tiennent dans le plafond.
    #[test]
    fn the_default_schedule_fits_the_daily_limit() {
        let accesses = Schedule::default().daily_slots() as u32 + 1;
        assert!(
            accesses <= DAILY_ACCESS_LIMIT,
            "{accesses} accès pour un plafond de {DAILY_ACCESS_LIMIT}"
        );
    }

    #[test]
    fn finds_the_next_slot() {
        let schedule = Schedule::default();
        assert_eq!(
            schedule.next_slot(moment("2026-08-30T07:00:00Z")),
            moment("2026-08-30T12:00:00Z")
        );
        // Après le dernier créneau, le premier du lendemain.
        assert_eq!(
            schedule.next_slot(moment("2026-08-30T22:00:00Z")),
            moment("2026-08-31T06:00:00Z")
        );
    }

    #[test]
    fn waits_when_no_slot_has_passed() {
        let schedule = Schedule::default();
        let decision = schedule.decide(
            Some(moment("2026-08-30T12:05:00Z")),
            moment("2026-08-30T14:00:00Z"),
        );
        assert_eq!(
            decision,
            Decision::WaitUntil(moment("2026-08-30T19:00:00Z"))
        );
    }

    #[test]
    fn syncs_when_a_slot_has_passed() {
        let schedule = Schedule::default();
        let decision = schedule.decide(
            Some(moment("2026-08-30T07:00:00Z")),
            moment("2026-08-30T12:30:00Z"),
        );
        assert_eq!(decision, Decision::SyncNow { missed: 1 });
    }

    /// Machine éteinte deux jours : le rattrapage doit être immédiat, et le
    /// nombre de créneaux manqués connu.
    #[test]
    fn counts_slots_missed_while_stopped() {
        let schedule = Schedule::default();
        let decision = schedule.decide(
            Some(moment("2026-08-28T06:00:00Z")),
            moment("2026-08-30T13:00:00Z"),
        );
        assert_eq!(decision, Decision::SyncNow { missed: 7 });
    }

    /// Sans historique, on ne fait pas attendre l'utilisateur.
    #[test]
    fn syncs_immediately_on_a_fresh_install() {
        assert_eq!(
            Schedule::default().decide(None, moment("2026-08-30T14:00:00Z")),
            Decision::SyncNow { missed: 0 }
        );
    }

    /// Cas normal : l'historique local touche la fenêtre de la banque.
    #[test]
    fn reports_no_gap_when_the_history_is_current() {
        assert_eq!(
            detect_gap("acc", Some(day("2026-08-29")), day("2026-08-30"), 90),
            None
        );
    }

    /// Après une longue absence, une période échappe définitivement à l'API.
    #[test]
    fn reports_the_period_the_api_can_no_longer_reach() {
        // Dernière opération connue : il y a un an. La banque ne rend que les
        // 90 derniers jours ; entre les deux, personne.
        let gap = detect_gap("acc", Some(day("2025-08-30")), day("2026-08-30"), 90)
            .expect("un trou d'un an doit être signalé");
        assert_eq!(gap.from, day("2025-08-31"));
        // Le 1er juin est le premier jour que la banque rend encore : le
        // dernier jour perdu est donc la veille.
        assert_eq!(gap.to, day("2026-05-31"));
        assert_eq!(gap.days(), 274);
    }

    /// Juste à la limite : rien ne manque encore.
    #[test]
    fn reports_no_gap_at_the_edge_of_the_window() {
        assert_eq!(
            detect_gap("acc", Some(day("2026-06-01")), day("2026-08-30"), 90),
            None
        );
    }

    /// Un jour de plus, et le trou apparaît — d'un seul jour.
    #[test]
    fn reports_a_gap_one_day_past_the_window() {
        // Dernière opération le 30/05, fenêtre ouvrant le 01/06 : seul le 31/05
        // échappe aux deux.
        let gap = detect_gap("acc", Some(day("2026-05-30")), day("2026-08-30"), 90)
            .expect("un jour au-delà de la fenêtre doit être signalé");
        assert_eq!(gap.from, day("2026-05-31"));
        assert_eq!(gap.to, day("2026-05-31"));
        assert_eq!(gap.days(), 1);
    }

    #[test]
    fn reports_no_gap_without_history() {
        assert_eq!(detect_gap("acc", None, day("2026-08-30"), 90), None);
    }

    #[test]
    fn counts_remaining_access() {
        assert_eq!(remaining_access(0), 4);
        assert_eq!(remaining_access(4), 0);
        // Un dépassement ne doit pas produire de nombre négatif.
        assert_eq!(remaining_access(9), 0);
    }

    #[test]
    fn ignores_impossible_hours() {
        assert_eq!(Schedule::new(vec![6, 25, 12, 6]).daily_slots(), 2);
        // Un planning entièrement invalide retombe sur le planning par défaut.
        assert_eq!(Schedule::new(vec![99]).daily_slots(), 3);
    }
}
