import { describe, expect, it } from "vitest";
import { buildIndex, filterPoints, fold, matches, suggest, tolerance } from "./search";
import { buildTree } from "../categories/tree";
import { totals } from "../../format";

/** Un point réduit à ce dont la recherche a besoin. */
const point = (raw: string, description = raw, category = "uncategorised") => ({
  raw,
  description,
  category,
});

/** Construit un arbre de catégories à partir de couples clé/libellé/parent. */
function secteurs(
  ...defs: Array<[key: string, label: string, parent?: string]>
) {
  return buildTree(
    defs.map(([key, label, parent]) => ({
      key,
      label,
      is_spending: true,
      builtin: true,
      parent: parent ?? null,
    })),
  );
}

const SECTEURS = secteurs(
  ["dining", "Restauration"],
  ["groceries", "Alimentation"],
  ["subscriptions", "Abonnements"],
);

describe("tolerance", () => {
  // Mesuré sur 1405 libellés réels : à distance 3, « uber » correspondait à
  // *tous* les libellés. Un seuil fixe ne peut pas convenir aux deux
  // extrêmes.
  it("n'accorde aucune tolérance à une requête courte", () => {
    expect(tolerance(1)).toBe(0);
    expect(tolerance(3)).toBe(0);
  });

  it("croît avec la longueur de la requête", () => {
    expect(tolerance(4)).toBe(1);
    expect(tolerance(8)).toBe(2);
    expect(tolerance(12)).toBe(3);
  });

  it("plafonne, pour qu'une requête longue ne ratisse pas large", () => {
    expect(tolerance(40)).toBe(3);
  });
});

describe("fold", () => {
  it("efface la casse et les accents", () => {
    expect(fold("Régie d'Eau")).toBe(fold("REGIE D'EAU"));
    expect(fold("Burger King")).toBe(fold("BURGER KING"));
  });

  it("ne confond pas deux bénéficiaires distincts", () => {
    expect(fold("UBER *EATS")).not.toBe(fold("UBER *ONE"));
  });
});

describe("matches", () => {
  const label = fold("CARTE 0602685 CB POINT P 1233 26/08/26");

  it("trouve la requête au milieu du libellé", () => {
    expect(matches(fold("POINT P"), label, 0)).toBe(true);
  });

  it("tolère une faute de frappe quand la requête est assez longue", () => {
    expect(matches(fold("NETFLX"), fold("CB NETFLIX COM"), 1)).toBe(true);
  });

  it("refuse au-delà de la tolérance", () => {
    expect(matches(fold("SPOTIFY"), fold("CB NETFLIX COM"), 1)).toBe(false);
  });

  // Comparer la requête au libellé entier n'aurait aucun sens : « uber » et
  // « CB UBER *EATS 12/03/26 » sont à une distance énorme l'un de l'autre.
  it("compare à une fenêtre, pas au libellé entier", () => {
    expect(matches(fold("UBER"), fold("CB UBER *EATS 12/03/26"), 0)).toBe(true);
  });
});

describe("suggest", () => {
  const points = [
    ...Array(8).fill(point("CB Burger King 01/02/26", "Burger King")),
    ...Array(2).fill(point("CB BURGER KING 03/02/26", "BURGER KING")),
    point("CB NETFLIX COM 04/02/26", "NETFLIX COM"),
  ];
  const index = buildIndex(points);

  // Sans repli, « Burger King » et « BURGER KING » apparaissaient comme deux
  // propositions distinctes.
  it("réunit les graphies d'un même bénéficiaire", () => {
    const trouve = suggest(points, index, "burger", 5);
    expect(trouve).toHaveLength(1);
    expect(trouve[0]!.count).toBe(10);
  });

  it("retient la graphie la plus fréquente", () => {
    expect(suggest(points, index, "burger", 5)[0]!.label).toBe("Burger King");
  });

  it("classe les propositions par fréquence", () => {
    const trouve = suggest(points, index, "c", 5);
    expect(trouve[0]!.count).toBeGreaterThanOrEqual(trouve[1]?.count ?? 0);
  });

  it("ne propose rien sur une requête vide", () => {
    expect(suggest(points, index, "   ", 5)).toEqual([]);
  });
});

describe("filterPoints", () => {
  const points = [
    point("CB Burger King 01/02/26", "Burger King"),
    point("CB BURGER KING 03/02/26", "BURGER KING"),
    point("CB NETFLIX COM 04/02/26", "NETFLIX COM"),
  ];
  const index = buildIndex(points);

  it("rend tous les points sans recherche", () => {
    expect(filterPoints(points, index, "", null)).toHaveLength(3);
  });

  // Le filtre exact naissait d'un clic sur une proposition : comparer les
  // chaînes à l'identique n'en retenait qu'une graphie sur deux.
  it("retient toutes les graphies sur un filtre exact", () => {
    const cible = { kind: "label" as const, value: "Burger King" };
    expect(filterPoints(points, index, "", cible)).toHaveLength(2);
    expect(
      filterPoints(points, index, "", { kind: "label", value: "BURGER KING" }),
    ).toHaveLength(2);
  });

  it("cherche dans le libellé complet, pas seulement l'affiché", () => {
    // « 0602685 » n'apparaît que dans le libellé brut.
    const brut = [point("CARTE 0602685 CB POINT P", "POINT P")];
    expect(filterPoints(brut, buildIndex(brut), "0602685", null)).toHaveLength(1);
  });

  it("ne rend rien quand la recherche ne correspond à personne", () => {
    expect(filterPoints(points, index, "carrefour", null)).toHaveLength(0);
  });
});

