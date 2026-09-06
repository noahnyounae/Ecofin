import type {
  Account,
  CapitalCurve,
  CategoryInfo,
  CategoryRule,
  FogGap,
  Subscription,
  TransactionDetail,
  WalletsState,
} from "./types";

/** Bornes d'une période, en RFC 3339, telles que l'API les attend. */
export interface Period {
  from: string | null;
  to: string | null;
}

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(path, {
    signal,
    headers: { Accept: "application/json" },
  });

  if (response.status === 401) {
    // Session expirée ou absente : le front doit repasser par la connexion,
    // pas afficher un message d'erreur technique.
    throw new Unauthenticated();
  }

  if (!response.ok) {
    // Le serveur renvoie ses erreurs en JSON ; on préfère son message au code.
    const message = await response
      .json()
      .then((body: { error?: string }) => body.error)
      .catch(() => null);
    throw new Error(message ?? `${response.status} ${response.statusText}`);
  }
  return response.json() as Promise<T>;
}

export function fetchAccounts(signal?: AbortSignal): Promise<Account[]> {
  return get<Account[]>("/api/accounts", signal);
}

export function fetchCapital(
  accountId: string,
  period: Period,
  signal?: AbortSignal,
): Promise<CapitalCurve> {
  const params = new URLSearchParams({ account: accountId });
  if (period.from) params.set("from", period.from);
  if (period.to) params.set("to", period.to);
  return get<CapitalCurve>(`/api/capital?${params}`, signal);
}

export function fetchTransaction(
  id: string,
  signal?: AbortSignal,
): Promise<TransactionDetail> {
  return get<TransactionDetail>(`/api/transactions/${encodeURIComponent(id)}`, signal);
}

// --- Authentification ------------------------------------------------------

export interface Session {
  email: string;
}

/** Levée quand le serveur exige une session : le front affiche la connexion. */
export class Unauthenticated extends Error {
  constructor() {
    super("session requise");
    this.name = "Unauthenticated";
  }
}

export async function login(email: string, password: string): Promise<Session> {
  const response = await fetch("/api/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password }),
  });

  if (!response.ok) {
    const message = await response
      .json()
      .then((body: { error?: string }) => body.error)
      .catch(() => null);
    throw new Error(message ?? "connexion refusée");
  }
  return response.json() as Promise<Session>;
}

export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" });
}

/** Session en cours, ou `null` si aucune. */
export async function currentSession(signal?: AbortSignal): Promise<Session | null> {
  const response = await fetch("/api/auth/me", { signal });
  if (response.status === 401) return null;
  if (!response.ok) throw new Error("état de session indéterminable");
  return response.json() as Promise<Session>;
}

export function fetchSubscriptions(
  accountId: string,
  signal?: AbortSignal,
): Promise<Subscription[]> {
  const params = new URLSearchParams({ account: accountId });
  return get<Subscription[]>(`/api/subscriptions?${params}`, signal);
}

export function fetchGaps(signal?: AbortSignal): Promise<FogGap[]> {
  return get<FogGap[]>("/api/gaps", signal);
}

// --- Catégories ------------------------------------------------------------

export function fetchCategories(signal?: AbortSignal): Promise<CategoryInfo[]> {
  return get<CategoryInfo[]>("/api/categories", signal);
}

export function fetchRules(signal?: AbortSignal): Promise<CategoryRule[]> {
  return get<CategoryRule[]>("/api/rules", signal);
}

/**
 * Pose ou remplace la règle d'un bénéficiaire.
 *
 * La règle porte sur le libellé, pas sur l'opération : classer un achat classe
 * toutes les opérations du même commerçant.
 */
export async function setRule(label: string, category: string): Promise<void> {
  const response = await fetch("/api/rules", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label, category }),
  });
  if (!response.ok) throw new Error("le classement n'a pas pu être enregistré");
}

/** Retire une règle, rendant le libellé au classement automatique. */
export async function deleteRule(label: string): Promise<void> {
  const response = await fetch(`/api/rules/${encodeURIComponent(label)}`, {
    method: "DELETE",
  });
  if (!response.ok) throw new Error("la règle n'a pas pu être retirée");
}

export async function addCategory(
  label: string,
  isSpending: boolean,
  parent?: string | null,
): Promise<CategoryInfo> {
  const response = await fetch("/api/categories", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label, is_spending: isSpending, parent: parent || null }),
  });
  if (!response.ok) throw new Error(await errorOf(response));
  return response.json() as Promise<CategoryInfo>;
}

export async function renameCategory(key: string, label: string): Promise<void> {
  const response = await fetch(`/api/categories/${encodeURIComponent(key)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label }),
  });
  if (!response.ok) throw new Error(await errorOf(response));
}

/**
 * Range une catégorie sous une autre, ou la ramène à la racine.
 *
 * `null` est une valeur pleine — « plus de parent » — et non une absence :
 * d'où une route à part, où les deux ne pourraient pas se distinguer.
 */
export async function setCategoryParent(
  key: string,
  parent: string | null,
): Promise<void> {
  const response = await fetch(`/api/categories/${encodeURIComponent(key)}/parent`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ parent }),
  });
  if (!response.ok) throw new Error(await errorOf(response));
}

/**
 * Retire une catégorie.
 *
 * `into` désigne celle qui reçoit ses motifs automatiques. Il est exigé pour
 * une catégorie d'origine, inutile pour une catégorie créée.
 */
export async function removeCategory(key: string, into?: string): Promise<void> {
  const params = into ? `?into=${encodeURIComponent(into)}` : "";
  const response = await fetch(
    `/api/categories/${encodeURIComponent(key)}${params}`,
    { method: "DELETE" },
  );
  if (!response.ok) throw new Error(await errorOf(response));
}

// --- Portefeuilles ---------------------------------------------------------

export function fetchWallets(
  account: string,
  month?: string,
  signal?: AbortSignal,
): Promise<WalletsState> {
  const params = new URLSearchParams({ account });
  if (month) params.set("month", month);
  return get<WalletsState>(`/api/wallets?${params}`, signal);
}

export async function addWallet(
  name: string,
  allocation: string,
  categories: string[],
  parent: number | null = null,
): Promise<void> {
  const response = await fetch("/api/wallets", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, allocation, categories, parent }),
  });
  if (!response.ok) throw new Error(await errorOf(response));
}

export async function updateWallet(
  id: number,
  name: string,
  allocation: string,
  categories: string[],
  carryTo: number | null,
  parent: number | null,
): Promise<void> {
  const response = await fetch(`/api/wallets/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, allocation, categories, carry_to: carryTo, parent }),
  });
  if (!response.ok) throw new Error(await errorOf(response));
}

export async function removeWallet(id: number): Promise<void> {
  const response = await fetch(`/api/wallets/${id}`, { method: "DELETE" });
  if (!response.ok) throw new Error(await errorOf(response));
}

/** Message d'erreur du serveur, ou un repli lisible. */
async function errorOf(response: Response): Promise<string> {
  return response
    .json()
    .then((body: { error?: string }) => body.error ?? "opération refusée")
    .catch(() => "opération refusée");
}
