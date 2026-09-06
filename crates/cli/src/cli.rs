//! Définition de l'interface en ligne de commande.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ecofin",
    version,
    about = "Suivi de comptes bancaires et d'économies, en local",
    long_about = "ecofin récupère tes opérations bancaires via l'API DSP2 d'Enable \
                  Banking et les conserve dans une base SQLite locale.\n\n\
                  Seule la commande `sync` accède à ta banque : tout le reste lit \
                  la base locale, donc l'historique reste consultable même après \
                  l'expiration du consentement."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Gérer l'application Enable Banking et sa clé privée
    #[command(subcommand)]
    Auth(AuthCommand),

    /// Chercher un établissement bancaire couvert
    Banks {
        /// Filtre sur le nom, insensible à la casse (ex. « credit agricole »)
        query: Option<String>,

        /// Code pays ISO à deux lettres
        #[arg(short, long, default_value = "FR")]
        country: String,

        /// Retélécharger la liste au lieu d'utiliser le cache local
        #[arg(long)]
        refresh: bool,
    },

    /// Connecter une banque, ou gérer les consentements existants
    #[command(subcommand)]
    Link(LinkCommand),

    /// Lister les comptes connus
    Accounts,

    /// Récupérer soldes et opérations depuis la banque (accès réseau)
    Sync {
        /// Ne synchroniser qu'un compte (identifiant, préfixe, IBAN, ou libellé)
        #[arg(short, long)]
        account: Option<String>,

        /// Repartir de zéro au lieu de reprendre après la dernière opération
        #[arg(long)]
        full: bool,
    },

    /// Importer un relevé exporté depuis l'espace bancaire
    ///
    /// L'API ne remonte que ce que la banque expose — 90 jours chez LCL.
    /// Un export manuel remonte à l'ouverture du compte : cette commande le
    /// verse dans la même base, sans créer de doublon avec ce qui est déjà là.
    ///
    /// Formats lus : relevés LCL en PDF, et CSV quel que soit le séparateur
    /// (`;`, tabulation, `,`), avec une colonne de montant signé ou deux
    /// colonnes débit et crédit.
    ///
    /// Chaque relevé PDF est vérifié contre le solde qu'il imprime : une
    /// lecture douteuse est signalée plutôt qu'enregistrée en silence.
    Import {
        /// Fichier à importer, ou dossier contenant plusieurs relevés
        file: PathBuf,

        /// Compte de destination (identifiant, préfixe, IBAN, ou libellé).
        /// Facultatif s'il n'y a qu'un seul compte.
        #[arg(short, long)]
        account: Option<String>,

        /// Analyser et afficher le résultat sans rien écrire en base
        #[arg(long)]
        dry_run: bool,
    },

    /// Gérer les comptes d'accès à l'application web
    #[command(subcommand)]
    User(UserCommand),

    /// Synchroniser automatiquement, à heures fixes
    ///
    /// Trois passages par jour, ce qui tient dans le plafond DSP2 de quatre
    /// accès quotidiens : les soldes ne sont relevés qu'une fois, les
    /// opérations à chaque passage.
    ///
    /// Un créneau manqué — machine éteinte, conteneur arrêté — est rattrapé
    /// dès le démarrage. Si l'interruption dépasse la fenêtre d'historique de
    /// la banque, la période devenue inaccessible est consignée comme zone de
    /// brouillard, à combler par un import de relevés.
    Daemon {
        /// Heures de synchronisation, en UTC (par défaut : 6, 12, 19)
        #[arg(long, value_delimiter = ',')]
        hours: Option<Vec<u32>>,

        /// Effectuer un seul passage puis rendre la main
        #[arg(long)]
        once: bool,
    },

    /// Lister les périodes que l'API ne peut plus rendre
    Gaps,

    /// Afficher les soldes en base
    Balance {
        /// Afficher l'évolution du solde plutôt que le seul dernier connu
        #[arg(long)]
        history: bool,

        /// Restreindre à un compte (identifiant, préfixe, IBAN, ou libellé)
        #[arg(short, long)]
        account: Option<String>,
    },

    /// Afficher les opérations en base
    Tx {
        /// Restreindre à un compte (identifiant, préfixe, IBAN, ou libellé)
        #[arg(short, long)]
        account: Option<String>,

        /// Ne montrer que les opérations à partir de cette date (AAAA-MM-JJ)
        #[arg(short, long)]
        since: Option<String>,

        /// Chercher dans le libellé et la contrepartie
        #[arg(short = 'q', long)]
        search: Option<String>,

        /// Nombre maximum de lignes
        #[arg(short, long, default_value_t = 40)]
        limit: usize,
    },
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Enregistrer l'identifiant d'application et la clé privée
    ///
    /// Crée d'abord une application sur https://enablebanking.com/cp/
    /// (Applications → New application). L'URL de retour doit être en https://
    /// — le schéma http:// est refusé, y compris pour localhost. Télécharge la
    /// clé privée au format PEM à la création : elle n'est plus téléchargeable
    /// ensuite.
    ///
    /// Avec un tunnel HTTPS vers cette machine, ajoute --listen pour que le CLI
    /// intercepte le retour au lieu de demander de recopier l'URL.
    Login {
        /// Identifiant d'application. Demandé de façon interactive s'il est omis.
        #[arg(long)]
        application_id: Option<String>,

        /// Chemin vers la clé privée .pem. Demandé s'il est omis.
        #[arg(long)]
        private_key: Option<String>,

        /// URL de retour, telle que déclarée dans l'application.
        ///
        /// Enable Banking exige le schéma https://.
        #[arg(long)]
        redirect_url: Option<String>,

        /// Adresse `hôte:port` où écouter le retour d'autorisation.
        ///
        /// À renseigner quand l'URL de retour est publique mais aboutit ici,
        /// par exemple via un tunnel Cloudflare. Dans un conteneur, utiliser
        /// `0.0.0.0:8484`. Sans cette option, une URL publique fait basculer
        /// en saisie manuelle de l'URL de retour.
        #[arg(long)]
        listen: Option<String>,
    },

    /// Vérifier que l'application et la clé enregistrées fonctionnent
    Status,
}