describe("recherche par secteur", () => {
  const points = [
    point("CB MC DONALDS 01/02/26", "MC DONALDS", "dining"),
    point("CB BURGER KING 02/02/26", "BURGER KING", "dining"),
    point("CB INTERMARCHE 03/02/26", "INTERMARCHE", "groceries"),
    point("CB NETFLIX 04/02/26", "NETFLIX", "subscriptions"),
  ];
  const index = buildIndex(points, SECTEURS);

  // Taper le nom d'un secteur doit atteindre ses opérations, sans avoir à
  // connaître les commerçants qui le composent.
  it("trouve les opérations d'un secteur par son nom", () => {
    expect(filterPoints(points, index, "restauration", null)).toHaveLength(2);
    expect(filterPoints(points, index, "alimentation", null)).toHaveLength(1);
  });

  it("tolère une faute sur le nom du secteur", () => {
    expect(filterPoints(points, index, "restauraton", null)).toHaveLength(2);
  });

  it("propose le secteur avant les bénéficiaires", () => {
    const trouve = suggest(points, index, "restauration", 5);
    expect(trouve[0]!.kind).toBe("category");
    expect(trouve[0]!.value).toBe("dining");
    expect(trouve[0]!.count).toBe(2);
  });

  // Taire une catégorie vide que la requête nomme laisserait croire qu'elle
  // n'existe pas, alors qu'elle attend seulement ses premières opérations.
  it("propose un secteur vide que la requête nomme, à zéro", () => {
    const vide = buildIndex([], secteurs(["health", "Santé"]));
    const trouve = suggest([], vide, "sante", 5);
    expect(trouve).toHaveLength(1);
    expect(trouve[0]!.value).toBe("health");
    expect(trouve[0]!.count).toBe(0);
  });

  it("ne propose rien pour une requête qui ne nomme aucun secteur", () => {
    const vide = buildIndex([], secteurs(["health", "Santé"]));
    expect(suggest([], vide, "charabia", 5)).toEqual([]);
  });

  it("filtre exactement sur un secteur choisi", () => {
    const retenu = filterPoints(points, index, "", { kind: "category", value: "dining" });
    expect(retenu).toHaveLength(2);
    expect(retenu.every((p) => p.category === "dining")).toBe(true);
  });

  // Le secteur choisi l'emporte sur le texte tapé : cliquer une proposition
  // doit donner exactement ce qu'elle annonce.
  it("ignore le texte quand un secteur est retenu", () => {
    const retenu = filterPoints(points, index, "netflix", {
      kind: "category",
      value: "groceries",
    });
    expect(retenu).toHaveLength(1);
    expect(retenu[0]!.description).toBe("INTERMARCHE");
  });

  it("cherche toujours dans les libellés, secteurs ou non", () => {
    expect(filterPoints(points, index, "netflix", null)).toHaveLength(1);
  });
});

describe("recherche par secteur emboîté", () => {
  // « Essence » et « Péage » relèvent de « Transport » : les opérations gardent
  // leur classement précis, et le secteur parent les embrasse toutes.
  const SECTEUR = secteurs(
    ["transport", "Transport"],
    ["u_essence", "Essence", "transport"],
    ["u_peage", "Péage", "transport"],
    ["groceries", "Alimentation"],
  );
  const points = [
    point("CB SNCF 01/02/26", "SNCF", "transport"),
    point("CB TOTAL 02/02/26", "TOTAL", "u_essence"),
    point("CB VINCI 03/02/26", "VINCI", "u_peage"),
    point("CB INTERMARCHE 04/02/26", "INTERMARCHE", "groceries"),
  ];
  const index = buildIndex(points, SECTEUR);

  it("atteint la branche entière par le nom du parent", () => {
    expect(filterPoints(points, index, "transport", null)).toHaveLength(3);
  });

  it("garde la sous-catégorie atteignable par son propre nom", () => {
    expect(filterPoints(points, index, "essence", null)).toHaveLength(1);
  });

  // Choisir « Transport » doit donner ce que la proposition annonçait.
  it("filtre sur la branche quand le parent est choisi", () => {
    const retenu = filterPoints(points, index, "", {
      kind: "category",
      value: "transport",
    });
    expect(retenu.map((p) => p.description).sort()).toEqual([
      "SNCF",
      "TOTAL",
      "VINCI",
    ]);
  });

  it("ne remonte pas d'une sous-catégorie vers ses sœurs", () => {
    const retenu = filterPoints(points, index, "", {
      kind: "category",
      value: "u_essence",
    });
    expect(retenu.map((p) => p.description)).toEqual(["TOTAL"]);
  });

  // Annoncer « Transport (3) » puis n'en afficher qu'une démentirait la
  // proposition au moment du clic.
  it("dénombre la branche entière dans la proposition", () => {
    const trouve = suggest(points, index, "transport", 5);
    expect(trouve[0]!.kind).toBe("category");
    expect(trouve[0]!.value).toBe("transport");
    expect(trouve[0]!.count).toBe(3);
  });

  it("nomme la proposition par son chemin complet", () => {
    const trouve = suggest(points, index, "essence", 5);
    expect(trouve[0]!.label).toBe("Transport › Essence");
  });

  it("laisse un secteur voisin en dehors de la branche", () => {
    expect(filterPoints(points, index, "transport", null)).not.toContainEqual(
      expect.objectContaining({ description: "INTERMARCHE" }),
    );
  });
});

