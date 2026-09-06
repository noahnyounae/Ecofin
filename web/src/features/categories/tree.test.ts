import { describe, expect, it } from "vitest";
import type { CategoryInfo } from "../../api/types";
import {
  ancestorsOf,
  branchOf,
  buildTree,
  flatten,
  pathOf,
  possibleParents,
  withinBranch,
} from "./tree";

function categorie(
  key: string,
  label: string,
  parent: string | null = null,
): CategoryInfo {
  return { key, label, is_spending: true, builtin: false, parent };
}

/** « Essence » et « Péage » relèvent de « Transport ». */
const TRANSPORT = [
  categorie("transport", "Transport"),
  categorie("u_essence", "Essence", "transport"),
  categorie("u_peage", "Péage", "transport"),
  categorie("u_diesel", "Diesel", "u_essence"),
  categorie("groceries", "Alimentation"),
];

describe("branchOf", () => {
  const tree = buildTree(TRANSPORT);

  // C'est ce qui donne son sens à « la somme de Transport ».
  it("embrasse la catégorie et tout ce qu'elle contient", () => {
    expect(branchOf("transport", tree)).toEqual(
      new Set(["transport", "u_essence", "u_peage", "u_diesel"]),
    );
  });

  it("descend à toute profondeur", () => {
    expect(branchOf("u_essence", tree)).toEqual(new Set(["u_essence", "u_diesel"]));
  });

  it("laisse une catégorie sans enfant à elle-même", () => {
    expect(branchOf("groceries", tree)).toEqual(new Set(["groceries"]));
  });

  // Une opération classée « Essence » relève aussi de « Transport », mais pas
  // l'inverse : le cumul monte, il ne redescend pas.
  it("ne remonte pas d'une sous-catégorie vers ses sœurs", () => {
    expect(withinBranch("u_essence", "transport", tree)).toBe(true);
    expect(withinBranch("transport", "u_essence", tree)).toBe(false);
    expect(withinBranch("u_peage", "u_essence", tree)).toBe(false);
  });
});

describe("ancestorsOf", () => {
  const tree = buildTree(TRANSPORT);

  it("remonte du parent direct vers la racine", () => {
    expect(ancestorsOf("u_diesel", tree.parents)).toEqual(["u_essence", "transport"]);
  });

  it("ne rend rien pour une racine", () => {
    expect(ancestorsOf("transport", tree.parents)).toEqual([]);
  });

  // Le serveur refuse les boucles, mais l'affichage ne doit pas en dépendre :
  // une réponse abîmée figerait l'onglet.
  it("s'arrête sur une boucle au lieu de tourner", () => {
    const boucle = new Map([
      ["a", "b"],
      ["b", "c"],
      ["c", "a"],
    ]);
    expect(ancestorsOf("a", boucle)).toEqual(["b", "c"]);
  });
});

describe("pathOf", () => {
  const tree = buildTree(TRANSPORT);

  it("écrit le chemin de la racine vers la feuille", () => {
    expect(pathOf("u_diesel", tree)).toBe("Transport › Essence › Diesel");
    expect(pathOf("transport", tree)).toBe("Transport");
  });
});

describe("flatten", () => {
  it("range les catégories dans l'ordre de l'arbre, avec leur profondeur", () => {
    expect(flatten(buildTree(TRANSPORT))).toEqual([
      { key: "transport", depth: 0 },
      { key: "u_essence", depth: 1 },
      { key: "u_diesel", depth: 2 },
      { key: "u_peage", depth: 1 },
      { key: "groceries", depth: 0 },
    ]);
  });
});

describe("possibleParents", () => {
  const tree = buildTree(TRANSPORT);

  // Proposer sa propre branche mènerait à une boucle, que le serveur
  // refuserait : mieux vaut ne pas l'offrir que la faire échouer.
  it("écarte la catégorie elle-même et sa descendance", () => {
    expect(possibleParents("u_essence", tree)).toEqual([
      "transport",
      "u_peage",
      "groceries",
    ]);
  });
});

describe("buildTree", () => {
  // Une catégorie retirée entre deux chargements laisserait ses enfants
  // pointer sur un nom qu'on ne saurait pas afficher.
  it("traite comme racine un enfant dont le parent a disparu", () => {
    const tree = buildTree([categorie("u_essence", "Essence", "disparue")]);
    expect(tree.roots).toEqual(["u_essence"]);
    expect(pathOf("u_essence", tree)).toBe("Essence");
  });
});
