import type { WalletInfo } from "../../api/types";

/**
 * Profondeur maximale parcourue, en écho à la borne du serveur.
 *
 * Le serveur refuse déjà les boucles, mais l'affichage ne doit pas dépendre de
 * cette garantie : une réponse abîmée figerait l'onglet du visiteur.
 */
const MAX_DEPTH = 8;

/** Une enveloppe et son rang dans l'emboîtement. */
export interface Placed {
  wallet: WalletInfo;
  depth: number;
}

/**
 * Range les enveloppes dans l'ordre de l'arbre, chacune avec sa profondeur.
 *
 * Une fille est prise sur la dotation de sa mère : l'indentation le donne à
 * voir sans avoir à l'expliquer, et l'ordre place chaque enveloppe sous celle
 * qui la finance.
 *
 * Une enveloppe dont la mère a disparu entre deux chargements est traitée
 * comme étant au premier rang : elle reste visible, plutôt que d'être omise de
 * la liste sans que rien ne le signale.
 */
export function arrange(wallets: WalletInfo[]): Placed[] {
  const known = new Set(wallets.map((w) => w.id));
  const children = new Map<number | null, WalletInfo[]>();

  for (const wallet of wallets) {
    const parent = wallet.parent !== null && known.has(wallet.parent) ? wallet.parent : null;
    children.set(parent, [...(children.get(parent) ?? []), wallet]);
  }

  const placed: Placed[] = [];
  const walk = (parent: number | null, depth: number) => {
    if (depth > MAX_DEPTH) return;
    for (const wallet of children.get(parent) ?? []) {
      placed.push({ wallet, depth });
      walk(wallet.id, depth + 1);
    }
  };
  walk(null, 0);

  // Une boucle laisserait des enveloppes hors de l'arbre : elles sont
  // rattrapées au premier rang plutôt que de disparaître de l'écran.
  const seen = new Set(placed.map((p) => p.wallet.id));
  for (const wallet of wallets) {
    if (!seen.has(wallet.id)) placed.push({ wallet, depth: 0 });
  }
  return placed;
}

/**
 * Les enveloppes où celle-ci peut être rangée.
 *
 * Sa propre descendance en est exclue : l'y ranger formerait une boucle, que
 * le serveur refuserait. Mieux vaut ne pas la proposer que la faire échouer.
 */
export function possibleParents(id: number, wallets: WalletInfo[]): WalletInfo[] {
  const forbidden = new Set([id]);
  for (let depth = 0; depth < MAX_DEPTH; depth += 1) {
    const before = forbidden.size;
    for (const wallet of wallets) {
      if (wallet.parent !== null && forbidden.has(wallet.parent)) forbidden.add(wallet.id);
    }
    if (forbidden.size === before) break;
  }
  return wallets.filter((wallet) => !forbidden.has(wallet.id));
}
