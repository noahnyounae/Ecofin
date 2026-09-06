// Types miroirs de l'API Rust.
//
// Les montants voyagent en chaîne : un `number` JavaScript perdrait des
// centimes sur de gros soldes. Ils ne sont convertis qu'au moment de tracer,
// jamais pour additionner.

export interface Account {
  id: string;
  institution: string;
  name: string;
  iban: string | null;
  currency: string;
  balance: string | null;
  last_synced_at: string | null;
  /** Bornes de l'historique connu, pour cadrer les sélecteurs de période. */
  first_transaction: string | null;
  last_transaction: string | null;
}

/** Un point de la courbe : le solde juste après une opération. */
export interface CapitalPoint {
  transaction_id: string;
  account_id: string;
  date: string;
  balance: string;
  amount: string;
  /** Libellé nettoyé, tel qu'affiché. */
  description: string;
  /** Libellé brut de la banque : sert à la vérification et à la recherche. */
  raw: string;
  /** Secteur de dépense, sous son nom stable. */
  category: string;
  currency: string;
  booked: boolean;
}

export interface PeriodSummary {
  credits: string;
  debits: string;
  net: string;
  count: number;
}

export interface CapitalCurve {
  account_id: string;
  currency: string;
  points: CapitalPoint[];
  reference_balance: string;
  reference_date: string | null;
  /** Vrai quand aucun solde bancaire n'est connu : la courbe part de zéro. */
  relative: boolean;
  summary: PeriodSummary;
}

export interface TransactionDetail {
  id: string;
  account_id: string;
  amount: string;
  currency: string;
  booking_date: string | null;
  value_date: string | null;
  /** Libellé nettoyé, tel qu'affiché. */
  description: string;
  /** Libellé brut de la banque, pour vérifier ce que le nettoyage a retiré. */
  raw_description: string;
  /** Secteur de dépense, sous son nom stable. */
  category: string;
  /** Vrai si la catégorie vient d'une règle posée par l'utilisateur. */
  category_is_manual: boolean;
  counterparty: string | null;
  bank_category: string | null;
  booked: boolean;
  /** « banque » ou « relevé ». */
  source: string;
}

/** Un prélèvement récurrent détecté dans l'historique. */
export interface Subscription {
  /** Libellé nettoyé, commun aux échéances. */
  label: string;
  cadence: "weekly" | "monthly" | "quarterly" | "yearly";
  /** Montant représentatif d'une échéance. */
  amount: string;
  /** Coût ramené au mois, pour comparer des rythmes différents. */
  monthly_cost: string;
  occurrences: number;
  first_seen: string;
  last_seen: string;
  /** Part des intervalles respectant le rythme, entre 0 et 1. */
  regularity: number;
  /** Faux quand plus aucune échéance n'est venue depuis deux périodes. */
  active: boolean;
  /** Vrai si les montants varient sensiblement d'une échéance à l'autre. */
  variable_amount: boolean;
}

/** Une période que l'API bancaire ne peut plus rendre. */
export interface FogGap {
  account_id: string;
  /** Premier jour manquant. */
  from: string;
  /** Dernier jour manquant. */
  to: string;
}

/** Une catégorie disponible. */
export interface CategoryInfo {
  /** Nom stable, utilisé dans les échanges. Ne change jamais. */
  key: string;
  /** Nom affiché, modifiable. */
  label: string;
  /** Faux pour les mouvements qui ne consomment rien. */
  is_spending: boolean;
  /**
   * Catégorie dont celle-ci relève, quand elle en relève d'une.
   *
   * « Essence » a « Transport » pour parent : chercher ou totaliser
   * « Transport » embrasse alors « Essence ».
   */
  parent: string | null;
  /**
   * Vraie pour les catégories d'origine.
   *
   * Des motifs automatiques les désignent : elles ne se retirent qu'en
   * redirigeant vers une autre, sous peine de faire taire ces motifs.
   */
  builtin: boolean;
}

/** Une règle de classement posée par l'utilisateur. */
export interface CategoryRule {
  /** Libellé nettoyé du bénéficiaire. */
  label: string;
  category: string;
}

/** Un portefeuille virtuel, avec son état sur le mois observé. */
export interface WalletInfo {
  id: number;
  name: string;
  /** Dotation mensuelle, en texte pour que les décimales traversent intactes. */
  allocation: string;
  /** Catégories désignées, avant extension à leurs sous-catégories. */
  categories: string[];
  /** Portefeuille qui reçoit le reliquat de fin de mois. */
  carry_to: number | null;
  /**
   * Portefeuille qui contient celui-ci.
   *
   * La mère dote ses filles : sa dotation est le budget de toute la branche.
   */
  parent: number | null;
  /** Ce qui finance réellement l'enveloppe, une fois ses filles servies. */
  effective_allocation: string;
  /** Somme confiée aux filles directes. */
  children_allocation: string;
  /** Ce qui reste sur toute la branche, filles comprises. */
  branch_balance: string;
  /** Vrai quand les filles réclament plus que la mère ne dispose. */
  overallocated: boolean;
  /** Premier mois doté, écrit `AAAA-MM`. */
  start: string;
  /** Reliquat reçu du mois précédent. */
  carried_in: string;
  /** Apports ponctuels versés ce mois-ci, hors dotation. */
  contributions: string;
  /** Le détail de ces apports, pour pouvoir en annuler un. */
  contribution_entries: ContributionInfo[];
  /** Somme signée des mouvements du mois : négative quand on a dépensé. */
  movements: string;
  /** Ce dont l'enveloppe disposait avant toute dépense. */
  available: string;
  /** Ce qu'il reste. */
  balance: string;
}

/** Un apport ponctuel versé à une enveloppe. */
export interface ContributionInfo {
  id: number;
  /** Signé : négatif, l'apport est un retrait. */
  amount: string;
  note: string | null;
}

/** L'état des enveloppes sur un mois. */
export interface WalletsState {
  month: string;
  wallets: WalletInfo[];
  account_balance: string;
  /**
   * Ce que les enveloppes ne couvrent pas.
   *
   * C'est cette part qui rend vraie la promesse « la somme des portefeuilles
   * vaut le solde du compte ».
   */
  unallocated: string;
}
