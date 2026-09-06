// Mise en forme à la française, partagée par tout le front.

const MONEY = new Intl.NumberFormat("fr-FR", {
  style: "currency",
  currency: "EUR",
  minimumFractionDigits: 2,
});

const MONEY_COMPACT = new Intl.NumberFormat("fr-FR", {
  notation: "compact",
  maximumFractionDigits: 1,
});

const DATE = new Intl.DateTimeFormat("fr-FR", { dateStyle: "medium" });
const DATE_SHORT = new Intl.DateTimeFormat("fr-FR", { day: "2-digit", month: "short" });

export function money(raw: string | number, currency = "EUR"): string {
  const value = typeof raw === "string" ? Number(raw) : raw;
  if (!Number.isFinite(value)) return "—";
  return currency === "EUR"
    ? MONEY.format(value)
    : new Intl.NumberFormat("fr-FR", { style: "currency", currency }).format(value);
}

/** Format court pour les axes, où la place manque. */
export function moneyCompact(value: number): string {
  return `${MONEY_COMPACT.format(value)} €`;
}

export function date(raw: string | null | undefined): string {
  if (!raw) return "—";
  const parsed = new Date(raw);
  return Number.isNaN(parsed.getTime()) ? "—" : DATE.format(parsed);
}

export function dateShort(raw: string): string {
  const parsed = new Date(raw);
  return Number.isNaN(parsed.getTime()) ? raw : DATE_SHORT.format(parsed);
}

/** Accorde un nom avec son nombre. En français, zéro prend le singulier. */
export function plural(count: number, word: string): string {
  return `${count.toLocaleString("fr-FR")} ${word}${count > 1 ? "s" : ""}`;
}

/**
 * Convertit un montant décimal en centimes entiers.
 *
 * Les montants circulent en chaîne depuis SQLite précisément pour ne pas
 * traverser un flottant. Les additionner en `Number` rendrait cette précaution
 * vaine : `0.1 + 0.2` ne vaut pas `0.3`, et sur des centaines d'opérations les
 * écarts s'accumulent. On additionne donc des entiers, et on ne divise qu'à
 * l'affichage.
 *
 * Renvoie `NaN` sur une entrée illisible, que l'appelant doit écarter.
 */
export function toCents(raw: string): number {
  const parsed = /^(-?)(\d+)(?:\.(\d+))?$/.exec(raw.trim());
  if (!parsed) return Number.NaN;

  const [, sign, whole, fraction = ""] = parsed;
  // Deux décimales : au-delà, une banque ne facture pas.
  const cents = fraction.padEnd(2, "0").slice(0, 2);
  const value = Number(whole) * 100 + Number(cents);
  return sign === "-" ? -value : value;
}

/** Met en forme un montant exprimé en centimes. */
export function moneyFromCents(cents: number, currency = "EUR"): string {
  return money(cents / 100, currency);
}

/** Totaux d'un ensemble d'opérations, calculés en centimes. */
export interface Totals {
  credits: number;
  debits: number;
  net: number;
  count: number;
}

/**
 * Additionne les montants d'un ensemble d'opérations.
 *
 * Une opération au montant illisible est ignorée plutôt que de propager `NaN`
 * à tout le total : mieux vaut un total légèrement incomplet qu'un affichage
 * entièrement cassé par une seule ligne.
 */
export function totals(amounts: string[]): Totals {
  let credits = 0;
  let debits = 0;
  let count = 0;

  for (const raw of amounts) {
    const cents = toCents(raw);
    if (Number.isNaN(cents)) continue;
    if (cents < 0) debits += cents;
    else credits += cents;
    count += 1;
  }

  return { credits, debits, net: credits + debits, count };
}