describe("totaux d'une branche", () => {
  // Ce que l'écran additionne, ce sont les points affichés : filtrer sur un
  // secteur parent doit donc en cumuler les sous-catégories, sans compter
  // deux fois une opération qui relève des deux.
  const SECTEUR = secteurs(
    ["transport", "Transport"],
    ["u_essence", "Essence", "transport"],
    ["u_peage", "Péage", "transport"],
    ["groceries", "Alimentation"],
  );
  const points = [
    { raw: "SNCF", description: "SNCF", category: "transport", amount: "-30.00" },
    { raw: "TOTAL", description: "TOTAL", category: "u_essence", amount: "-60.00" },
    { raw: "VINCI", description: "VINCI", category: "u_peage", amount: "-10.00" },
    { raw: "INTERMARCHE", description: "INTERMARCHE", category: "groceries", amount: "-25.00" },
  ];
  const index = buildIndex(points, SECTEUR);

  it("additionne le parent et ses sous-catégories", () => {
    const visible = filterPoints(points, index, "", {
      kind: "category",
      value: "transport",
    });
    // 30 + 60 + 10, et pas l'alimentation.
    expect(totals(visible.map((p) => p.amount)).debits).toBe(-10000);
    expect(totals(visible.map((p) => p.amount)).count).toBe(3);
  });

  it("s'en tient à la sous-catégorie quand c'est elle qu'on choisit", () => {
    const visible = filterPoints(points, index, "", {
      kind: "category",
      value: "u_essence",
    });
    expect(totals(visible.map((p) => p.amount)).debits).toBe(-6000);
  });
});

describe("propositions d'une branche", () => {
  const SECTEUR = secteurs(
    ["transport", "Transport"],
    ["u_peage", "Péage", "transport"],
    ["u_essence", "Essence", "transport"],
    ["groceries", "Alimentation"],
  );
  const points = [
    point("CB ESCOTA 28/08/26", "ESCOTA", "u_peage"),
    point("CB TCL 69 LYO 20/08/26", "TCL 69 LYO", "transport"),
    point("CB INTERMARCHE 21/08/26", "INTERMARCHE", "groceries"),
  ];
  const index = buildIndex(points, SECTEUR);

  // Chercher le secteur qui les englobe doit faire apparaître ses branches :
  // l'utilisateur n'a aucune raison de savoir les nommer une par une.
  it("fait apparaître les sous-secteurs quand on cherche leur racine", () => {
    const trouve = suggest(points, index, "transport", 8)
      .filter((s) => s.kind === "category")
      .map((s) => s.label);
    expect(trouve).toContain("Transport");
    expect(trouve).toContain("Transport › Péage");
  });

  it("compte la branche entière sur la racine", () => {
    const racine = suggest(points, index, "transport", 8).find(
      (s) => s.value === "transport",
    );
    expect(racine!.count).toBe(2);
  });

  // Une sous-catégorie fraîchement créée n'a encore aucune opération : la
  // taire la rendrait introuvable, et donc inutilisable.
  it("propose une sous-catégorie encore vide quand on la nomme", () => {
    const trouve = suggest(points, index, "essence", 8);
    expect(trouve[0]!.label).toBe("Transport › Essence");
    expect(trouve[0]!.count).toBe(0);
  });

  // En revanche, chercher la racine ne doit pas faire défiler ses branches
  // désertes : elles n'apprendraient rien.
  it("tait les branches vides atteintes par leur seule racine", () => {
    const trouve = suggest(points, index, "transport", 8).map((s) => s.label);
    expect(trouve).not.toContain("Transport › Essence");
  });

  it("laisse un secteur voisin hors de la branche", () => {
    const trouve = suggest(points, index, "transport", 8).map((s) => s.label);
    expect(trouve).not.toContain("Alimentation");
  });

  it("garde la sous-catégorie atteignable par son propre nom", () => {
    const trouve = suggest(points, index, "peage", 8);
    expect(trouve[0]!.value).toBe("u_peage");
    expect(trouve[0]!.count).toBe(1);
  });
});