#[derive(Subcommand)]
pub enum LinkCommand {
    /// Connecter une banque
    ///
    /// Ouvre le navigateur, attend l'authentification, puis importe les comptes.
    New {
        /// Nom de la banque, tel que renvoyé par `ecofin banks`
        institution: String,

        /// Code pays ISO à deux lettres
        #[arg(short, long, default_value = "FR")]
        country: String,

        /// Durée du consentement en jours, plafonnée par la banque.
        ///
        /// La plupart des établissements français acceptent 180 jours ; la
        /// demande est ramenée automatiquement au maximum accepté.
        #[arg(long, default_value_t = 180)]
        days: i64,

        /// Type d'utilisateur : `personal` ou `business`
        #[arg(long, default_value = "personal")]
        psu_type: String,

        /// Ne pas ouvrir le navigateur automatiquement
        #[arg(long)]
        no_open: bool,

        /// Compte de connexion propriétaire des comptes bancaires importés.
        ///
        /// Facultatif s'il n'existe qu'un seul compte de connexion.
        #[arg(long)]
        user: Option<String>,
    },

    /// Relire l'état des consentements enregistrés
    Status {
        /// Identifiant de session, préfixe, ou nom de banque. Toutes si omis.
        id: Option<String>,
    },

    /// Lister les consentements enregistrés et leur échéance
    List,

    /// Reprendre une session déjà autorisée, à partir de son identifiant
    ///
    /// Le consentement vit côté Enable Banking. Si une session a été accordée
    /// par la banque mais n'a pas pu être enregistrée localement, elle reste
    /// exploitable : nul besoin de refaire le parcours d'authentification.
    Import {
        /// Identifiant de session renvoyé par l'API
        session_id: String,

        /// Compte de connexion propriétaire des comptes bancaires importés.
        #[arg(long)]
        user: Option<String>,
    },

    /// Vérifier que le retour d'autorisation parvient bien jusqu'ici
    ///
    /// Démarre le même serveur que `link new` et attend une requête, sans
    /// toucher à ta banque. À lancer avant une vraie connexion : un tunnel mal
    /// routé se découvre alors en quelques secondes, plutôt qu'au milieu d'une
    /// authentification bancaire qu'il faudrait recommencer.
    ///
    /// Fonctionne sans `auth login` si les deux options sont fournies, ce qui
    /// permet de valider un tunnel avant même de déclarer l'application.
    Check {
        /// URL de retour à tester. Par défaut, celle enregistrée.
        #[arg(long)]
        redirect_url: Option<String>,

        /// Adresse `hôte:port` où écouter. Par défaut, celle enregistrée.
        #[arg(long)]
        listen: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum UserCommand {
    /// Créer un compte d'accès à l'application web
    ///
    /// Crée aussi, au premier appel, le secret qui signe les sessions. Le
    /// serveur web monte la base en lecture seule : lui seul ne peut pas s'en
    /// charger.
    Add {
        /// Adresse de connexion
        email: String,
    },

    /// Lister les comptes existants
    List,

    /// Changer le mot de passe d'un compte
    ///
    /// Invalide du même coup les sessions ouvertes : un mot de passe changé
    /// parce qu'on le croit compromis ne doit pas en laisser vivre.
    Passwd { email: String },

    /// Fermer toutes les sessions ouvertes d'un compte
    Revoke { email: String },

    /// Supprimer un compte
    Remove { email: String },
}
