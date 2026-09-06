import { describe, expect, it } from "vitest";
import type { WalletInfo } from "../../api/types";
import { arrange, possibleParents } from "./nesting";

function enveloppe(id: number, name: string, parent: number | null = null): WalletInfo {
  return {
    id,
    name,
    allocation: "0",
    categories: [],
    carry_to: null,
    parent,
    effective_allocation: "0",
    children_allocation: "0",
    branch_balance: "0",
    overallocated: false,
    start: "2026-01",
    carried_in: "0",
    movements: "0",
    available: "0",
    balance: "0",
  };
}

/** Vie courante contient Alimentation, qui contient Restaurants. */
const ARBRE = [
  enveloppe(1, "Vie courante"),
  enveloppe(2, "Alimentation", 1),
  enveloppe(3, "Restaurants", 2),
  enveloppe(4, "Abonnements"),
];

describe("arrange", () => {
  it("place chaque enveloppe sous celle qui la finance", () => {
    expect(arrange(ARBRE).map((p) => [p.wallet.name, p.depth])).toEqual([
      ["Vie courante", 0],
      ["Alimentation", 1],
      ["Restaurants", 2],
      ["Abonnements", 0],
    ]);
  });

  // Une enveloppe omise de la liste disparaîtrait de l'écran sans que rien ne
  // le signale : elle est rattrapée au premier rang.
  it("remonte au premier rang une enveloppe dont la mère a disparu", () => {
    const orpheline = [enveloppe(2, "Alimentation", 404)];
    expect(arrange(orpheline)).toEqual([{ wallet: orpheline[0], depth: 0 }]);
  });

  it("n'oublie personne, même dans une boucle", () => {
    const a = enveloppe(1, "A", 2);
    const b = enveloppe(2, "B", 1);
    expect(arrange([a, b]).map((p) => p.wallet.name).sort()).toEqual(["A", "B"]);
  });

  it("ne rend rien pour une liste vide", () => {
    expect(arrange([])).toEqual([]);
  });
});

describe("possibleParents", () => {
  // Proposer sa propre descendance mènerait à une boucle, que le serveur
  // refuserait : mieux vaut ne pas l'offrir.
  it("écarte l'enveloppe elle-même et tout ce qu'elle contient", () => {
    expect(possibleParents(1, ARBRE).map((w) => w.name)).toEqual(["Abonnements"]);
  });

  it("laisse le choix large à une feuille", () => {
    expect(possibleParents(4, ARBRE).map((w) => w.name)).toEqual([
      "Vie courante",
      "Alimentation",
      "Restaurants",
    ]);
  });

  it("écarte la descendance à toute profondeur", () => {
    expect(possibleParents(2, ARBRE).map((w) => w.name)).toEqual([
      "Vie courante",
      "Abonnements",
    ]);
  });
});
