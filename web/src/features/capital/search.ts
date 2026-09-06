// Recherche approximative dans les libellés bancaires.
//
// La recherche porte sur le libellé **complet**, pas sur sa version nettoyée :
// un mot écarté à l'affichage — une référence, un nom de porteur — doit rester
// atteignable.

import { emptyTree, pathOf, withinBranch, type CategoryTree } from "../categories/tree";

/**
 * Tolérance aux fautes, en fonction de la longueur de la requête.
 *
 * Un seuil fixe ne convient pas, et la mesure sur 1405 libellés réels le montre
 * sans appel : à distance 3, « uber » correspond à *tous* les libellés, tandis
 * que « intermarche » en donne 91 quel que soit le seuil. Plus la requête est
 * courte, plus une substitution la dénature.
 *
 * En dessous de quatre caractères, aucune tolérance : la recherche se fait par
 * sous-chaîne stricte, sans quoi elle ramène tout.
 */
export function tolerance(length: number): number {
  return Math.min(3, Math.floor(length / 4));
}

/**
 * Retire accents et casse, pour que « crédit » trouve « CREDIT ».
 */
export function fold(text: string): string {
  return text
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toUpperCase();
}

/**
 * Distance de Levenshtein, abandonnée dès qu'elle dépasse `cap`.
 *
 * L'abandon anticipé compte : la recherche s'exécute à chaque frappe sur plus
 * d'un millier de libellés, et la plupart sont écartés dès les premières
 * lignes du calcul.
 */
function distance(a: string, b: string, cap: number): number {
  if (Math.abs(a.length - b.length) > cap) return cap + 1;

  let previous = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 1; i <= a.length; i += 1) {
    const current = new Array<number>(b.length + 1);
    current[0] = i;
    let best = i;
    for (let j = 1; j <= b.length; j += 1) {
      const substitution = previous[j - 1]! + (a[i - 1] === b[j - 1] ? 0 : 1);
      current[j] = Math.min(previous[j]! + 1, current[j - 1]! + 1, substitution);
      best = Math.min(best, current[j]!);
    }
    if (best > cap) return cap + 1;
    previous = current;
  }
  return previous[b.length]!;
}

/**
 * Vrai si le libellé contient la requête, à `cap` fautes près.
 *
 * On compare la requête à chaque fenêtre du libellé, et non au libellé entier :
 * « uber » face à `CB UBER *EATS 12/03/26` doit correspondre, alors que la
 * distance entre les deux chaînes complètes est énorme.
 */
export function matches(query: string, label: string, cap: number): boolean {
  if (label.includes(query)) return true;
  if (cap === 0) return false;

  for (let width = Math.max(1, query.length - cap); width <= query.length + cap; width += 1) {
    for (let start = 0; start + width <= label.length; start += 1) {
      if (distance(query, label.slice(start, start + width), cap) <= cap) return true;
    }
  }
  return false;
}

/**
 * Écarte à peu de frais les libellés qui ne peuvent pas correspondre.
 *
 * Si la distance d'édition entre la requête et une fenêtre vaut au plus `cap`,
 * alors au moins `longueur − cap` caractères de la requête figurent dans cette
 * fenêtre, donc dans le libellé. La réciproque est fausse, mais peu importe :
 * c'est une condition *nécessaire*, elle ne peut donc écarter aucune vraie
 * correspondance — seulement épargner le balayage coûteux aux libellés sans
 * espoir.
 */
function couldMatch(queryCounts: Map<string, number>, needed: number, label: Map<string, number>): boolean {
  let shared = 0;
  for (const [char, count] of queryCounts) {
    shared += Math.min(count, label.get(char) ?? 0);
    if (shared >= needed) return true;
  }
  return false;
}

/** Dénombre les caractères d'une chaîne. */
function countChars(text: string): Map<string, number> {
  const counts = new Map<string, number>();
  for (const char of text) counts.set(char, (counts.get(char) ?? 0) + 1);
  return counts;
}

/**
 * Libellés préparés une fois pour toutes.
 *
 * Replier les accents et dénombrer les caractères coûte cher rapporté à chaque
 * frappe ; ces calculs ne dépendent que des données, pas de la requête.
 */
export interface SearchIndex {
  entries: Array<{ raw: string; folded: string; chars: Map<string, number> }>;
  /** L'arbre des catégories, pour chercher « transport » et atteindre sa branche. */
  tree: CategoryTree;
}

/**
 * Prépare les libellés une fois pour toutes.
 *
 * Le texte indexé joint le libellé brut au nom de sa catégorie **et à ceux des
 * catégories dont elle relève** : une opération classée « Essence » répond
 * ainsi à « transport », sans que l'utilisateur ait à connaître ni les
 * commerçants du secteur ni le détail de son propre découpage.
 */
export function buildIndex<T extends Searchable>(
  points: T[],
  tree: CategoryTree = emptyTree(),
): SearchIndex {
  const seen = new Map<string, { raw: string; folded: string; chars: Map<string, number> }>();
  for (const point of points) {
    if (seen.has(point.raw)) continue;
    const lineage = [point.category, ...(tree.ancestors.get(point.category) ?? [])]
      .map((key) => tree.labels.get(key) ?? "")
      .join(" ");
    const folded = fold(`${point.raw} ${lineage}`);
    seen.set(point.raw, { raw: point.raw, folded, chars: countChars(folded) });
  }
  return { entries: [...seen.values()], tree };
}

