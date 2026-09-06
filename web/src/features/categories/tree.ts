import type { CategoryInfo } from "../../api/types";

/**
 * Profondeur maximale parcourue, en écho à la borne du serveur.
 *
 * Le serveur refuse déjà les boucles, mais l'affichage ne doit pas dépendre de
 * cette garantie : une réponse abîmée figerait l'onglet du visiteur.
 */
const MAX_DEPTH = 8;

/** Séparateur des chemins affichés : « Transport › Essence ». */
const SEPARATOR = " › ";

/**
 * Les catégories vues comme un arbre, préparées une fois pour toutes.
 *
 * Une catégorie peut relever d'une autre : « Essence » et « Péage » relèvent
 * de « Transport ». Les opérations gardent leur classement précis, et c'est ce
 * parcours ascendant qui les fait aussi compter dans le secteur qui les
 * englobe — chercher « Transport » atteint alors les trois, et leurs totaux se
 * cumulent sans qu'aucune opération soit comptée deux fois.
 */
export interface CategoryTree {
  /** Nom stable → libellé affiché. */
  labels: Map<string, string>;
  /** Nom stable → catégorie dont elle relève. */
  parents: Map<string, string>;
  /** Nom stable → elle-même et tout ce qu'elle contient, à toute profondeur. */
  branches: Map<string, Set<string>>;
  /** Nom stable → chaîne ascendante, du parent direct vers la racine. */
  ancestors: Map<string, string[]>;
  /** Nom stable → chemin lisible, « Transport › Essence ». */
  paths: Map<string, string>;
  /** Catégories sans parent, dans l'ordre d'affichage. */
  roots: string[];
  /** Nom stable → sous-catégories directes, dans l'ordre d'affichage. */
  children: Map<string, string[]>;
}

/** Un arbre vide, pour les appels qui n'ont pas encore de catégories. */
export function emptyTree(): CategoryTree {
  return {
    labels: new Map(),
    parents: new Map(),
    branches: new Map(),
    ancestors: new Map(),
    paths: new Map(),
    roots: [],
    children: new Map(),
  };
}

/**
 * Prépare l'arbre des catégories.
 *
 * Tout est calculé ici plutôt qu'à chaque frappe : la recherche interroge ces
 * tables à chaque caractère saisi, et remonter la chaîne à la volée coûterait
 * un parcours par opération et par frappe.
 */
export function buildTree(categories: CategoryInfo[]): CategoryTree {
  const tree = emptyTree();
  const known = new Set(categories.map((c) => c.key));

  for (const category of categories) {
    tree.labels.set(category.key, category.label);
    // Un parent absent de la liste — retiré entre deux chargements — est
    // ignoré : la catégorie est traitée comme une racine plutôt que rattachée
    // à un nom que rien ne permettrait d'afficher.
    if (category.parent && known.has(category.parent)) {
      tree.parents.set(category.key, category.parent);
    }
  }

  for (const category of categories) {
    const chain = ancestorsOf(category.key, tree.parents);
    tree.ancestors.set(category.key, chain);
    tree.paths.set(
      category.key,
      [...chain]
        .reverse()
        .concat(category.key)
        .map((key) => tree.labels.get(key) ?? key)
        .join(SEPARATOR),
    );

    const parent = tree.parents.get(category.key);
    if (parent === undefined) {
      tree.roots.push(category.key);
    } else {
      tree.children.set(parent, [...(tree.children.get(parent) ?? []), category.key]);
    }
  }

  // Chaque catégorie s'appartient : totaliser « Transport » compte les
  // opérations classées « Transport » autant que celles de ses branches.
  for (const category of categories) {
    tree.branches.set(category.key, new Set([category.key]));
  }
  for (const category of categories) {
    for (const ancestor of tree.ancestors.get(category.key) ?? []) {
      tree.branches.get(ancestor)?.add(category.key);
    }
  }

  return tree;
}

/**
 * Chaîne ascendante d'une catégorie, du parent direct vers la racine.
 *
 * Le parcours s'arrête sur une clé déjà vue : une boucle ne doit pas figer
 * l'affichage, même si le serveur la refuse à l'écriture.
 */
export function ancestorsOf(key: string, parents: Map<string, string>): string[] {
  const chain: string[] = [];
  let current = key;
  for (let depth = 0; depth < MAX_DEPTH; depth += 1) {
    const parent = parents.get(current);
    if (parent === undefined || parent === key || chain.includes(parent)) break;
    chain.push(parent);
    current = parent;
  }
  return chain;
}

/**
 * Une catégorie et tout ce qu'elle contient.
 *
 * C'est ce qui donne son sens à « la somme de Transport » : le total embrasse
 * la branche entière, pas le seul classement exact.
 */
export function branchOf(key: string, tree: CategoryTree): Set<string> {
  return tree.branches.get(key) ?? new Set([key]);
}

/** Vrai si une opération relève de cette catégorie, directement ou non. */
export function withinBranch(category: string, key: string, tree: CategoryTree): boolean {
  return branchOf(key, tree).has(category);
}

/** Chemin lisible d'une catégorie, « Transport › Essence ». */
export function pathOf(key: string, tree: CategoryTree): string {
  return tree.paths.get(key) ?? key;
}

/**
 * Les catégories dans l'ordre de l'arbre, chacune avec sa profondeur.
 *
 * Sert aux listes et aux menus déroulants, où l'emboîtement se lit à
 * l'indentation.
 */
export function flatten(tree: CategoryTree): Array<{ key: string; depth: number }> {
  const ordered: Array<{ key: string; depth: number }> = [];
  const walk = (key: string, depth: number) => {
    ordered.push({ key, depth });
    for (const child of tree.children.get(key) ?? []) walk(child, depth + 1);
  };
  for (const root of tree.roots) walk(root, 0);
  return ordered;
}

/**
 * Les catégories sous lesquelles `key` peut être rangée.
 *
 * Sa propre branche en est exclue : l'y rattacher formerait une boucle, que le
 * serveur refuserait. Mieux vaut ne pas la proposer que de la faire échouer.
 */
export function possibleParents(key: string, tree: CategoryTree): string[] {
  const forbidden = branchOf(key, tree);
  return flatten(tree)
    .map((entry) => entry.key)
    .filter((candidate) => !forbidden.has(candidate));
}
