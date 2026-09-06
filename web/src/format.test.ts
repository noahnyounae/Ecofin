import { describe, expect, it } from "vitest";
import { date, money, moneyCompact, moneyFromCents, plural, toCents, totals } from "./format";

describe("toCents", () => {
  it("lit un montant décimal", () => {
    expect(toCents("12.50")).toBe(1250);
    expect(toCents("-12.50")).toBe(-1250);
    expect(toCents("1000")).toBe(100_000);
  });

  it("complète une décimale manquante", () => {
    expect(toCents("12.5")).toBe(1250);
  });

  // Aucune banque ne facture au millième ; au-delà de deux décimales, on
  // tronque plutôt que d'inventer un arrondi.
  it("tronque au-delà de deux décimales", () => {
    expect(toCents("12.509")).toBe(1250);
  });

  it("signale une entrée illisible", () => {
    expect(toCents("douze euros")).toBeNaN();
    expect(toCents("")).toBeNaN();
  });
});

describe("totals", () => {
  it("sépare les entrées des sorties", () => {
    const t = totals(["100.00", "-30.00", "-20.50"]);
    expect(t.credits).toBe(10_000);
    expect(t.debits).toBe(-5050);
    expect(t.net).toBe(4950);
    expect(t.count).toBe(3);
  });

  // Les montants voyagent en chaîne précisément pour ne pas traverser un
  // flottant : les additionner en `Number` rendrait la précaution vaine.
  it("additionne sans dérive de virgule flottante", () => {
    const montants = Array(10).fill("0.10");
    expect(totals(montants).net).toBe(100);

    const flottant = montants.reduce((s, m) => s + Number(m), 0);
    expect(flottant).not.toBe(1);
  });

  it("ignore une ligne illisible plutôt que de tout casser", () => {
    const t = totals(["100.00", "n/a", "-40.00"]);
    expect(t.net).toBe(6000);
    expect(t.count).toBe(2);
  });

  it("accepte une liste vide", () => {
    expect(totals([])).toEqual({ credits: 0, debits: 0, net: 0, count: 0 });
  });
});

describe("plural", () => {
  it("garde le singulier à zéro et à un", () => {
    expect(plural(0, "opération")).toBe("0 opération");
    expect(plural(1, "opération")).toBe("1 opération");
  });

  it("accorde au-delà", () => {
    expect(plural(4, "opération")).toBe("4 opérations");
  });
});

describe("money", () => {
  it("met en forme à la française", () => {
    // Espace insécable étroite comme séparateur, virgule décimale.
    expect(money("1234.56")).toMatch(/1\s?234,56/);
    expect(money("1234.56")).toContain("€");
  });

  it("marque les montants négatifs", () => {
    expect(money("-12.50")).toContain("-");
  });

  it("rend un tiret sur une entrée illisible", () => {
    expect(money("n/a")).toBe("—");
  });

  it("convertit depuis les centimes", () => {
    expect(moneyFromCents(1250)).toBe(money("12.50"));
  });

  it("abrège pour les axes, où la place manque", () => {
    expect(moneyCompact(12_500)).toMatch(/12,5\s?k/);
  });
});

describe("date", () => {
  it("met en forme une date ISO", () => {
    expect(date("2026-08-30")).toMatch(/30/);
  });

  it("rend un tiret sur une valeur absente ou illisible", () => {
    expect(date(null)).toBe("—");
    expect(date("pas une date")).toBe("—");
  });
});