/** Libellés bruts retenus par une requête. */
export function matchingLabels(index: SearchIndex, query: string): Set<string> {
  const folded = fold(query.trim());
  const found = new Set<string>();
  if (!folded) return found;

  const cap = tolerance(folded.length);
  const queryCounts = countChars(folded);
  const needed = folded.length - cap;

  for (const entry of index.entries) {
    if (entry.folded.includes(folded)) {
      found.add(entry.raw);
      continue;
    }
    if (cap === 0) continue;
    if (!couldMatch(queryCounts, needed, entry.chars)) continue;
    if (matches(folded, entry.folded, cap)) found.add(entry.raw);
  }
  return found;
}

/** Une proposition de la barre de recherche. */
export interface Suggestion {
  /** Ce que la proposition désigne : un bénéficiaire ou un secteur. */
  kind: "label" | "category";
  /** Valeur retenue en cas de clic : le libellé, ou le nom de catégorie. */
  value: string;
  /** Texte affiché. */
  label: string;
  /** Nombre d'opérations concernées. */
  count: number;
}

interface Searchable {
  description: string;
  raw: string;
  /** Nom stable de la catégorie, ex. « dining ». */
  category: string;
}

/** Ce que la recherche sait d'une sélection retenue. */
export type Selection =
  | { kind: "label"; value: string }
  | { kind: "category"; value: string };

/**
 * Propositions correspondant à une requête, les plus fréquentes d'abord.
 *
 * Les propositions portent le libellé *nettoyé* — c'est ce que l'utilisateur
 * lit — mais la correspondance se cherche dans le libellé complet.
 */
export function suggest<T extends Searchable>(
  points: T[],
  index: SearchIndex,
  query: string,
  limit = 8,
): Suggestion[] {
  const folded = fold(query.trim());
  if (!folded) return [];

  // Les secteurs viennent en tête : ils rassemblent bien plus d'opérations
  // qu'un commerçant, et c'est souvent ce qu'on cherche en tapant « resto ».
  const cap = tolerance(folded.length);
  const categories: Suggestion[] = [...index.tree.labels.entries()]
    .map(([key, label]): Suggestion | null => {
      // Nommée directement par la requête…
      const named = matches(folded, fold(label), cap);
      // …ou atteinte par une de celles dont elle relève : chercher
      // « Transport » doit faire apparaître « Transport › Péage », que
      // l'utilisateur n'a aucune raison de savoir nommer autrement.
      const inherited = (index.tree.ancestors.get(key) ?? []).some((ancestor) =>
        matches(folded, fold(index.tree.labels.get(ancestor) ?? ""), cap),
      );
      if (!named && !inherited) return null;

      // La branche entière est dénombrée : annoncer « Transport (12) » puis
      // n'en afficher que 4 démentirait la proposition au moment du clic.
      const count = points.filter((p) =>
        withinBranch(p.category, key, index.tree),
      ).length;

      // Une catégorie vide reste proposée quand la requête la nomme : la
      // taire laisserait croire qu'elle n'existe pas, alors qu'elle attend
      // seulement ses premières opérations. Atteinte par son seul parent,
      // elle est tue — sans quoi chercher un secteur ferait défiler toutes
      // ses branches désertes.
      if (count === 0 && !named) return null;

      return {
        kind: "category" as const,
        value: key,
        label: pathOf(key, index.tree),
        count,
      };
    })
    .filter((suggestion): suggestion is Suggestion => suggestion !== null)
    // Le parent avant ses branches, et les branches vides en dernier.
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));

  const retained = matchingLabels(index, query);

  // Les graphies d'un même bénéficiaire sont comptées ensemble, et la plus
  // fréquente sert d'étiquette.
  const counts = new Map<string, number>();
  const spellings = new Map<string, Map<string, number>>();

  for (const point of points) {
    if (!retained.has(point.raw)) continue;
    const key = fold(point.description);
    counts.set(key, (counts.get(key) ?? 0) + 1);

    const seen = spellings.get(key) ?? new Map<string, number>();
    seen.set(point.description, (seen.get(point.description) ?? 0) + 1);
    spellings.set(key, seen);
  }

  const labels: Suggestion[] = [...counts.entries()]
    .map(([key, count]): [string, number] => {
      const seen = [...(spellings.get(key) ?? new Map())];
      // À égalité, l'ordre alphabétique tranche pour que l'affichage ne
      // change pas d'une frappe à l'autre.
      seen.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
      return [seen[0]?.[0] ?? key, count];
    })
    .map(([label, count]) => ({ kind: "label" as const, value: label, label, count }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));

  return [...categories, ...labels].slice(0, limit);
}

/**
 * Restreint les points à ceux que la recherche retient.
 *
 * `exact` correspond au clic sur une proposition : on ne garde alors que le
 * libellé choisi, sans approximation — cliquer une proposition doit donner
 * exactement ce qu'elle annonce.
 */
export function filterPoints<T extends Searchable>(
  points: T[],
  index: SearchIndex,
  query: string,
  selection: Selection | null,
): T[] {
  if (selection?.kind === "category") {
    // La branche entière, pas le seul classement exact : choisir « Transport »
    // doit donner « Transport », « Essence » et « Péage » réunis.
    return points.filter((p) => withinBranch(p.category, selection.value, index.tree));
  }
  if (selection?.kind === "label") {
    // Comparaison repliée : la banque écrit « SCI Berthelot » un mois et
    // « SCI BERTHELOT » le suivant. Comparer à l'identique ne retiendrait
    // qu'une des deux graphies.
    const wanted = fold(selection.value);
    return points.filter((p) => fold(p.description) === wanted);
  }
  if (!query.trim()) return points;

  const retained = matchingLabels(index, query);
  return points.filter((point) => retained.has(point.raw));
}
