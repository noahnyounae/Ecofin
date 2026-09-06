# ecofin

Suivi de comptes bancaires en ligne de commande. Récupère soldes et opérations
via l'API DSP2 d'[Enable Banking](https://enablebanking.com) et les conserve
dans une base SQLite locale.

Première étape d'une application web de suivi de finances — le CLI valide
l'accès aux données ; le modèle de domaine, le stockage et la logique métier
sont écrits pour être réutilisés tels quels côté web.

## Pourquoi Enable Banking

En France, lire ses comptes bancaires par programme passe obligatoirement par un
agrégateur agréé AISP (DSP2). Enable Banking permet une inscription libre en
self-service, et son mode *Restricted Production* couvre gratuitement la lecture
de ses propres comptes, sous son propre agrément — donc sans certificat eIDAS
(~2 000 €/an) ni démarche ACPR.

GoCardless Bank Account Data (ex-Nordigen) n'accepte plus de nouvelles
inscriptions depuis juillet 2025 et n'est donc pas une option.

## Prérequis

1. Créer un compte sur <https://enablebanking.com/cp/>
2. Applications → **New application**
   - déclarer une URL de retour **en `https://`** — le schéma `http://` est
     refusé, y compris pour `localhost`
   - télécharger la **clé privée `.pem` à la création** — elle n'est plus
     téléchargeable ensuite
3. Noter l'**Application ID**

### L'URL de retour

Enable Banking impose HTTPS, ce qui empêche de rediriger directement vers un
serveur local. Deux montages fonctionnent.

**Tunnel HTTPS vers la machine (recommandé).** Un tunnel Cloudflare — ou tout
équivalent — expose `https://ecofin.exemple.eu/callback` et le fait aboutir sur
le port 8484 local. Le CLI intercepte alors le retour tout seul :

```sh
ecofin auth login \
  --application-id <ID> \
  --private-key ~/Downloads/key.pem \
  --redirect-url https://ecofin.exemple.eu/callback \
  --listen 127.0.0.1:8484          # 0.0.0.0:8484 dans un conteneur
```

`--listen` est ce qui découple « où la banque redirige » de « où le CLI
écoute ». Sans cette option, une URL de retour publique bascule en saisie
manuelle.

**Sans tunnel.** Déclare n'importe quelle page HTTPS que tu contrôles. Après
l'authentification, recopie l'URL complète depuis la barre d'adresse : le CLI la
demande et en extrait le code. [`docs/callback.html`](docs/callback.html) est une
page prête à héberger qui affiche l'URL avec un bouton de copie.

## Installation

```sh
cargo build --release
# le binaire est dans target/release/ecofin
```

### Avec Docker et un tunnel Cloudflare

Le tunnel résout l'exigence HTTPS d'Enable Banking sans rien exposer : il publie
`https://ecofin.exemple.eu/callback` et le fait aboutir sur le port 8484 du
conteneur, à travers une connexion sortante.

**1. Créer le tunnel.** Dans Cloudflare Zero Trust → Networks → Tunnels, créer
un tunnel, puis dans l'onglet *Public Hostname* déclarer :

| Champ | Valeur |
|---|---|
| Subdomain / Domain | `ecofin.exemple.eu` |
| Path | `callback` |
| Service | `HTTP` → `ecofin:8484` |

**2. Poser la clé privée.** Elle reste sur l'hôte, montée en lecture seule :
elle n'entre ni dans une couche d'image ni dans un volume.

```sh
cp ~/Downloads/<fichier>.pem secrets/enablebanking.pem
chmod 600 secrets/enablebanking.pem
```

Le `chmod` n'est pas décoratif : ecofin refuse de lire une clé accessible à
d'autres utilisateurs, et Docker préserve les droits sur le montage. Le
répertoire `secrets/` est exclu du dépôt et du contexte de build.

**3. Renseigner `.env`.**

```sh
cp .env.example .env
```

| Variable | Contenu |
|---|---|
| `CLOUDFLARE_TUNNEL_TOKEN` | la valeur après `--token` de l'écran d'installation |
| `ECOFIN_APPLICATION_ID` | l'identifiant affiché par Enable Banking |
| `ECOFIN_PRIVATE_KEY` | `/run/secrets/enablebanking.pem` (vu du conteneur) |
| `ECOFIN_REDIRECT_URL` | `https://ecofin.exemple.eu/callback` |
| `ECOFIN_LISTEN` | `0.0.0.0:8484` |

Ces variables suffisent : **`auth login` devient inutile**, la configuration se
construit entièrement depuis l'environnement et les conteneurs restent
jetables. Une variable laissée vide est ignorée plutôt qu'appliquée, donc un
`.env` partiel n'écrase pas une configuration déjà enregistrée.

**4. Démarrer.**

```sh
docker compose up -d cloudflared
docker compose build ecofin
docker compose run --rm ecofin auth status   # valide identifiant et clé
```

**5. Vérifier le tunnel avant tout.** Cette commande démarre le même serveur
que `link new`, sans toucher à la banque :

```sh
docker compose run --rm --use-aliases ecofin link check
```

Elle affiche une URL à ouvrir ; si `OK` s'affiche, le retour d'autorisation
arrive bien. Un tunnel mal routé se découvre ainsi en quelques secondes, au lieu
d'échouer au milieu d'une authentification bancaire qu'il faudrait recommencer.

`link check` accepte ses deux options sans configuration préalable : le tunnel
se valide donc **avant** d'avoir déclaré l'application Enable Banking.

**6. Connecter une banque.** `--use-aliases` est indispensable :

```sh
docker compose run --rm --use-aliases ecofin link new "Credit Agricole"
```

#### Les trois pièges

- **`--use-aliases`.** Sans lui, `docker compose run` crée un conteneur sans
  l'alias réseau du service, et `cloudflared` ne sait pas résoudre `ecofin` :
  le retour d'autorisation part en erreur 502.
- **`0.0.0.0:8484`, pas `127.0.0.1`.** Dans un conteneur, la boucle locale
  n'est visible que du conteneur lui-même ; ni le tunnel ni le port publié
  n'atteindraient le listener.
- **La clé privée n'entre pas dans l'image.** `.dockerignore` exclut `*.pem`,
  `*.db`, `config.toml` et `.env` : aucun secret ne peut se retrouver dans une
  couche, même par inadvertance. Le volume `ecofin-data` conserve
  configuration, clé et base entre deux exécutions.

#### Compilation incrémentale

Le `Dockerfile` sépare les dépendances du code via
[cargo-chef](https://github.com/LukeMathWalker/cargo-chef) : la couche longue
n'est invalidée que par `Cargo.toml` ou `Cargo.lock`. Modifier `src/` ne
recompile qu'ecofin.

Pour itérer sans reconstruire d'image du tout, le service `dev` garde `target/`
et le registre cargo dans des volumes nommés :

```sh
docker compose run --rm dev              # premier build, long
# puis, dans le conteneur :
cargo build      # incrémental, quelques secondes
cargo test
```

Les volumes nommés sont délibérés : un bind mount de `target/` depuis macOS
serait beaucoup plus lent que le système de fichiers du conteneur.

## Utilisation

```sh
# 1. Enregistrer l'application (valide les identifiants auprès de l'API)
ecofin auth login --application-id <ID> --private-key ~/Downloads/key.pem

# 2. Trouver sa banque (le nom doit être exact ensuite)
ecofin banks "credit agricole"

# 3. Connecter — ouvre le navigateur, attend l'authentification,
#    importe les comptes automatiquement
ecofin link new "Credit Agricole"

# 4. Récupérer soldes et opérations
ecofin sync

# 5. Consulter (lecture locale, aucun accès réseau)
ecofin balance
ecofin tx
ecofin tx --account "Livret" --since 2026-01-01
ecofin tx -q monoprix --limit 100
```

Compléter l'historique avec les relevés téléchargés :

```sh
# Dépose les relevés dans imports/, puis :
ecofin import imports/ --dry-run   # analyse sans rien écrire
ecofin import imports/             # verse en base, sans doublon
```

Gestion des consentements :

```sh
ecofin link list      # échéances et état
ecofin link status    # relit l'état côté API
ecofin link check     # vérifie que le retour d'autorisation arrive
ecofin auth status    # vérifie application, clé, et couverture
```

## Application web

Un graphe du capital dans le temps, un point par opération, cliquable pour
ouvrir le détail dans un panneau latéral. La période se choisit à la date et à
l'heure au-dessus du graphe.

### Lancer

```sh
# 1. Créer un compte de connexion (crée aussi le secret de session)
docker compose run --rm ecofin user add ton@adresse.fr

# 2. Démarrer le serveur et son point d'entrée
docker compose up -d traefik web
```

Le CLI doit tourner au moins une fois avant le serveur : lui seul peut écrire,
donc lui seul applique les migrations de schéma. Le serveur refuse d'ouvrir une
base dont la version ne correspond pas à ce qu'il sait lire.

**En local** : <http://localhost:8080>

C'est Traefik qui écoute sur ce port, comme en production : le routage est donc
éprouvé dès le développement, et l'application n'est jamais atteinte par un
chemin qui n'existerait qu'en local.

Sur `http://`, il faut lever la protection des cookies, sinon le navigateur ne
les renvoie jamais et la connexion échoue sans message clair :

```sh
echo 'ECOFIN_INSECURE_COOKIES=1' >> .env
docker compose up -d traefik web
```

### Exposer par le tunnel

Un seul point d'entrée : **Traefik**. Il aiguille `/callback` vers le listener
de retour d'autorisation bancaire et tout le reste vers l'application.

```
Internet → Cloudflare → tunnel → traefik:80 ─┬─ /callback → ecofin:8484
                                             └─ /*        → web:8080
```

Le tunnel n'a donc qu'une seule entrée à déclarer, dans Cloudflare Zero Trust →
Networks → Tunnels → *Public Hostname* :

| Domaine | Path | Service |
|---|---|---|
| `ecofin.exemple.eu` | *(vide)* | `HTTP` → `traefik:80` |

L'application est alors à l'adresse **`https://ecofin.exemple.eu`**, et
`ECOFIN_INSECURE_COOKIES` doit rester vide : le tunnel sert du HTTPS.

Le routage lui-même vit dans [`traefik/dynamic/routes.yml`](traefik/dynamic/routes.yml),
versionné — contrairement à une configuration saisie dans une interface web.
Deux points s'y jouent :

- **La priorité.** `/callback` porte une priorité supérieure à la route
  fourre-tout. Sans cela, l'application capterait tout et le retour
  d'autorisation bancaire n'arriverait jamais.
- **Le socket Docker n'est pas monté.** Traefik sait découvrir les conteneurs
  tout seul, mais au prix de `/var/run/docker.sock`, qui équivaut à un accès
  root sur l'hôte. Deux destinations connues d'avance ne le justifient pas.

Hors d'un `link new`, `/callback` répond 502 : rien n'écoute, et c'est
l'information juste.

### Ce que le serveur ne peut pas faire

Il ouvre la base avec une **connexion SQLite en lecture seule** : aucune
écriture n'est possible, même par erreur de programmation. Il n'appelle jamais
la banque non plus — synchroniser et importer restent le travail du CLI. Le
serveur ne peut donc ni consommer le quota DSP2, ni corrompre l'historique.

Le fichier de base reste malgré tout accessible en écriture sur le disque, et
ce n'est pas une contradiction : SQLite en mode WAL doit pouvoir créer son
fichier d'index partagé, même pour lire. Un volume monté en lecture seule
empêcherait la base de s'ouvrir du tout.

### Accès et cloisonnement

L'accès exige un compte, créé depuis le CLI — le serveur, en lecture seule, ne
peut pas en créer. Le mot de passe est haché en Argon2id, la session voyage
dans un cookie `HttpOnly` valable 12 h, et cinq échecs bloquent l'adresse
pendant cinq minutes.

**Chaque utilisateur ne voit que ses propres comptes bancaires.** Le filtrage
n'est pas laissé à la discipline : un type `Scope` est exigé par le compilateur
à chaque requête sur des données bancaires, et les requêtes par compte joignent
`accounts` pour en hériter. Un compte appartenant à quelqu'un d'autre répond
exactement comme un identifiant inventé — impossible d'en déduire l'existence.

```sh
ecofin user list                    # comptes de connexion
ecofin user passwd <adresse>        # change le mot de passe et ferme les sessions
ecofin user revoke <adresse>        # ferme les sessions sans changer le mot de passe
ecofin link new "LCL" --user <adresse>   # rattache une banque à un utilisateur
```

## Fonctionnement

```
cli.rs ──▶ main.rs ──┬──▶ providers/enablebanking/  (le seul à parler réseau)
                     │      ├── auth.rs    JWT RS256 + serveur de callback local
                     │      ├── client.rs  appels API, conversion vers le domaine
                     │      └── types.rs   DTO calqués sur le JSON de l'API
                     │
                     ├──▶ store.rs   SQLite : comptes, soldes, opérations
                     ├──▶ model.rs   types de domaine, neutres vis-à-vis du provider
                     └──▶ ui.rs      tableaux, montants au format français
```

**Seule la commande `sync` accède au réseau.** Tout le reste lit la base locale.
Conséquence utile : l'historique reste consultable après l'expiration du
consentement, et au-delà de la fenêtre que la banque accepte d'exposer.

**`model.rs`, `store.rs` et `ui.rs` ignorent tout de la DSP2.** Ajouter un import
CSV ou basculer vers Powens ne demande d'écrire qu'un nouveau module dans
`providers/`.

### Points d'implémentation notables

- **Signe des montants** — Enable Banking transmet un montant toujours positif
  accompagné d'un sens (`CRDT`/`DBIT`). Le signe est appliqué à la conversion,
  pour que le reste du code manipule des montants signés
  ([client.rs](src/providers/enablebanking/client.rs)).
- **Montants en `TEXT`** — SQLite n'a pas de type décimal ; passer par un
  flottant fait perdre des centimes. Stockage en chaîne, `rust_decimal` en
  mémoire.
- **Déduplication** — toutes les banques ne fournissent pas de référence
  d'écriture. À défaut, une clé est synthétisée depuis le contenu de
  l'opération, ce qui évite d'empiler des doublons à chaque synchronisation.
- **Synchronisation incrémentale** — `sync` reprend une semaine avant la
  dernière opération connue : les opérations en attente sont réécrites quand
  elles se comptabilisent, et leur date peut reculer. `--full` repart de zéro.
- **Secrets** — clé privée et configuration écrites en `0600`, avec refus de
  lecture si les droits sont plus permissifs.

## Fichiers

| Chemin | Contenu |
|---|---|
| `~/.config/ecofin/config.toml` | Application ID, chemin de clé, URL de retour |
| `~/.config/ecofin/enablebanking.pem` | Clé privée RSA |
| `~/.config/ecofin/ecofin.db` | Comptes, soldes, opérations |

En Docker, ce répertoire est le volume `ecofin-data` monté sur `/data`, et la
clé vient du montage `./secrets` en lecture seule plutôt que du volume.

## Variables d'environnement

Elles surchargent le fichier de configuration, et suffisent à s'en passer
entièrement : avec `ECOFIN_APPLICATION_ID` et `ECOFIN_PRIVATE_KEY`, le CLI
fonctionne sans qu'`auth login` ait jamais été lancé. Une variable vide est
ignorée, pas appliquée.

| Variable | Rôle |
|---|---|
| `ECOFIN_APPLICATION_ID` | Identifiant d'application Enable Banking |
| `ECOFIN_PRIVATE_KEY` | Chemin de la clé privée RSA |
| `ECOFIN_REDIRECT_URL` | URL de retour déclarée côté Enable Banking |
| `ECOFIN_LISTEN` | Adresse `hôte:port` d'écoute du retour d'autorisation |
| `ECOFIN_HOME` | Répertoire de configuration et de données |

`ECOFIN_HOME` est pratique pour tester sans toucher à sa configuration réelle.

## Ce qui est mis en cache

Toute donnée récupérée du réseau est écrite en SQLite ; les commandes de lecture
n'y touchent jamais.

| Donnée | Conservation |
|---|---|
| Opérations | Ajoutées, jamais écrasées |
| Comptes, sessions | Mis à jour |
| **Soldes** | **Historisés** : une ligne par date d'arrêté |
| Établissements | Cache d'une semaine, `banks --refresh` pour forcer |
| Journal des appels | Ajouté, sert au garde-fou DSP2 |

Les soldes étaient auparavant écrasés à chaque synchronisation, ne laissant que
le dernier instantané. Ils forment désormais une série :

```sh
ecofin balance --history
```

C'est le solde *déclaré par la banque* à chaque arrêté, à distinguer de celui
qu'impliquent les opérations. Disposer des deux permet de les confronter — le
contrôle qui a validé l'import des quatre années d'historique.

`auth status` interroge l'API à chaque appel, délibérément : son rôle est de
dire l'état courant côté Enable Banking, pas de le rejouer depuis un cache.

## Import de relevés

L'API ne remonte que ce que la banque expose — 90 jours chez LCL. Les relevés
mensuels téléchargés depuis l'espace bancaire remontent, eux, à l'ouverture du
compte. `ecofin import` les verse dans la même base.

Formats lus : **relevés LCL en PDF** et **CSV** (séparateur `;`, tabulation ou
`,` ; montant signé ou colonnes débit/crédit séparées).

Trois difficultés que le format LCL impose, et leur traitement :

- **Le sens d'une opération n'est porté que par sa position.** Aucun signe :
  un montant est un débit ou un crédit selon la colonne. Les montants étant
  alignés à droite, c'est leur position de *fin* qui les situe — `1,00`
  commence plus à droite que `101,00` dans la même colonne. Et les colonnes se
  décalent d'une page à l'autre, donc l'en-tête est relu à chaque page.
- **Les dates n'ont pas d'année** (`02.07`), déduite de la période du relevé.
  Le relevé de janvier porte un ancien solde daté du 31.12, qui appartient à
  l'année précédente.
- **Aucune référence d'écriture**, donc rien à quoi rattacher une
  déduplication par identifiant. Le recouvrement avec les données de l'API est
  résolu en comptant les couples (date, montant) : deux achats identiques le
  même jour restent deux opérations, alors qu'un simple test de présence en
  perdrait une.

Chaque relevé PDF est **vérifié contre le solde qu'il imprime**. Un écart est
signalé plutôt qu'enregistré en silence : un changement de mise en page devient
une erreur visible, pas des données fausses.

`pdftotext` (poppler) est requis pour les PDF — `brew install poppler` sous
macOS. L'image Docker l'embarque.

## Limites connues

- **Consentement à renouveler tous les 90 jours**, plafond réglementaire DSP2.
  `ecofin link list` affiche les échéances ; `ecofin link new` reconnecte.
- **Couverture variable selon la banque**, en particulier pour les caisses
  régionales (Crédit Agricole, Crédit Mutuel). À vérifier avec `ecofin banks`
  avant d'aller plus loin.
- **Mode restreint limité à ses propres comptes.** Connecter les comptes de
  tiers impose de basculer en production complète, par contrat.
- **Épargne et investissement mal couverts** par la DSP2 : Livret A et PEA
  remontent parfois, un contrat d'assurance-vie ou un courtier étranger
  quasiment jamais.

## Développement

```sh
cargo test    # 23 tests : conversion, format, serveur de callback
cargo clippy
```

Les tests du serveur de callback ouvrent de vraies sockets TCP sur un port
attribué par le système, et couvrent le cas nominal, la vérification du `state`,
le refus de la banque, et les requêtes parasites du navigateur.
